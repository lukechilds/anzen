//! The native Metal nonce kernel must agree with Miniscript, like the wgpu one.

use anzen_common::{keys::DeviceKeys, policy::VaultAddressTemplate, protocol::SoftwareCrypto};
use anzen_vanity::{MetalSearch, NONCE_LOCKTIMES, NonceInputs, SearchInputs};
use bitcoin::{
    Network,
    secp256k1::{Secp256k1, XOnlyPublicKey},
};
use std::str::FromStr;

fn library_output_key(
    internal: &XOnlyPublicKey,
    signer: &XOnlyPublicKey,
    locktime: u32,
) -> [u8; 32] {
    let descriptor = miniscript::Descriptor::<XOnlyPublicKey>::from_str(&format!(
        "tr({internal},and_v(v:after({locktime}),and_v(v:older(4259839),pk({signer}))))"
    ))
    .unwrap();
    descriptor.script_pubkey().as_bytes()[2..34]
        .try_into()
        .unwrap()
}

#[test]
fn metal_nonce_kernel_matches_miniscript() {
    let metal = match MetalSearch::open() {
        Ok(metal) => metal,
        Err(error) => {
            eprintln!("skipping: {error:#}");
            return;
        }
    };
    let secp = Secp256k1::new();
    let phone = DeviceKeys::generate_for_network(&secp, Network::Regtest).unwrap();
    let signer = DeviceKeys::generate_for_network(&secp, Network::Regtest).unwrap();
    let internal =
        anzen_common::policy::internal_key(phone.vault_pubkey, signer.vault_pubkey).unwrap();
    let inputs = NonceInputs {
        internal_key: internal.serialize(),
        signer_key: signer.vault_pubkey.serialize(),
        target_value: 0x5a,
        target_bits: 8,
    };
    for start in [
        NONCE_LOCKTIMES.start,
        1_234_567_890,
        NONCE_LOCKTIMES.end - 256,
    ] {
        let keys = metal.nonce_output_keys(&inputs, start, 256).unwrap();
        for (offset, key) in keys.into_iter().enumerate() {
            let locktime = start + offset as u32;
            assert_eq!(
                key,
                Some(library_output_key(
                    &internal,
                    &signer.vault_pubkey,
                    locktime
                )),
                "locktime {locktime}"
            );
        }
    }
    let result = metal
        .search_nonces(&inputs, NONCE_LOCKTIMES, |_| true)
        .unwrap();
    let locktime = result.index.expect("an 8-bit target matches quickly");
    assert_eq!(
        library_output_key(&internal, &signer.vault_pubkey, locktime)[0],
        0x5a
    );
}

#[test]
fn metal_key_kernel_matches_the_library() {
    use bitcoin::hashes::Hash;
    let metal = match MetalSearch::open() {
        Ok(metal) => metal,
        Err(error) => {
            eprintln!("skipping: {error:#}");
            return;
        }
    };
    let secp = Secp256k1::new();
    let phone = DeviceKeys::generate_for_network(&secp, Network::Regtest).unwrap();
    let signer = DeviceKeys::generate_for_network(&secp, Network::Regtest).unwrap();
    let merkle_root =
        anzen_common::policy::VaultPolicy::new(phone.vault_pubkey, signer.vault_pubkey)
            .unwrap()
            .leaf(anzen_common::policy::SpendPath::HwwRecovery)
            .unwrap()
            .leaf_hash
            .to_byte_array();
    let parent = phone.vault_parent_xpriv(&secp).unwrap();
    let parent_public = bitcoin::bip32::Xpub::from_priv(&secp, &parent);
    let inputs = SearchInputs {
        parent_public_key: parent_public.public_key.serialize(),
        chain_code: parent_public.chain_code.to_bytes(),
        signer_key: signer.vault_pubkey.serialize(),
        merkle_root,
        target_value: 0,
        target_bits: 8,
    };
    for start in [0, 1_000_000, anzen_vanity::MAX_INDEX - 255] {
        let keys = metal.output_keys(&inputs, start, 256).unwrap();
        for (offset, key) in keys.into_iter().enumerate() {
            let index = start + offset as u32;
            let (_, _, child) =
                DeviceKeys::derive_vault_key_from_parent(&secp, &parent, index).unwrap();
            let expected = VaultAddressTemplate::new(signer.vault_pubkey)
                .unwrap()
                .output_key(&mut SoftwareCrypto::new(), child)
                .unwrap();
            assert_eq!(key, Some(expected), "index {index}");
        }
    }
}
