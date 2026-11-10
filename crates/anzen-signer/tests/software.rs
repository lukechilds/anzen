//! The pure-Rust arithmetic and keys a memory-rich signer uses (`anzen_signer::software`) must
//! agree with the host's own implementations: same primitives, same joint signatures, and
//! single signatures Bitcoin accepts.

use anzen_protocol::{
    crypto::{Crypto, SoftwareCrypto},
    script::{VaultKeys, XOnlyKey},
    wire::PubNonce,
};
use anzen_signer::{
    CooperativeSession, Keys,
    software::{Place, SoftwareArithmetic, SoftwareKey},
};
use bitcoin::{
    key::{Keypair, Secp256k1},
    secp256k1::{
        Message, PublicKey, XOnlyPublicKey,
        rand::{RngCore, thread_rng},
        schnorr,
    },
};

fn random() -> [u8; 32] {
    let mut bytes = [0; 32];
    thread_rng().fill_bytes(&mut bytes);
    bytes
}

fn random_point() -> [u8; 33] {
    Keypair::new(&Secp256k1::new(), &mut thread_rng())
        .public_key()
        .serialize()
}

#[test]
fn primitives_match_the_host_implementation() {
    let (mut k256, mut host) = (SoftwareArithmetic, SoftwareCrypto::new());
    for _ in 0..200 {
        let (a, first, second) = (random(), random_point(), random_point());
        // Scalars above the group order and points off the curve are refused by both.
        assert_eq!(
            k256.mul_add(&a, &first, &second).ok(),
            host.mul_add(&a, &first, &second).ok()
        );
        let (key, tweak) = (random(), random());
        assert_eq!(k256.is_valid_xonly_key(&key), host.is_valid_xonly_key(&key));
        assert_eq!(
            k256.taproot_tweak(&key, &tweak).ok(),
            host.taproot_tweak(&key, &tweak).ok()
        );
        let parts: [&[u8]; 3] = [&a, &first, &tweak];
        assert_eq!(k256.sha256(&parts), host.sha256(&parts));
    }
    // An x coordinate above the field prime is no key.
    assert!(!k256.is_valid_xonly_key(&[0xff; 32]));
    assert!(k256.taproot_tweak(&[0xff; 32], &random()).is_err());
    // A tweak at or above the group order is refused.
    let valid = XOnlyPublicKey::from_keypair(&Keypair::new(&Secp256k1::new(), &mut thread_rng())).0;
    assert!(k256.taproot_tweak(&valid.serialize(), &[0xff; 32]).is_err());
    // P + (-P) has no compressed form.
    let point = random_point();
    let mut negated = point;
    negated[0] ^= 1;
    let mut one = [0; 32];
    one[31] = 1;
    assert!(k256.mul_add(&one, &point, &negated).is_err());
}

#[test]
fn software_keys_sign_as_the_host_signer_does() {
    let secp = Secp256k1::new();
    for _ in 0..100 {
        let (phone, signer) = (
            Keypair::new(&secp, &mut thread_rng()),
            Keypair::new(&secp, &mut thread_rng()),
        );
        let mut key = SoftwareKey::new(signer.secret_bytes()).unwrap();
        let hww = XOnlyKey(signer.x_only_public_key().0.serialize());
        assert_eq!(key.vault_key().unwrap(), hww);

        // Single signatures: identical to libsecp256k1's without auxiliary randomness.
        let message = random();
        let signature = key.sign(&message).unwrap();
        assert_eq!(
            signature,
            secp.sign_schnorr_no_aux_rand(&Message::from_digest(message), &signer)
                .serialize()
        );
        secp.verify_schnorr(
            &schnorr::Signature::from_slice(&signature).unwrap(),
            &Message::from_digest(message),
            &signer.x_only_public_key().0,
        )
        .unwrap();

        // Joint signatures: byte for byte what the host's MuSig2 produces.
        let keys = VaultKeys {
            phone: XOnlyKey(phone.x_only_public_key().0.serialize()),
            hww,
            recovery_locktime: 500_000_000,
        };
        let merkle_root = random();
        let mut phone_nonce = [0; 66];
        phone_nonce[..33].copy_from_slice(&random_point());
        phone_nonce[33..].copy_from_slice(&random_point());
        let phone_nonce = PubNonce(phone_nonce);
        let session = CooperativeSession {
            keys: &keys,
            merkle_root: &merkle_root,
            message: &message,
            phone_nonce: &phone_nonce,
        };
        assert_eq!(
            key.sign_cooperative(&session).unwrap(),
            anzen_common::musig::sign_last(
                &signer.secret_bytes(),
                &keys,
                &merkle_root,
                &message,
                &phone_nonce
            )
            .unwrap()
        );
    }
}

#[test]
fn zero_and_out_of_range_secrets_are_not_keys() {
    assert!(SoftwareKey::new([0; 32]).is_err());
    assert!(SoftwareKey::new([0xff; 32]).is_err());
    let _ = PublicKey::from_slice(&random_point()).unwrap();
}

#[test]
fn a_seal_is_the_keys_own_and_covers_the_state_and_its_place() {
    let (key, other) = (
        SoftwareKey::new([7; 32]).unwrap(),
        SoftwareKey::new([8; 32]).unwrap(),
    );
    let state = random();
    let seal = key.seal(Place::Vault, &state);
    assert!(key.sealed(Place::Vault, &state, &seal));
    // The same key seals the same state the same way: a seal is no secret nonce.
    assert_eq!(seal, key.seal(Place::Vault, &state));

    // Another key, another place, another state or another seal: not this signer's.
    assert!(!other.sealed(Place::Vault, &state, &seal));
    assert!(!key.sealed(Place::Rotated, &state, &seal));
    assert_ne!(seal, key.seal(Place::Rotated, &state));
    for index in 0..state.len() {
        let mut altered = state;
        altered[index] ^= 1;
        assert!(!key.sealed(Place::Vault, &altered, &seal));
    }
    for index in 0..seal.len() {
        let mut altered = seal;
        altered[index] ^= 0x80;
        assert!(!key.sealed(Place::Vault, &state, &altered));
    }
    // Longer or shorter state, same prefix.
    assert!(!key.sealed(Place::Vault, &state[..31], &seal));
    let mut longer = state.to_vec();
    longer.push(0);
    assert!(!key.sealed(Place::Vault, &longer, &seal));
    assert!(key.sealed(Place::Vault, &[], &key.seal(Place::Vault, &[])));
}

#[test]
fn a_seal_is_hmac_sha256_under_a_key_derived_from_the_vault_key() {
    use bitcoin::hashes::{Hash, HashEngine, Hmac, HmacEngine, sha256};
    let secret = [7_u8; 32];
    let state = b"what the signer would have stored";
    let mut derived = sha256::Hash::engine();
    derived.input(b"Anzen/sealed-state/v1");
    derived.input(&secret);
    let derived = sha256::Hash::from_engine(derived);
    let mut mac = HmacEngine::<sha256::Hash>::new(derived.as_byte_array());
    mac.input(&[Place::Rotated as u8]);
    mac.input(state);
    assert_eq!(
        SoftwareKey::new(secret)
            .unwrap()
            .seal(Place::Rotated, state),
        Hmac::<sha256::Hash>::from_engine(mac).to_byte_array()
    );
}
