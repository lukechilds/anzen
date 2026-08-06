//! Minimal hardware-wallet-facing policy validation and signing implementation.
//!
//! Keep this module deliberately small and readable. It may depend on `crate::core`, but never on
//! `crate::hot_wallet`.

use crate::core::{
    ceremony::{self, BatchManifest},
    crypto::{self, EncryptedBlob},
    keys::DeviceKeys,
    policy::SpendPath,
    recovery::{
        self, CooperativeSweepPackage, PhoneRecoveryPackage, PhoneRotationPackage, SweepPath,
        SweepResult,
    },
    social::{self, CloudRecoveryBackup, RecoveryPayload},
    storage::{
        DeviceFile, HWW_DEVICE_FILE, InitializedDevice, PHONE_BACKUP_FILE, PHONE_DEVICE_FILE,
        VaultConfig, load_config, load_device, load_device_keys, network_name, read_json,
        validate_supported_network, write_json,
    },
    transactions::{sign_vault_psbt, verify_vault_psbt_signature},
    types::VaultUtxo,
};
use anyhow::{Context, Result};
use bitcoin::{Address, Network, Transaction, key::Secp256k1};
use std::path::Path;
use std::str::FromStr;

/// Initialize the simulated HWW. The descriptor-bound cloud backup is created after `anzen init`.
pub fn initialize(data_dir: &Path, network: Network) -> Result<InitializedDevice> {
    validate_supported_network(network)?;
    let hww_path = data_dir.join(HWW_DEVICE_FILE);
    if hww_path.exists() {
        anyhow::bail!("HWW already initialized at {}", hww_path.display());
    }
    let secp = Secp256k1::new();
    let phone_file =
        load_device(data_dir, PHONE_DEVICE_FILE).context("initialize the phone before the HWW")?;
    if phone_file.bitcoin_network()? != network {
        anyhow::bail!(
            "phone is configured for {}; initialize the HWW for the same network",
            phone_file.network
        );
    }
    DeviceKeys::parse_for_network(&secp, &phone_file.mnemonic, network)?;
    let hww = DeviceKeys::generate_for_network(&secp, network)?;
    let mnemonic = hww.mnemonic.to_string();
    write_json(
        &hww_path,
        &DeviceFile {
            kind: "hww".to_owned(),
            network: network_name(network).to_owned(),
            mnemonic: mnemonic.clone(),
        },
    )?;
    Ok(InitializedDevice {
        mnemonic,
        vault_pubkey: hww.vault_pubkey.to_string(),
    })
}

pub fn decrypt_phone_backup(data_dir: &Path) -> Result<String> {
    let config = load_config(data_dir)?;
    let hww = load_device_keys(data_dir, HWW_DEVICE_FILE)?;
    let backup =
        load_or_migrate_cloud_backup(data_dir, &data_dir.join(PHONE_BACKUP_FILE), &config, &hww)?;
    let payload = social::decrypt_with_hww(&backup, &hww.seed)?;
    payload.validate_against(&config)?;
    Ok(payload.phone_mnemonic)
}

/// Create a descriptor-bound cloud backup after the vault configuration exists.
pub fn create_cloud_recovery_backup(
    data_dir: &Path,
    config: &VaultConfig,
) -> Result<CloudRecoveryBackup> {
    let phone = load_device_keys(data_dir, PHONE_DEVICE_FILE)?;
    let hww = load_device_keys(data_dir, HWW_DEVICE_FILE)?;
    let backup = social::create_backup(&RecoveryPayload::new(config, &phone)?, &hww.seed, &[])?;
    write_json(&data_dir.join(PHONE_BACKUP_FILE), &backup)?;
    Ok(backup)
}

/// Add a 1-of-N OpenPGP recovery friend by wrapping the existing symmetric backup key.
pub fn add_recovery_friend(data_dir: &Path, public_key: &[u8]) -> Result<String> {
    let config = load_config(data_dir)?;
    let hww = load_device_keys(data_dir, HWW_DEVICE_FILE)?;
    let mut backup =
        load_or_migrate_cloud_backup(data_dir, &data_dir.join(PHONE_BACKUP_FILE), &config, &hww)?;
    social::decrypt_with_hww(&backup, &hww.seed)?.validate_against(&config)?;
    let fingerprint = social::add_friend(&mut backup, &hww.seed, public_key)?;
    write_json(&data_dir.join(PHONE_BACKUP_FILE), &backup)?;
    Ok(fingerprint)
}

/// Validate every transaction against the displayed policy, verify the phone signatures, and add
/// the HWW signatures.
pub fn approve_policy(data_dir: &Path, batch_dir: &Path) -> Result<BatchManifest> {
    let config = load_config(data_dir)?;
    approve_policy_for_config(data_dir, &config, batch_dir)
}

fn approve_policy_for_config(
    data_dir: &Path,
    config: &VaultConfig,
    batch_dir: &Path,
) -> Result<BatchManifest> {
    let mut manifest = ceremony::load_manifest(batch_dir)?;
    let policy = ceremony::validate_batch(config, &manifest, batch_dir)?;
    let phone_pubkey = bitcoin::secp256k1::XOnlyPublicKey::from_str(&config.phone_vault_pubkey)?;
    let hww = load_device_keys(data_dir, HWW_DEVICE_FILE)?;
    if hww.vault_pubkey.to_string() != config.hww_vault_pubkey {
        anyhow::bail!("HWW key does not match the configured vault policy");
    }

    for transaction in ceremony::manifest_transactions(&manifest) {
        let path = batch_dir.join(&transaction.psbt_file);
        let mut psbt = ceremony::read_psbt(&path)?;
        verify_vault_psbt_signature(&psbt, &policy, SpendPath::Cooperative, phone_pubkey)?;
        sign_vault_psbt(
            &mut psbt,
            &policy,
            SpendPath::Cooperative,
            &hww.vault_keypair,
        )?;
        ceremony::write_psbt(&path, &psbt)?;
    }
    manifest.hww_approved = true;
    write_json(&batch_dir.join("manifest.json"), &manifest)?;
    Ok(manifest)
}

pub fn decrypt_phone_backup_package(
    data_dir: &Path,
    backup_path: &Path,
) -> Result<PhoneRecoveryPackage> {
    let config = load_config(data_dir)?;
    let hww = load_device_keys(data_dir, HWW_DEVICE_FILE)?;
    let backup = load_or_migrate_cloud_backup(data_dir, backup_path, &config, &hww)?;
    let payload = social::decrypt_with_hww(&backup, &hww.seed)?;
    payload.validate_against(&config)?;
    let words = payload.phone_mnemonic;
    let phone =
        DeviceKeys::parse_for_network(&Secp256k1::new(), &words, config.bitcoin_network()?)?;
    if phone.vault_pubkey.to_string() != config.phone_vault_pubkey {
        anyhow::bail!("decrypted phone backup does not match the configured vault policy");
    }
    Ok(PhoneRecoveryPackage {
        version: 2,
        kind: "phone-recovery".to_owned(),
        phone_mnemonic: words,
        phone_vault_pubkey: phone.vault_pubkey.to_string(),
        vault_descriptor: payload.vault_descriptor,
        vault_address: payload.vault_address,
    })
}

pub fn recover(
    data_dir: &Path,
    config: &VaultConfig,
    utxos: &[VaultUtxo],
    tip_height: u64,
    destination: &Address,
) -> Result<(Transaction, SweepResult)> {
    let plan = recovery::prepare_sweep(
        config,
        utxos,
        tip_height,
        SweepPath::HwwRecovery,
        destination,
    )?;
    let hww = load_device_keys(data_dir, HWW_DEVICE_FILE)?;
    recovery::sign_recovery_sweep(plan, SweepPath::HwwRecovery, &hww)
}

pub fn approve_cooperative_sweep(
    data_dir: &Path,
    package: &CooperativeSweepPackage,
) -> Result<CooperativeSweepPackage> {
    let config = load_config(data_dir)?;
    approve_cooperative_sweep_for_config(data_dir, &config, package)
}

fn approve_cooperative_sweep_for_config(
    data_dir: &Path,
    config: &VaultConfig,
    package: &CooperativeSweepPackage,
) -> Result<CooperativeSweepPackage> {
    let (policy, mut psbt) = recovery::validate_cooperative_sweep(config, package)?;
    let phone_pubkey = bitcoin::secp256k1::XOnlyPublicKey::from_str(&config.phone_vault_pubkey)?;
    verify_vault_psbt_signature(&psbt, &policy, SpendPath::Cooperative, phone_pubkey)?;
    let hww = load_device_keys(data_dir, HWW_DEVICE_FILE)?;
    if hww.vault_pubkey.to_string() != config.hww_vault_pubkey {
        anyhow::bail!("HWW key does not match the configured vault policy");
    }
    sign_vault_psbt(
        &mut psbt,
        &policy,
        SpendPath::Cooperative,
        &hww.vault_keypair,
    )?;
    let mut approved = package.clone();
    approved.psbt = psbt.to_string();
    approved.hww_approved = true;
    Ok(approved)
}

pub fn approve_phone_rotation(
    data_dir: &Path,
    package: &PhoneRotationPackage,
) -> Result<PhoneRotationPackage> {
    let (old_config, new_config, new_phone) = recovery::validate_phone_rotation(data_dir, package)?;
    let mut approved = package.clone();
    approved.sweep = approve_cooperative_sweep_for_config(data_dir, &old_config, &package.sweep)?;

    if let Some(policy) = &package.renewed_policy {
        let workspace = data_dir.join("hww/rotation-policy-review");
        reset_workspace(&workspace)?;
        ceremony::materialize_policy_package(policy, &workspace)?;
        recovery::validate_rotation_policy_binding(
            &old_config,
            &new_config,
            &approved.sweep,
            policy,
        )?;
        approve_policy_for_config(data_dir, &new_config, &workspace)?;
        approved.renewed_policy = Some(ceremony::package_from_batch(&workspace)?);
        reset_workspace(&workspace)?;
    }

    let hww = load_device_keys(data_dir, HWW_DEVICE_FILE)?;
    let current_backup = load_or_migrate_cloud_backup(
        data_dir,
        &data_dir.join(PHONE_BACKUP_FILE),
        &old_config,
        &hww,
    )?;
    social::decrypt_with_hww(&current_backup, &hww.seed)?.validate_against(&old_config)?;
    let friend_public_keys = social::friend_public_keys(&current_backup);
    let payload = RecoveryPayload::new(&new_config, &new_phone)?;
    approved.cloud_recovery_backup = Some(social::create_backup(
        &payload,
        &hww.seed,
        &friend_public_keys,
    )?);
    Ok(approved)
}

fn load_or_migrate_cloud_backup(
    data_dir: &Path,
    backup_path: &Path,
    config: &VaultConfig,
    hww: &DeviceKeys,
) -> Result<CloudRecoveryBackup> {
    if let Ok(backup) = read_json::<CloudRecoveryBackup>(backup_path) {
        return Ok(backup);
    }
    let legacy: EncryptedBlob = read_json(backup_path)
        .context("backup is neither a cloud recovery envelope nor a legacy phone backup")?;
    let words = crypto::decrypt(&hww.seed, "phone-seed-backup", &legacy)?;
    let words = String::from_utf8(words.to_vec()).context("legacy phone backup was not UTF-8")?;
    let phone =
        DeviceKeys::parse_for_network(&Secp256k1::new(), &words, config.bitcoin_network()?)?;
    let payload = RecoveryPayload::new(config, &phone)?;
    let migrated = social::create_backup(&payload, &hww.seed, &[])?;
    if backup_path == data_dir.join(PHONE_BACKUP_FILE) {
        write_json(backup_path, &migrated)?;
    }
    Ok(migrated)
}

fn reset_workspace(path: &Path) -> Result<()> {
    if path.exists() {
        std::fs::remove_dir_all(path)
            .with_context(|| format!("failed to reset workspace {}", path.display()))?;
    }
    Ok(())
}
