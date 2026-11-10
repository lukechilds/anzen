//! Pure-Rust curve arithmetic and keys, for signers with memory to spare.
//!
//! A secure element runs the signer on its own protected arithmetic (the Ledger app does). A
//! signer whose key lives in a general-purpose microcontroller, such as a Trezor app, has no such
//! arithmetic to call and plenty of memory, so it uses this: the `k256` crate, constant time but
//! not hardened against someone probing the chip. Both run the same [`crate::musig`] code, and
//! the tests check this implementation against the host's.

use crate::{
    CooperativeSession, Keys,
    musig::{self, Arithmetic},
};
use anzen_protocol::{
    Error, Result,
    crypto::{Crypto, TweakedKey},
    script::XOnlyKey,
    wire::CooperativeSignature,
};
use k256::{
    AffinePoint, EncodedPoint, ProjectivePoint, Scalar, U256,
    elliptic_curve::{
        PrimeField,
        group::Group,
        ops::Reduce,
        sec1::{FromEncodedPoint, ToEncodedPoint},
        subtle::ConstantTimeEq,
    },
    schnorr::SigningKey,
};
use sha2::{Digest, Sha256};
use zeroize::Zeroize;

fn scalar(bytes: &[u8; 32]) -> Result<Scalar> {
    Option::from(Scalar::from_repr((*bytes).into())).ok_or(Error::InvalidKey)
}

fn point(bytes: &[u8; 33]) -> Result<ProjectivePoint> {
    let encoded = EncodedPoint::from_bytes(bytes).map_err(|_| Error::InvalidKey)?;
    Option::<AffinePoint>::from(AffinePoint::from_encoded_point(&encoded))
        .map(ProjectivePoint::from)
        .ok_or(Error::InvalidKey)
}

/// The compressed point. The point at infinity has none.
fn compress(point: ProjectivePoint) -> Result<[u8; 33]> {
    if bool::from(point.is_identity()) {
        return Err(Error::InvalidKey);
    }
    point
        .to_affine()
        .to_encoded_point(true)
        .as_bytes()
        .try_into()
        .map_err(|_| Error::InvalidKey)
}

fn even(key: &[u8; 32]) -> [u8; 33] {
    let mut even = [0x02; 33];
    even[1..].copy_from_slice(key);
    even
}

/// The protocol's primitives and the MuSig2 arithmetic, on `k256`.
#[derive(Debug, Default, Clone, Copy)]
pub struct SoftwareArithmetic;

impl Crypto for SoftwareArithmetic {
    fn sha256(&mut self, parts: &[&[u8]]) -> [u8; 32] {
        let mut hasher = Sha256::new();
        for part in parts {
            hasher.update(part);
        }
        hasher.finalize().into()
    }

    fn is_valid_xonly_key(&mut self, key: &[u8; 32]) -> bool {
        point(&even(key)).is_ok()
    }

    fn taproot_tweak(&mut self, internal_key: &[u8; 32], tweak: &[u8; 32]) -> Result<TweakedKey> {
        let output =
            compress(point(&even(internal_key))? + ProjectivePoint::GENERATOR * scalar(tweak)?)?;
        let mut output_key = [0_u8; 32];
        output_key.copy_from_slice(&output[1..]);
        Ok(TweakedKey {
            output_key,
            odd_y: output[0] == 0x03,
        })
    }

    fn mul_add(&mut self, a: &[u8; 32], first: &[u8; 33], second: &[u8; 33]) -> Result<[u8; 33]> {
        compress(point(first)? * scalar(a)? + point(second)?)
    }
}

impl Arithmetic for SoftwareArithmetic {
    fn reduce(&mut self, a: &[u8; 32]) -> Result<[u8; 32]> {
        Ok(<Scalar as Reduce<U256>>::reduce_bytes(&(*a).into())
            .to_bytes()
            .into())
    }

    fn add(&mut self, a: &[u8; 32], b: &[u8; 32]) -> Result<[u8; 32]> {
        Ok((scalar(a)? + scalar(b)?).to_bytes().into())
    }

    fn mul(&mut self, a: &[u8; 32], b: &[u8; 32]) -> Result<[u8; 32]> {
        Ok((scalar(a)? * scalar(b)?).to_bytes().into())
    }

    fn base_mul(&mut self, k: &[u8; 32]) -> Result<[u8; 33]> {
        compress(ProjectivePoint::GENERATOR * scalar(k)?)
    }
}

/// A vault key held in memory. Wiped when dropped.
pub struct SoftwareKey([u8; 32]);

impl SoftwareKey {
    /// `secret` must be a valid secp256k1 secret key.
    pub fn new(secret: [u8; 32]) -> Result<Self> {
        if bool::from(scalar(&secret)?.is_zero()) {
            return Err(Error::InvalidKey);
        }
        Ok(Self(secret))
    }
}

/// Which of the signer's two memories a sealed state is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum Place {
    /// The vault this signer accepted, and the last policy it approved.
    Vault = 1,
    /// The vault of an approved phone rotation, not yet adopted.
    Rotated = 2,
}

impl SoftwareKey {
    /// A seal over state this signer hands to its host to keep.
    ///
    /// A signer with no storage of its own remembers by proxy: it seals what it would have
    /// stored, the host keeps it, and the signer takes it back only with its own seal on it. The
    /// seal is HMAC-SHA256 under a key derived from the vault key, so no one else can make one,
    /// and it covers `place`, so the state of an unadopted rotation cannot pass for the vault's.
    ///
    /// A seal proves the signer once held this state, not that it is the latest: a host can
    /// present an older one.
    pub fn seal(&self, place: Place, state: &[u8]) -> [u8; 32] {
        const BLOCK: usize = 64;
        let mut key = [0_u8; BLOCK];
        key[..32].copy_from_slice(
            &Sha256::new()
                .chain_update(b"Anzen/sealed-state/v1")
                .chain_update(self.0)
                .finalize(),
        );
        let (mut inner, mut outer) = ([0x36_u8; BLOCK], [0x5c_u8; BLOCK]);
        for index in 0..BLOCK {
            inner[index] ^= key[index];
            outer[index] ^= key[index];
        }
        let digest = Sha256::new()
            .chain_update(inner)
            .chain_update([place as u8])
            .chain_update(state)
            .finalize();
        let seal = Sha256::new()
            .chain_update(outer)
            .chain_update(digest)
            .finalize()
            .into();
        key.zeroize();
        inner.zeroize();
        outer.zeroize();
        seal
    }

    /// Whether `seal` is this signer's own over `state` in `place`. Constant time.
    pub fn sealed(&self, place: Place, state: &[u8], seal: &[u8; 32]) -> bool {
        bool::from(self.seal(place, state).ct_eq(seal))
    }
}

impl Drop for SoftwareKey {
    fn drop(&mut self) {
        self.0.zeroize();
    }
}

impl Keys for SoftwareKey {
    fn vault_key(&mut self) -> Result<XOnlyKey> {
        let point = SoftwareArithmetic.base_mul(&self.0)?;
        let mut key = [0_u8; 32];
        key.copy_from_slice(&point[1..]);
        Ok(XOnlyKey(key))
    }

    /// BIP340 with all-zero auxiliary randomness, as the host's software signer signs.
    fn sign(&mut self, message: &[u8; 32]) -> Result<[u8; 64]> {
        let key = SigningKey::from_bytes(&self.0).map_err(|_| Error::SigningFailed)?;
        let signature = key
            .sign_raw(message, &[0; 32])
            .map_err(|_| Error::SigningFailed)?;
        Ok(signature.to_bytes())
    }

    fn sign_cooperative(
        &mut self,
        session: &CooperativeSession<'_>,
    ) -> Result<CooperativeSignature> {
        musig::sign_last(&mut SoftwareArithmetic, &self.0, session)
    }
}
