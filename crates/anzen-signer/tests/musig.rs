//! The hardware MuSig2 code must answer exactly as the host implementation does: both are BIP327
//! `DeterministicSign`, so the same request has one right answer.

use anzen_protocol::{
    Result,
    crypto::{Crypto, SoftwareCrypto, TweakedKey},
    script::{VaultKeys, XOnlyKey},
    wire::PubNonce,
};
use anzen_signer::{
    CooperativeSession,
    musig::{Arithmetic, sign_last},
};
use bitcoin::{
    key::{Keypair, Secp256k1},
    secp256k1::rand::{RngCore, thread_rng},
};
use musig2::secp::{MaybeScalar, Scalar};

/// Software arithmetic, standing in for a secure element's.
#[derive(Default)]
struct Software(SoftwareCrypto);

impl Crypto for Software {
    fn sha256(&mut self, parts: &[&[u8]]) -> [u8; 32] {
        self.0.sha256(parts)
    }
    fn is_valid_xonly_key(&mut self, key: &[u8; 32]) -> bool {
        self.0.is_valid_xonly_key(key)
    }
    fn taproot_tweak(&mut self, internal_key: &[u8; 32], tweak: &[u8; 32]) -> Result<TweakedKey> {
        self.0.taproot_tweak(internal_key, tweak)
    }
    fn mul_add(&mut self, a: &[u8; 32], first: &[u8; 33], second: &[u8; 33]) -> Result<[u8; 33]> {
        self.0.mul_add(a, first, second)
    }
}

fn scalar(bytes: &[u8; 32]) -> MaybeScalar {
    MaybeScalar::try_from(bytes.as_slice()).expect("below the group order")
}

impl Arithmetic for Software {
    fn reduce(&mut self, a: &[u8; 32]) -> Result<[u8; 32]> {
        Ok(MaybeScalar::reduce_from(a).serialize())
    }
    fn add(&mut self, a: &[u8; 32], b: &[u8; 32]) -> Result<[u8; 32]> {
        Ok((scalar(a) + scalar(b)).serialize())
    }
    fn mul(&mut self, a: &[u8; 32], b: &[u8; 32]) -> Result<[u8; 32]> {
        Ok((scalar(a) * scalar(b)).serialize())
    }
    fn base_mul(&mut self, k: &[u8; 32]) -> Result<[u8; 33]> {
        Ok(Scalar::try_from(k.as_slice())
            .map_err(|_| anzen_protocol::Error::SigningFailed)?
            .base_point_mul()
            .serialize())
    }
}

fn random() -> [u8; 32] {
    let mut bytes = [0; 32];
    thread_rng().fill_bytes(&mut bytes);
    bytes
}

fn random_nonce() -> PubNonce {
    let secp = Secp256k1::new();
    let mut nonce = [0; 66];
    for half in nonce.chunks_exact_mut(33) {
        half.copy_from_slice(
            &Keypair::new(&secp, &mut thread_rng())
                .public_key()
                .serialize(),
        );
    }
    PubNonce(nonce)
}

#[test]
fn hardware_musig_answers_exactly_as_the_host_implementation() {
    let secp = Secp256k1::new();
    // Enough draws to cover every parity of the signer key, the aggregate, the tweaked output
    // and the final nonce, and both key orders.
    let (mut odd_keys, mut signer_first) = (0, 0);
    for _ in 0..200 {
        let (phone, signer) = (
            Keypair::new(&secp, &mut thread_rng()),
            Keypair::new(&secp, &mut thread_rng()),
        );
        let keys = VaultKeys {
            phone: XOnlyKey(phone.x_only_public_key().0.serialize()),
            hww: XOnlyKey(signer.x_only_public_key().0.serialize()),
            recovery_locktime: 500_000_000,
        };
        odd_keys += usize::from(signer.public_key().serialize()[0] == 0x03);
        signer_first += usize::from(keys.hww < keys.phone);
        let (merkle_root, message, phone_nonce) = (random(), random(), random_nonce());
        let session = CooperativeSession {
            keys: &keys,
            merkle_root: &merkle_root,
            message: &message,
            phone_nonce: &phone_nonce,
        };
        let hardware =
            sign_last(&mut Software::default(), &signer.secret_bytes(), &session).unwrap();
        let host = anzen_common::musig::sign_last(
            &signer.secret_bytes(),
            &keys,
            &merkle_root,
            &message,
            &phone_nonce,
        )
        .unwrap();
        assert_eq!(hardware, host);
        // The same request repeats the same answer; a fresh phone nonce changes the nonce.
        assert_eq!(
            sign_last(&mut Software::default(), &signer.secret_bytes(), &session).unwrap(),
            hardware
        );
        let retry = random_nonce();
        let retried = sign_last(
            &mut Software::default(),
            &signer.secret_bytes(),
            &CooperativeSession {
                phone_nonce: &retry,
                ..session
            },
        )
        .unwrap();
        assert_ne!(retried.nonce, hardware.nonce);
    }
    assert!(odd_keys > 20 && odd_keys < 180 && signer_first > 20 && signer_first < 180);
}

#[test]
fn hardware_musig_refuses_another_vault_and_invalid_nonces() {
    let secp = Secp256k1::new();
    let (phone, signer, other) = (
        Keypair::new(&secp, &mut thread_rng()),
        Keypair::new(&secp, &mut thread_rng()),
        Keypair::new(&secp, &mut thread_rng()),
    );
    let keys = VaultKeys {
        phone: XOnlyKey(phone.x_only_public_key().0.serialize()),
        hww: XOnlyKey(signer.x_only_public_key().0.serialize()),
        recovery_locktime: 500_000_000,
    };
    let (merkle_root, message, phone_nonce) = (random(), random(), random_nonce());
    let session = CooperativeSession {
        keys: &keys,
        merkle_root: &merkle_root,
        message: &message,
        phone_nonce: &phone_nonce,
    };
    assert!(sign_last(&mut Software::default(), &other.secret_bytes(), &session).is_err());
    // An x coordinate above the field prime is no point.
    let mut not_on_curve = phone_nonce;
    not_on_curve.0[1..33].copy_from_slice(&[0xff; 32]);
    assert!(
        sign_last(
            &mut Software::default(),
            &signer.secret_bytes(),
            &CooperativeSession {
                phone_nonce: &not_on_curve,
                ..session
            },
        )
        .is_err()
    );
    let mut same = keys;
    same.phone = same.hww;
    assert!(
        sign_last(
            &mut Software::default(),
            &signer.secret_bytes(),
            &CooperativeSession {
                keys: &same,
                ..session
            }
        )
        .is_err()
    );
}
