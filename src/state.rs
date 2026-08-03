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
    pub hard_limit_sats: u64,
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

pub fn initialize(data_dir: &Path, hard_limit_sats: u64) -> Result<InitializedVault> {
    if hard_limit_sats == 0 {
        bail!("hard limit must be greater than zero");
    }
    if data_dir.join(CONFIG_FILE).exists() {
        bail!("vault already initialized at {}", data_dir.display());
    }

    let secp = Secp256k1::new();
    let phone = DeviceKeys::generate(&secp)?;
    let hww = DeviceKeys::generate(&secp)?;
    let policy = VaultPolicy::new(phone.vault_pubkey, hww.vault_pubkey)?;
    let (hot_external, hot_internal) = phone.hot_descriptors(&secp)?;

    let phone_mnemonic = phone.mnemonic.to_string();
    let hww_mnemonic = hww.mnemonic.to_string();
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
        hard_limit_sats,
    };

    write_json(&data_dir.join(CONFIG_FILE), &config)?;
    write_json(
        &data_dir.join(PHONE_DEVICE_FILE),
        &DeviceFile {
            kind: "phone".to_owned(),
            mnemonic: phone_mnemonic.clone(),
        },
    )?;
    write_json(
        &data_dir.join(HWW_DEVICE_FILE),
        &DeviceFile {
            kind: "hww".to_owned(),
            mnemonic: hww_mnemonic.clone(),
        },
    )?;

    let backup = crypto::encrypt(&hww.seed, "phone-seed-backup", phone_mnemonic.as_bytes())?;
    write_json(&data_dir.join(PHONE_BACKUP_FILE), &backup)?;

    // Initialize the BDK SQLite state while the device file is present.
    crate::hot::HotWallet::open_or_create(data_dir)?;

    Ok(InitializedVault {
        config,
        phone_mnemonic,
        hww_mnemonic,
    })
}

pub fn load_config(data_dir: &Path) -> Result<VaultConfig> {
    read_json(&data_dir.join(CONFIG_FILE))
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
        let initialized = initialize(dir.path(), 10_000_000).unwrap();
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
        initialize(dir.path(), 10_000_000).unwrap();
        assert!(initialize(dir.path(), 10_000_000).is_err());
    }

    #[test]
    fn zero_hard_limit_is_rejected() {
        let dir = tempfile::tempdir().unwrap();
        assert!(initialize(dir.path(), 0).is_err());
    }
}
