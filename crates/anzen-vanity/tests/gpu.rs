//! The shader must agree with the library code for every candidate. Skipped where no GPU adapter
//! is available.

use anzen_common::{keys::DeviceKeys, policy::VaultAddressTemplate, protocol::SoftwareCrypto};
use anzen_vanity::{GpuSearch, SearchInputs};
use bitcoin::{Network, secp256k1::Secp256k1};

fn setup() -> Option<(GpuSearch, DeviceKeys, VaultAddressTemplate, SearchInputs)> {
    let gpu = match GpuSearch::open() {
        Ok(gpu) => gpu,
        Err(error) => {
            eprintln!("skipping: {error:#}");
            return None;
        }
    };
    let secp = Secp256k1::new();
    let phone = DeviceKeys::generate_for_network(&secp, Network::Regtest).unwrap();
    let signer = DeviceKeys::generate_for_network(&secp, Network::Regtest).unwrap();
    let template = VaultAddressTemplate::new(signer.vault_pubkey).unwrap();
    let parent = phone.vault_parent_xpriv(&secp).unwrap();
    let parent_public = bitcoin::bip32::Xpub::from_priv(&secp, &parent);
    let inputs = SearchInputs {
        parent_public_key: parent_public.public_key.serialize(),
        chain_code: parent_public.chain_code.to_bytes(),
        signer_key: template.hww_pubkey(),
        merkle_root: template.merkle_root(&mut SoftwareCrypto::new()),
        target_value: 0,
        target_bits: 8,
    };
    Some((gpu, phone, template, inputs))
}

fn cpu_output_key(phone: &DeviceKeys, template: &VaultAddressTemplate, index: u32) -> [u8; 32] {
    let secp = Secp256k1::new();
    let parent = phone.vault_parent_xpriv(&secp).unwrap();
    let (_, _, key) = DeviceKeys::derive_vault_key_from_parent(&secp, &parent, index).unwrap();
    template
        .output_key(&mut SoftwareCrypto::new(), key)
        .unwrap()
}

#[test]
fn shader_output_keys_match_the_library() {
    let Some((gpu, phone, template, inputs)) = setup() else {
        return;
    };
    for start in [0, 1_000_000, anzen_vanity::MAX_INDEX - 255] {
        let keys = gpu.output_keys_blocking(&inputs, start, 256).unwrap();
        assert_eq!(keys.len(), 256);
        for (offset, key) in keys.into_iter().enumerate() {
            let index = start + offset as u32;
            assert_eq!(
                key,
                Some(cpu_output_key(&phone, &template, index)),
                "index {index}"
            );
        }
    }
}

#[test]
fn search_finds_the_first_match_the_library_agrees_with() {
    let Some((gpu, phone, template, mut inputs)) = setup() else {
        return;
    };
    inputs.target_value = 0x5a;
    inputs.target_bits = 8;
    let result = gpu
        .search_blocking(&inputs, 0..anzen_vanity::MAX_INDEX + 1, |_| {})
        .unwrap();
    let index = result.index.expect("an 8-bit target matches quickly");
    assert_eq!(cpu_output_key(&phone, &template, index)[0], 0x5a);
    // The first batch covers every lower index, so no earlier index may match.
    for earlier in 0..index {
        assert_ne!(cpu_output_key(&phone, &template, earlier)[0], 0x5a);
    }
}
