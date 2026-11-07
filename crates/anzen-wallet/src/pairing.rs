//! Initial pairing: build the vault configuration from this phone's key and a signer's public key.

use anyhow::{Context, Result, bail};
use anzen_common::{
    HWW_RECOVERY_BLOCKS, PHONE_RECOVERY_BLOCKS,
    keys::DeviceKeys,
    policy::VaultPolicy,
    storage::{
        CONFIG_FILE, HWW_PUBLIC_FILE, PHONE_DEVICE_FILE, PublicDeviceFile, VaultConfig,
        config_exists, load_device, network_name, validate_supported_network, write_json,
    },
};
use bitcoin::{Network, key::Secp256k1};
use std::path::Path;

/// Pair this phone with a signer from the signer's public identity alone. The signer
/// separately validates and accepts the resulting configuration.
pub fn initialize_vault_with_public_hww(
    data_dir: &Path,
    network: Network,
    hww: &PublicDeviceFile,
) -> Result<VaultConfig> {
    validate_supported_network(network)?;
    hww.validate_kind("hww-public-key")?;
    if config_exists(data_dir) {
        bail!("vault already initialized at {}", data_dir.display());
    }
    let phone_file = load_device(data_dir, PHONE_DEVICE_FILE)
        .context("initialize the phone before the vault")?;
    if phone_file.bitcoin_network()? != network || hww.bitcoin_network()? != network {
        bail!("phone, HWW, and vault must be initialized for the same network");
    }
    let secp = Secp256k1::new();
    let phone = DeviceKeys::parse_for_network_at_index(
        &secp,
        &phone_file.mnemonic,
        network,
        phone_file.vault_key_index,
    )?;
    let hww_key = hww.parsed_vault_pubkey()?;
    let policy = VaultPolicy::new_for_network(phone.vault_pubkey, hww_key, network)?;
    let (hot_external, hot_internal) = phone.hot_descriptors(&secp)?;
    let config = VaultConfig {
        version: 1,
        network: network_name(network).to_owned(),
        phone_vault_pubkey: phone.vault_pubkey.to_string(),
        hww_vault_pubkey: hww_key.to_string(),
        phone_hot_external_descriptor: hot_external,
        phone_hot_internal_descriptor: hot_internal,
        vault_descriptor: policy.descriptor_string(),
        vault_address: policy.address.to_string(),
        phone_recovery_blocks: PHONE_RECOVERY_BLOCKS,
        hww_recovery_blocks: HWW_RECOVERY_BLOCKS,
        monthly_limit_sats: 0,
        emergency_access_limit_sats: 0,
    };
    config.validate_public_policy()?;
    write_json(&data_dir.join(HWW_PUBLIC_FILE), hww)?;
    write_json(&data_dir.join(CONFIG_FILE), &config)?;

    Ok(config)
}
