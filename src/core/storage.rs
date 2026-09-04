use super::{HWW_RECOVERY_BLOCKS, PHONE_RECOVERY_BLOCKS, keys::DeviceKeys, policy::VaultPolicy};
use anyhow::{Context, Result, bail};
use bitcoin::{Network, key::Secp256k1, secp256k1::XOnlyPublicKey};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use std::{
    fs,
    io::Write,
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
        emergency_access_limit_sats: 0,
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
    let emergency_access_limit_sats = load_config(data_dir)?.emergency_access_limit_sats;
    set_policy_limits(data_dir, monthly_limit_sats, emergency_access_limit_sats)
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
    write_private(path, &serde_json::to_vec_pretty(value)?)
}

/// Replace a file atomically on the same filesystem. The temporary file is private from
/// creation, and is flushed before the rename so interrupted writes cannot truncate live keys
/// or the active schedule. Persist the directory entry as well on Unix.
pub fn write_private(path: &Path, bytes: &[u8]) -> Result<()> {
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent).with_context(|| format!("failed to create {}", parent.display()))?;
    let mut temporary = tempfile::NamedTempFile::new_in(parent)?;
    temporary
        .write_all(bytes)
        .with_context(|| format!("failed to write {}", path.display()))?;
    temporary.as_file().sync_all()?;
    temporary
        .persist(path)
        .with_context(|| format!("failed to replace {}", path.display()))?;
    #[cfg(unix)]
    fs::File::open(parent)?.sync_all()?;
    Ok(())
}

pub fn read_json<T: DeserializeOwned>(path: &Path) -> Result<T> {
    let bytes = fs::read(path).with_context(|| format!("failed to read {}", path.display()))?;
    serde_json::from_slice(&bytes).with_context(|| format!("invalid JSON in {}", path.display()))
}

pub fn hot_db_path(data_dir: &Path) -> PathBuf {
    data_dir.join("phone/hot-wallet.sqlite")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn private_files_are_atomically_replaced_and_remain_private() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.json");
        write_json(&path, &vec![1_u64; 100]).unwrap();
        let old_handle = fs::File::open(&path).unwrap();
        write_json(&path, &vec![2_u64; 100]).unwrap();
        assert_eq!(read_json::<Vec<u64>>(&path).unwrap(), vec![2; 100]);
        // An open reader still sees the complete old inode, not truncated/partially written JSON.
        assert_eq!(
            serde_json::from_reader::<_, Vec<u64>>(old_handle).unwrap(),
            vec![1; 100]
        );
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 1);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
    }

    #[test]
    fn failed_replacement_preserves_destination_and_cleans_temporary_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("not-a-file");
        fs::create_dir(&path).unwrap();
        write_private(&path.join("retained"), b"unchanged").unwrap();
        assert!(write_private(&path, b"replacement").is_err());
        assert_eq!(fs::read(path.join("retained")).unwrap(), b"unchanged");
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 1);
    }
}
