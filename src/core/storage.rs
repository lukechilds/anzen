use super::{HWW_RECOVERY_BLOCKS, PHONE_RECOVERY_BLOCKS, keys::DeviceKeys, policy::VaultPolicy};
use anyhow::{Context, Result, bail};
use bitcoin::{Network, key::Secp256k1, secp256k1::XOnlyPublicKey};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use std::{
    fs,
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
}

impl VaultConfig {
    pub fn bitcoin_network(&self) -> Result<Network> {
        parse_network_name(&self.network)
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

pub fn initialize_vault(data_dir: &Path) -> Result<VaultConfig> {
    initialize_vault_for_network(data_dir, Network::Regtest)
}

pub fn initialize_vault_for_network(data_dir: &Path, network: Network) -> Result<VaultConfig> {
    validate_supported_network(network)?;
    if data_dir.join(CONFIG_FILE).exists() || data_dir.join(LEGACY_CONFIG_FILE).exists() {
        bail!("vault already initialized at {}", data_dir.display());
    }
    let secp = Secp256k1::new();
    let phone_file = load_device(data_dir, PHONE_DEVICE_FILE)
        .context("initialize the phone before the vault")?;
    let hww_file =
        load_device(data_dir, HWW_DEVICE_FILE).context("initialize the HWW before the vault")?;
    if phone_file.bitcoin_network()? != network || hww_file.bitcoin_network()? != network {
        bail!("phone, HWW, and vault must be initialized for the same network");
    }
    let phone = DeviceKeys::parse_for_network_at_index(
        &secp,
        &phone_file.mnemonic,
        network,
        phone_file.vault_key_index,
    )?;
    let hww = DeviceKeys::parse_for_network_at_index(
        &secp,
        &hww_file.mnemonic,
        network,
        hww_file.vault_key_index,
    )?;
    if data_dir.join(HWW_PUBLIC_FILE).exists() {
        let public_hww = load_public_device(data_dir, HWW_PUBLIC_FILE)?;
        if public_hww.bitcoin_network()? != network
            || public_hww.parsed_vault_pubkey()? != hww.vault_pubkey
        {
            bail!("HWW public metadata does not match the initialized HWW key");
        }
    }
    let policy = VaultPolicy::new_for_network(phone.vault_pubkey, hww.vault_pubkey, network)?;
    let (hot_external, hot_internal) = phone.hot_descriptors(&secp)?;
    let config = VaultConfig {
        version: 1,
        network: network_name(network).to_owned(),
        phone_vault_pubkey: phone.vault_pubkey.to_string(),
        hww_vault_pubkey: hww.vault_pubkey.to_string(),
        phone_hot_external_descriptor: hot_external,
        phone_hot_internal_descriptor: hot_internal,
        vault_descriptor: policy.descriptor_string(),
        vault_address: policy.address.to_string(),
        phone_recovery_blocks: PHONE_RECOVERY_BLOCKS,
        hww_recovery_blocks: HWW_RECOVERY_BLOCKS,
        monthly_limit_sats: 0,
    };
    write_json(&data_dir.join(CONFIG_FILE), &config)?;

    Ok(config)
}

pub fn load_config(data_dir: &Path) -> Result<VaultConfig> {
    read_json(&config_path(data_dir))
}

pub fn config_exists(data_dir: &Path) -> bool {
    data_dir.join(CONFIG_FILE).exists() || data_dir.join(LEGACY_CONFIG_FILE).exists()
}

pub fn set_monthly_limit(data_dir: &Path, monthly_limit_sats: u64) -> Result<VaultConfig> {
    let mut config = load_config(data_dir)?;
    config.monthly_limit_sats = monthly_limit_sats;
    write_json(&config_path(data_dir), &config)?;
    Ok(config)
}

fn config_path(data_dir: &Path) -> PathBuf {
    let current = data_dir.join(CONFIG_FILE);
    if current.exists() {
        return current;
    }

    let legacy = data_dir.join(LEGACY_CONFIG_FILE);
    if legacy.exists() {
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
        _ => "unsupported",
    }
}

pub fn parse_network_name(name: &str) -> Result<Network> {
    match name {
        "mainnet" | "bitcoin" => Ok(Network::Bitcoin),
        "regtest" => Ok(Network::Regtest),
        _ => bail!("unsupported vault network: {name}"),
    }
}

pub fn validate_supported_network(network: Network) -> Result<()> {
    match network {
        Network::Bitcoin | Network::Regtest => Ok(()),
        other => bail!("unsupported vault network: {other}"),
    }
}

fn default_network_name() -> String {
    "regtest".to_owned()
}

pub fn write_json<T: Serialize>(path: &Path, value: &T) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }
    let bytes = serde_json::to_vec_pretty(value)?;
    fs::write(path, bytes).with_context(|| format!("failed to write {}", path.display()))?;
    set_private_permissions(path)?;
    Ok(())
}

pub fn write_private(path: &Path, bytes: &[u8]) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }
    fs::write(path, bytes).with_context(|| format!("failed to write {}", path.display()))?;
    set_private_permissions(path)?;
    Ok(())
}

pub fn read_json<T: DeserializeOwned>(path: &Path) -> Result<T> {
    let bytes = fs::read(path).with_context(|| format!("failed to read {}", path.display()))?;
    serde_json::from_slice(&bytes).with_context(|| format!("invalid JSON in {}", path.display()))
}

#[cfg(unix)]
fn set_private_permissions(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o600))
        .with_context(|| format!("failed to set private permissions on {}", path.display()))
}

#[cfg(not(unix))]
fn set_private_permissions(_path: &Path) -> Result<()> {
    Ok(())
}

pub fn hot_db_path(data_dir: &Path) -> PathBuf {
    data_dir.join("phone/hot-wallet.sqlite")
}
