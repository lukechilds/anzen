//! Phone-key rotation orchestration.
//!
//! Rotation is intentionally a phone concern. The cold wallet sees only the shared proposal and
//! applies the validation/signing rules in `cold_wallet` and `core`.

use super::{
    HotWallet, HotWalletBackend, activate_policy, broadcast_cooperative_sweep_for_config,
    ensure_backend_network,
};
use crate::core::{
    ceremony::{
        DEFAULT_BATCH_DIR, PolicyPackage, SCHEDULE_FILE, build_policy_proposal,
        materialize_policy_package, package_from_batch, validate_batch,
    },
    keys::DeviceKeys,
    policy::VaultPolicy,
    recovery::{
        self, PhoneRotationPackage, RotationResult, validate_phone_rotation,
        validate_rotation_policy_binding,
    },
    storage::{
        CONFIG_FILE, DeviceFile, PHONE_BACKUP_FILE, PHONE_DEVICE_FILE, VaultConfig, load_config,
        load_device_keys, read_json, write_json,
    },
    types::VaultUtxo,
};
use anyhow::{Context, Result, bail};
use bitcoin::{Psbt, key::Secp256k1, secp256k1::XOnlyPublicKey};
use chrono::Utc;
use std::{
    fs,
    path::{Path, PathBuf},
    str::FromStr,
};

pub fn create_phone_rotation(
    data_dir: &Path,
    backend: &dyn HotWalletBackend,
) -> Result<PhoneRotationPackage> {
    let old_config = load_config(data_dir)?;
    ensure_backend_network(backend, &old_config)?;
    let network = old_config.bitcoin_network()?;
    let new_phone = DeviceKeys::generate_for_network(&Secp256k1::new(), network)?;
    let hww_pubkey = XOnlyPublicKey::from_str(&old_config.hww_vault_pubkey)?;
    let new_policy = VaultPolicy::new_for_network(new_phone.vault_pubkey, hww_pubkey, network)?;
    let new_config = recovery::rotated_config(&old_config, &new_phone, &new_policy)?;
    write_json(
        &data_dir.join(recovery::PENDING_PHONE_ROTATION_FILE),
        &DeviceFile {
            kind: "pending-phone-rotation".to_owned(),
            network: old_config.network.clone(),
            mnemonic: new_phone.mnemonic.to_string(),
            vault_key_index: new_phone.vault_key_index,
        },
    )?;

    let utxos = backend.scan_vault(&old_config)?;
    let old_phone = load_device_keys(data_dir, PHONE_DEVICE_FILE)?;
    let sweep =
        recovery::create_cooperative_sweep(&old_config, &utxos, &new_policy.address, &old_phone)?;
    let renewed_policy = if old_config.monthly_limit_sats == 0 {
        None
    } else {
        Some(build_renewed_policy(
            data_dir,
            &old_config,
            &new_config,
            &new_phone,
            &sweep,
        )?)
    };
    Ok(PhoneRotationPackage {
        version: 1,
        kind: "phone-key-rotation".to_owned(),
        old_vault_descriptor: old_config.vault_descriptor,
        new_phone_vault_pubkey: new_phone.vault_pubkey.to_string(),
        new_vault_descriptor: new_policy.descriptor_string(),
        new_vault_address: new_policy.address.to_string(),
        monthly_limit_sats: old_config.monthly_limit_sats,
        sweep,
        renewed_policy,
        cloud_recovery_backup: None,
    })
}

pub fn activate_phone_rotation(
    data_dir: &Path,
    backend: &dyn HotWalletBackend,
    package: &PhoneRotationPackage,
) -> Result<RotationResult> {
    let (old_config, new_config, new_phone) = validate_phone_rotation(data_dir, package)?;
    ensure_backend_network(backend, &old_config)?;
    let backup = package
        .cloud_recovery_backup
        .as_ref()
        .context("HWW-approved phone backup is missing from the rotation package")?;
    if !package.sweep.hww_approved {
        bail!("HWW approval is missing from the rotation package");
    }
    let pending: DeviceFile = read_json(&data_dir.join(recovery::PENDING_PHONE_ROTATION_FILE))?;
    let policy_workspace = data_dir.join("phone/rotation-policy-activation");
    if let Some(policy) = &package.renewed_policy {
        if !policy.manifest.hww_approved {
            bail!("HWW approval is missing from the renewed monthly policy");
        }
        reset_workspace(&policy_workspace)?;
        materialize_policy_package(policy, &policy_workspace)?;
        validate_batch(&new_config, &policy.manifest, &policy_workspace)?;
        validate_rotation_policy_binding(&old_config, &new_config, &package.sweep, policy)?;
        validate_rotation_hot_addresses(&new_phone, policy)?;
    }

    let sweep = broadcast_cooperative_sweep_for_config(backend, &old_config, &package.sweep)?;
    archive_old_epoch(data_dir, &old_config, sweep.txid)?;
    write_json(&data_dir.join(CONFIG_FILE), &new_config)?;
    write_json(
        &data_dir.join(PHONE_DEVICE_FILE),
        &DeviceFile {
            kind: "phone".to_owned(),
            network: new_config.network.clone(),
            mnemonic: pending.mnemonic.clone(),
            vault_key_index: pending.vault_key_index,
        },
    )?;
    write_json(&data_dir.join(PHONE_BACKUP_FILE), backup)?;

    let mut hot = HotWallet::open_or_create(data_dir)?;
    if let Some(policy) = &package.renewed_policy {
        for month in &policy.manifest.months {
            if hot.next_receive_address()?.to_string() != month.hot_address {
                bail!("renewed monthly policy does not match the new phone address sequence");
            }
        }
    }
    let renewed_schedule = package
        .renewed_policy
        .as_ref()
        .map(|_| activate_policy(data_dir, backend, &policy_workspace))
        .transpose()?;
    reset_workspace(&policy_workspace)?;
    let pending_path = data_dir.join(recovery::PENDING_PHONE_ROTATION_FILE);
    if pending_path.exists() {
        fs::remove_file(pending_path)?;
    }

    Ok(RotationResult {
        sweep,
        old_address: old_config.vault_address,
        new_address: new_config.vault_address,
        new_phone_mnemonic: pending.mnemonic,
        renewed_schedule,
    })
}

fn build_renewed_policy(
    data_dir: &Path,
    old_config: &VaultConfig,
    new_config: &VaultConfig,
    new_phone: &DeviceKeys,
    sweep: &recovery::CooperativeSweepPackage,
) -> Result<PolicyPackage> {
    let sweep_psbt = Psbt::from_str(&sweep.psbt).context("invalid rotation sweep PSBT")?;
    let sweep_output = sweep_psbt
        .unsigned_tx
        .output
        .first()
        .context("rotation sweep has no output")?;
    let virtual_utxo = VaultUtxo {
        outpoint: bitcoin::OutPoint::new(sweep_psbt.unsigned_tx.compute_txid(), 0),
        txout: sweep_output.clone(),
        confirmation_height: 0,
    };
    let workspace = data_dir.join("phone/rotation-policy-proposal");
    reset_workspace(&workspace)?;
    let mut addresses = HotWallet::ephemeral(new_phone)?;
    build_policy_proposal(
        new_config,
        &[virtual_utxo],
        Utc::now(),
        old_config.monthly_limit_sats,
        &workspace,
        new_phone,
        &mut addresses,
    )?;
    let package = package_from_batch(&workspace)?;
    reset_workspace(&workspace)?;
    Ok(package)
}

fn validate_rotation_hot_addresses(new_phone: &DeviceKeys, renewed: &PolicyPackage) -> Result<()> {
    let mut addresses = HotWallet::ephemeral(new_phone)?;
    for month in &renewed.manifest.months {
        if addresses.next_receive_address()?.to_string() != month.hot_address {
            bail!("renewed monthly policy does not use the new phone's address sequence");
        }
    }
    Ok(())
}

fn reset_workspace(path: &Path) -> Result<()> {
    if path.exists() {
        fs::remove_dir_all(path)
            .with_context(|| format!("failed to reset workspace {}", path.display()))?;
    }
    Ok(())
}

fn archive_old_epoch(data_dir: &Path, old_config: &VaultConfig, txid: bitcoin::Txid) -> Result<()> {
    let archive = data_dir.join("history").join(format!("rotation-{txid}"));
    fs::create_dir_all(&archive)
        .with_context(|| format!("failed to create rotation archive {}", archive.display()))?;
    write_json(&archive.join(CONFIG_FILE), old_config)?;

    for relative in [
        PathBuf::from(SCHEDULE_FILE),
        PathBuf::from("phone/transactions"),
        PathBuf::from(DEFAULT_BATCH_DIR),
        PathBuf::from("phone/hot-wallet.sqlite-wal"),
        PathBuf::from("phone/hot-wallet.sqlite-shm"),
        PathBuf::from("phone/hot-wallet.sqlite"),
    ] {
        let source = data_dir.join(&relative);
        if !source.exists() {
            continue;
        }
        let destination = archive.join(&relative);
        if let Some(parent) = destination.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::rename(&source, &destination).with_context(|| {
            format!(
                "failed to archive obsolete state {} to {}",
                source.display(),
                destination.display()
            )
        })?;
    }
    Ok(())
}
