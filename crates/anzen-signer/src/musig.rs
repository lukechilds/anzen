//! The signer's half of a joint MuSig2 (BIP327) signature, for hardware.
//!
//! [`sign_last`] is BIP327 `DeterministicSign` without auxiliary randomness, specialised to the
//! vault: two keys, one Taproot tweak, the signer last. It is written against [`Arithmetic`], so
//! a hardware wallet runs it on its secure element's own scalar and point operations. The host
//! implementations (`anzen_common::musig`, built on the `musig2` crate) must give byte-identical
//! answers, and this crate's tests check that they do.
//!
//! This is a second implementation of the same signing, kept only so hardware can use its own
//! arithmetic: phones, browsers and the software signer still use `anzen_common::musig`. Today
//! its one user is the Ledger app (`ledger-app/src/bolos.rs`). If every signer moved to this
//! code, or hardware could run the `musig2` crate, one of the two could go.
//!
//! See [`crate::Keys::sign_cooperative`] for why the nonce is derived, never stored.

use crate::CooperativeSession;
use anzen_protocol::{
    Error, Result,
    crypto::Crypto,
    musig::{plain_key, sorted_keys},
    wire::{CooperativeSignature, PartialSignature, PubNonce},
};

/// Scalar arithmetic modulo the secp256k1 group order `n`, and multiplication of the generator.
/// Scalars are 32 big-endian bytes. Operations on secrets must be constant time.
pub trait Arithmetic: Crypto {
    /// `a mod n`, for turning a hash into a scalar.
    fn reduce(&mut self, a: &[u8; 32]) -> Result<[u8; 32]>;
    /// `a + b mod n`, for scalars below `n`.
    fn add(&mut self, a: &[u8; 32], b: &[u8; 32]) -> Result<[u8; 32]>;
    /// `a · b mod n`, for scalars below `n`.
    fn mul(&mut self, a: &[u8; 32], b: &[u8; 32]) -> Result<[u8; 32]>;
    /// `k·G` as a compressed point, for a nonzero scalar below `n`.
    fn base_mul(&mut self, k: &[u8; 32]) -> Result<[u8; 33]>;
}

/// `n - 1`: multiplying by it negates a scalar.
const MINUS_ONE: [u8; 32] = [
    0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xfe,
    0xba, 0xae, 0xdc, 0xe6, 0xaf, 0x48, 0xa0, 0x3b, 0xbf, 0xd2, 0x5e, 0x8c, 0xd0, 0x36, 0x41, 0x40,
];
const ONE: [u8; 32] = {
    let mut one = [0; 32];
    one[31] = 1;
    one
};

fn tagged<A: Arithmetic + ?Sized>(math: &mut A, tag: &[u8], parts: &[&[u8]]) -> [u8; 32] {
    let tag = math.sha256(&[tag]);
    let mut all: alloc::vec::Vec<&[u8]> = alloc::vec::Vec::with_capacity(parts.len() + 2);
    all.push(&tag);
    all.push(&tag);
    all.extend_from_slice(parts);
    math.sha256(&all)
}

fn x(point: &[u8; 33]) -> [u8; 32] {
    let mut x = [0; 32];
    x.copy_from_slice(&point[1..]);
    x
}

fn odd(point: &[u8; 33]) -> bool {
    point[0] == 0x03
}

fn point(bytes: &[u8]) -> Result<[u8; 33]> {
    bytes.try_into().map_err(|_| Error::SigningFailed)
}

/// The signer's public nonce and partial signature for one joint vault signature, as the last
/// of the two signers. `secret` is the signer's vault key.
pub fn sign_last<A: Arithmetic + ?Sized>(
    math: &mut A,
    secret: &[u8; 32],
    session: &CooperativeSession<'_>,
) -> Result<CooperativeSignature> {
    // Each x-only vault key enters aggregation as its even-y point, so sign with that point's
    // secret.
    let own_point = math.base_mul(secret)?;
    if x(&own_point) != session.keys.hww.0 {
        return Err(Error::WrongVault);
    }
    let secret = if odd(&own_point) {
        math.mul(secret, &MINUS_ONE)?
    } else {
        *secret
    };
    let own = plain_key(&session.keys.hww);

    // KeyAgg: the first sorted key is weighted by a hash of the key list, the second by one.
    let [first, second] = sorted_keys(session.keys);
    if first == second {
        return Err(Error::InvalidKey);
    }
    let list = tagged(math, b"KeyAgg list", &[&first, &second]);
    let first_coefficient = tagged(math, b"KeyAgg coefficient", &[&list, &first]);
    let first_coefficient = math.reduce(&first_coefficient)?;
    let aggregate = math.mul_add(&first_coefficient, &first, &second)?;
    let coefficient = if own == first { first_coefficient } else { ONE };

    // The Taproot tweak: Q = lift_x(aggregate) + t·G. An odd aggregate is negated first, and an
    // odd Q is negated when signing, so the secret's sign follows both parities.
    let tweak = tagged(math, b"TapTweak", &[&x(&aggregate), session.merkle_root]);
    let output = math.taproot_tweak(&x(&aggregate), &tweak)?;
    let secret_for_output = if odd(&aggregate) != output.odd_y {
        math.mul(&secret, &MINUS_ONE)?
    } else {
        secret
    };

    // DeterministicSign's nonces: both depend on the phone's nonce, so they cannot be chosen
    // before it is fixed.
    let mut nonce = |index: u8| -> Result<[u8; 32]> {
        let digest = tagged(
            math,
            b"MuSig/deterministic/nonce",
            &[
                &secret,
                &session.phone_nonce.0,
                &output.output_key,
                &(session.message.len() as u64).to_be_bytes(),
                session.message,
                &[index],
            ],
        );
        let nonce = math.reduce(&digest)?;
        if nonce == [0; 32] {
            return Err(Error::SigningFailed);
        }
        Ok(nonce)
    };
    let (mut first_nonce, mut second_nonce) = (nonce(0)?, nonce(1)?);
    let own_nonce = [math.base_mul(&first_nonce)?, math.base_mul(&second_nonce)?];

    // NonceAgg with the phone's nonce, then the nonce coefficient and the final nonce
    // R = R1 + b·R2. A sum at infinity cannot be produced without knowing this signer's nonce;
    // refuse it rather than sign.
    let phone = [
        point(&session.phone_nonce.0[..33])?,
        point(&session.phone_nonce.0[33..])?,
    ];
    let aggregate_nonce = [
        math.mul_add(&ONE, &phone[0], &own_nonce[0])
            .map_err(|_| Error::SigningFailed)?,
        math.mul_add(&ONE, &phone[1], &own_nonce[1])
            .map_err(|_| Error::SigningFailed)?,
    ];
    let nonce_coefficient = tagged(
        math,
        b"MuSig/noncecoef",
        &[
            &aggregate_nonce[0],
            &aggregate_nonce[1],
            &output.output_key,
            session.message,
        ],
    );
    let nonce_coefficient = math.reduce(&nonce_coefficient)?;
    let final_nonce = math
        .mul_add(&nonce_coefficient, &aggregate_nonce[1], &aggregate_nonce[0])
        .map_err(|_| Error::SigningFailed)?;
    let challenge = tagged(
        math,
        b"BIP0340/challenge",
        &[&x(&final_nonce), &output.output_key, session.message],
    );
    let challenge = math.reduce(&challenge)?;
    if odd(&final_nonce) {
        first_nonce = math.mul(&first_nonce, &MINUS_ONE)?;
        second_nonce = math.mul(&second_nonce, &MINUS_ONE)?;
    }

    // s = k1 + b·k2 + e·a·d
    let weighted_nonce = math.mul(&nonce_coefficient, &second_nonce)?;
    let weighted_secret = math.mul(&coefficient, &secret_for_output)?;
    let weighted_secret = math.mul(&challenge, &weighted_secret)?;
    let partial = math.add(&first_nonce, &weighted_nonce)?;
    let partial = math.add(&partial, &weighted_secret)?;

    let mut public_nonce = [0; 66];
    public_nonce[..33].copy_from_slice(&own_nonce[0]);
    public_nonce[33..].copy_from_slice(&own_nonce[1]);
    Ok(CooperativeSignature {
        nonce: PubNonce(public_nonce),
        partial: PartialSignature(partial),
    })
}
