use crate::{
    HWW_RECOVERY_BLOCKS, PHONE_RECOVERY_BLOCKS,
    crypto::{self, EncryptedBlob},
    keys::DeviceKeys,
    policy::VaultPolicy,
};
use anyhow::{Context, Result, bail};
use bitcoin::key::Secp256k1;
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use std::{
    fs,
    path::{Path, PathBuf},
};

pub const CONFIG_FILE: &str = "vault.json";
pub const PHONE_DEVICE_FILE: &str = "phone/device.json";
pub const HWW_DEVICE_FILE: &str = "hww/device.json";
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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeviceFile {
    pub kind: String,
    pub mnemonic: String,
}

#[derive(Debug)]
pub struct InitializedVault {
    pub config: VaultConfig,
    pub phone_mnemonic: String,
    pub hww_mnemonic: String,
}

#[derive(Debug)]
pub struct InitializedDevice {
    pub mnemonic: String,
    pub vault_pubkey: String,
}

pub fn initialize_phone(data_dir: &Path) -> Result<InitializedDevice> {
    let phone_path = data_dir.join(PHONE_DEVICE_FILE);
    if phone_path.exists() {
        bail!("phone already initialized at {}", phone_path.display());
    }
    let secp = Secp256k1::new();
    let phone = DeviceKeys::generate(&secp)?;
    let phone_mnemonic = phone.mnemonic.to_string();
    write_json(
        &phone_path,
        &DeviceFile {
            kind: "phone".to_owned(),
            mnemonic: phone_mnemonic.clone(),
        },
    )?;
    Ok(InitializedDevice {
        mnemonic: phone_mnemonic,
        vault_pubkey: phone.vault_pubkey.to_string(),
    })
}

pub fn initialize_hww(data_dir: &Path) -> Result<InitializedDevice> {
    let hww_path = data_dir.join(HWW_DEVICE_FILE);
    if hww_path.exists() {
        bail!("HWW already initialized at {}", hww_path.display());
    }
    let secp = Secp256k1::new();
    let phone_file =
        load_device(data_dir, PHONE_DEVICE_FILE).context("initialize the phone before the HWW")?;
    let phone = DeviceKeys::parse(&secp, &phone_file.mnemonic)?;
    let hww = DeviceKeys::generate(&secp)?;
    let hww_mnemonic = hww.mnemonic.to_string();
    write_json(
        &hww_path,
        &DeviceFile {
            kind: "hww".to_owned(),
            mnemonic: hww_mnemonic.clone(),
        },
    )?;

    let backup = crypto::encrypt(
        &hww.seed,
        "phone-seed-backup",
        phone.mnemonic.to_string().as_bytes(),
    )?;
    write_json(&data_dir.join(PHONE_BACKUP_FILE), &backup)?;
    Ok(InitializedDevice {
        mnemonic: hww_mnemonic,
        vault_pubkey: hww.vault_pubkey.to_string(),
    })
}

pub fn initialize_vault(data_dir: &Path) -> Result<VaultConfig> {
    if data_dir.join(CONFIG_FILE).exists() {
        bail!("vault already initialized at {}", data_dir.display());
    }
    let secp = Secp256k1::new();
    let phone_file = load_device(data_dir, PHONE_DEVICE_FILE)
        .context("initialize the phone before the vault")?;
    let hww_file =
        load_device(data_dir, HWW_DEVICE_FILE).context("initialize the HWW before the vault")?;
    let phone = DeviceKeys::parse(&secp, &phone_file.mnemonic)?;
    let hww = DeviceKeys::parse(&secp, &hww_file.mnemonic)?;
    let policy = VaultPolicy::new(phone.vault_pubkey, hww.vault_pubkey)?;
    let (hot_external, hot_internal) = phone.hot_descriptors(&secp)?;
    let config = VaultConfig {
        version: 1,
        network: "regtest".to_owned(),
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

    // Initialize the BDK SQLite state while the device file is present.
    crate::hot::HotWallet::open_or_create(data_dir)?;

    Ok(config)
}

pub fn initialize(data_dir: &Path) -> Result<InitializedVault> {
    let phone = initialize_phone(data_dir)?;
    let hww = initialize_hww(data_dir)?;
    let config = initialize_vault(data_dir)?;

    Ok(InitializedVault {
        config,
        phone_mnemonic: phone.mnemonic,
        hww_mnemonic: hww.mnemonic,
    })
}

pub fn load_config(data_dir: &Path) -> Result<VaultConfig> {
    read_json(&data_dir.join(CONFIG_FILE))
}

pub fn set_monthly_limit(data_dir: &Path, monthly_limit_sats: u64) -> Result<VaultConfig> {
    let mut config = load_config(data_dir)?;
    config.monthly_limit_sats = monthly_limit_sats;
    write_json(&data_dir.join(CONFIG_FILE), &config)?;
    Ok(config)
}

pub fn load_device(data_dir: &Path, relative_path: &str) -> Result<DeviceFile> {
    read_json(&data_dir.join(relative_path))
}

pub fn recover_phone_mnemonic(data_dir: &Path) -> Result<String> {
    let secp = Secp256k1::new();
    let hww_file = load_device(data_dir, HWW_DEVICE_FILE)?;
    let hww = DeviceKeys::parse(&secp, &hww_file.mnemonic)?;
    let backup: EncryptedBlob = read_json(&data_dir.join(PHONE_BACKUP_FILE))?;
    let words = crypto::decrypt(&hww.seed, "phone-seed-backup", &backup)?;
    String::from_utf8(words.to_vec()).context("decrypted phone backup was not UTF-8")
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn initialization_persists_separate_devices_and_recoverable_backup() {
        let dir = tempfile::tempdir().unwrap();
        let initialized = initialize(dir.path()).unwrap();
        assert_eq!(
            load_config(dir.path()).unwrap().vault_address,
            initialized.config.vault_address
        );
        assert!(dir.path().join(PHONE_DEVICE_FILE).exists());
        assert!(dir.path().join(HWW_DEVICE_FILE).exists());
        assert!(dir.path().join(PHONE_BACKUP_FILE).exists());
        assert_eq!(
            recover_phone_mnemonic(dir.path()).unwrap(),
            initialized.phone_mnemonic
        );
    }

    #[test]
    fn initialization_refuses_to_overwrite_existing_vault() {
        let dir = tempfile::tempdir().unwrap();
        initialize(dir.path()).unwrap();
        assert!(initialize_vault(dir.path()).is_err());
    }

    #[test]
    fn device_and_vault_initialization_are_separate() {
        let dir = tempfile::tempdir().unwrap();
        let phone = initialize_phone(dir.path()).unwrap();
        assert!(!phone.mnemonic.is_empty());
        assert!(initialize_vault(dir.path()).is_err());
        let hww = initialize_hww(dir.path()).unwrap();
        assert!(!hww.mnemonic.is_empty());
        let config = initialize_vault(dir.path()).unwrap();
        assert_eq!(config.monthly_limit_sats, 0);
    }
}
