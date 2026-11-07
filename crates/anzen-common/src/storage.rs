//! Persisted device and vault records, plus the storage boundary every application writes through.

use super::{HWW_RECOVERY_BLOCKS, PHONE_RECOVERY_BLOCKS, keys::DeviceKeys, policy::VaultPolicy};
use anyhow::{Context, Result, bail};
#[cfg(target_arch = "wasm32")]
pub use anzen_storage::browser;
pub use anzen_storage::{
    fs, read_json, tempdir, with_protected_storage, write_json, write_private,
};
use bitcoin::{Network, key::Secp256k1, secp256k1::XOnlyPublicKey};
use fs::StoragePath;
use serde::{Deserialize, Serialize};
use std::{
    path::{Path, PathBuf},
    str::FromStr,
};

pub const CONFIG_FILE: &str = "anzen.json";
const LEGACY_CONFIG_FILE: &str = "vault.json";
pub const PHONE_DEVICE_FILE: &str = "phone/device.json";
pub const HWW_DEVICE_FILE: &str = "hww/device.json";
pub const HWW_PUBLIC_FILE: &str = "hww/public.json";
pub const PHONE_BACKUP_FILE: &str = "cloud/phone-seed-backup.json";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VaultConfig {
    pub version: u8,
    pub network: String,
    pub phone_vault_pubkey: String,
    pub hww_vault_pubkey: String,
    pub phone_hot_external_descriptor: String,
    pub phone_hot_internal_descriptor: String,
    pub vault_descriptor: String,
    pub vault_address: String,
    pub phone_recovery_blocks: u16,
    pub hww_recovery_blocks: u16,
    #[serde(default, alias = "hard_limit_sats")]
    pub monthly_limit_sats: u64,
    #[serde(default)]
    pub emergency_access_limit_sats: u64,
}

impl VaultConfig {
    pub fn bitcoin_network(&self) -> Result<Network> {
        parse_network_name(&self.network)
    }

    /// Reconstruct the fixed policy instead of trusting descriptor/address metadata from a peer.
    pub fn validate_public_policy(&self) -> Result<()> {
        if self.version != 1 {
            bail!("unsupported vault configuration version");
        }
        let network = self.bitcoin_network()?;
        validate_supported_network(network)?;
        let phone = XOnlyPublicKey::from_str(&self.phone_vault_pubkey)?;
        let hww = XOnlyPublicKey::from_str(&self.hww_vault_pubkey)?;
        if phone == hww {
            bail!("phone and hardware wallet must use independent keys");
        }
        let policy = VaultPolicy::new_for_network(phone, hww, network)?;
        if self.phone_recovery_blocks != PHONE_RECOVERY_BLOCKS
            || self.hww_recovery_blocks != HWW_RECOVERY_BLOCKS
            || self.vault_descriptor != policy.descriptor_string()
            || self.vault_address != policy.address.to_string()
        {
            bail!("vault configuration does not match the fixed Anzen policy");
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeviceFile {
    pub kind: String,
    #[serde(default = "default_network_name")]
    pub network: String,
    pub mnemonic: String,
    #[serde(default)]
    pub vault_key_index: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PublicDeviceFile {
    pub version: u8,
    pub kind: String,
    pub network: String,
    pub vault_pubkey: String,
}

impl PublicDeviceFile {
    pub fn bitcoin_network(&self) -> Result<Network> {
        parse_network_name(&self.network)
    }

    pub fn parsed_vault_pubkey(&self) -> Result<XOnlyPublicKey> {
        XOnlyPublicKey::from_str(&self.vault_pubkey).context("invalid public device vault key")
    }

    pub fn validate_kind(&self, expected: &str) -> Result<()> {
        if self.version != 1 || self.kind != expected {
            bail!("unsupported public device identity");
        }
        validate_supported_network(self.bitcoin_network()?)?;
        self.parsed_vault_pubkey()?;
        Ok(())
    }
}

impl DeviceFile {
    pub fn bitcoin_network(&self) -> Result<Network> {
        parse_network_name(&self.network)
    }
}

#[derive(Debug)]
pub struct InitializedDevice {
    pub mnemonic: String,
    pub vault_pubkey: String,
    pub vault_key_index: u32,
}

pub fn load_config(data_dir: &Path) -> Result<VaultConfig> {
    read_json(&config_path(data_dir))
}

pub fn config_exists(data_dir: &Path) -> bool {
    data_dir.join(CONFIG_FILE).storage_exists()
        || data_dir.join(LEGACY_CONFIG_FILE).storage_exists()
}

pub fn set_policy_limits(
    data_dir: &Path,
    monthly_limit_sats: u64,
    emergency_access_limit_sats: u64,
) -> Result<VaultConfig> {
    let mut config = load_config(data_dir)?;
    config.monthly_limit_sats = monthly_limit_sats;
    config.emergency_access_limit_sats = emergency_access_limit_sats;
    write_json(&config_path(data_dir), &config)?;
    Ok(config)
}

fn config_path(data_dir: &Path) -> PathBuf {
    let current = data_dir.join(CONFIG_FILE);
    if current.storage_exists() {
        return current;
    }

    let legacy = data_dir.join(LEGACY_CONFIG_FILE);
    if legacy.storage_exists() {
        return legacy;
    }

    current
}

pub fn load_device(data_dir: &Path, relative_path: &str) -> Result<DeviceFile> {
    read_json(&data_dir.join(relative_path))
}

pub fn load_public_device(data_dir: &Path, relative_path: &str) -> Result<PublicDeviceFile> {
    read_json(&data_dir.join(relative_path))
}

pub fn load_device_keys(data_dir: &Path, relative_path: &str) -> Result<DeviceKeys> {
    let file = load_device(data_dir, relative_path)?;
    DeviceKeys::parse_for_network_at_index(
        &Secp256k1::new(),
        &file.mnemonic,
        file.bitcoin_network()?,
        file.vault_key_index,
    )
}

pub fn network_name(network: Network) -> &'static str {
    match network {
        Network::Bitcoin => "mainnet",
        Network::Regtest => "regtest",
        Network::Signet => "mutinynet",
        _ => "unsupported",
    }
}

pub fn parse_network_name(name: &str) -> Result<Network> {
    match name {
        "mainnet" | "bitcoin" => Ok(Network::Bitcoin),
        "regtest" => Ok(Network::Regtest),
        "mutinynet" => Ok(Network::Signet),
        _ => bail!("unsupported vault network: {name}"),
    }
}

pub fn validate_supported_network(network: Network) -> Result<()> {
    match network {
        Network::Bitcoin | Network::Regtest | Network::Signet => Ok(()),
        other => bail!("unsupported vault network: {other}"),
    }
}

fn default_network_name() -> String {
    "regtest".to_owned()
}
