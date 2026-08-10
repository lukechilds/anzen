//! Phone-key rotation orchestration.
//!
//! Rotation is intentionally a phone concern. The cold wallet sees only the shared proposal and
//! applies the validation/signing rules in `cold_wallet` and `core`.

use super::{
    HotWallet, HotWalletBackend, VANITY_SUFFIX, VanityPhoneKey, activate_policy,
    available_worker_count, broadcast_cooperative_sweep_for_config, ensure_backend_network,
    grind_vanity_phone_key,
};
use crate::core::{
    ceremony::{
        DEFAULT_BATCH_DIR, PolicyLimits, PolicyPackage, SCHEDULE_FILE, build_policy_proposal,
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
use anyhow::{Context, Result, bail, ensure};
use bitcoin::{Psbt, key::Secp256k1, secp256k1::XOnlyPublicKey};
use chrono::Utc;
use std::{
    fs,
    path::{Path, PathBuf},
    str::FromStr,
};

#[derive(Debug)]
pub struct VanityPhoneRotation {
    pub package: PhoneRotationPackage,
    pub attempts: u64,
    pub worker_count: usize,
    pub resumed: bool,
}

struct RotationContext {
    old_config: VaultConfig,
    utxos: Vec<VaultUtxo>,
    old_phone: DeviceKeys,
}

pub fn create_phone_rotation(
    data_dir: &Path,
    backend: &dyn HotWalletBackend,
) -> Result<PhoneRotationPackage> {
    let context = load_rotation_context(data_dir, backend)?;
    let network = context.old_config.bitcoin_network()?;
    let new_phone = DeviceKeys::generate_for_network(&Secp256k1::new(), network)?;
    build_phone_rotation(data_dir, context, new_phone)
}

pub fn create_vanity_phone_rotation<F>(
    data_dir: &Path,
    backend: &dyn HotWalletBackend,
    report_progress: F,
) -> Result<VanityPhoneRotation>
where
    F: Fn(u64) + Sync,
{
    create_vanity_phone_rotation_with_suffix(
        data_dir,
        backend,
        VANITY_SUFFIX,
        available_worker_count(),
        report_progress,
    )
}

fn create_vanity_phone_rotation_with_suffix<F>(
    data_dir: &Path,
    backend: &dyn HotWalletBackend,
    suffix: &str,
    worker_count: usize,
    report_progress: F,
) -> Result<VanityPhoneRotation>
where
    F: Fn(u64) + Sync,
{
    let context = load_rotation_context(data_dir, backend)?;
    let network = context.old_config.bitcoin_network()?;
    let hww_pubkey = XOnlyPublicKey::from_str(&context.old_config.hww_vault_pubkey)?;
    let pending = load_pending_vanity_phone(data_dir, &context.old_config, hww_pubkey, suffix)?;
    let (new_phone, expected_address, attempts, actual_worker_count, resumed) = match pending {
        Some((phone, address)) => (phone, address, 0, 0, true),
        None => {
            let VanityPhoneKey {
                phone,
                vault_address,
                attempts,
                worker_count,
            } = grind_vanity_phone_key(network, hww_pubkey, suffix, worker_count, report_progress)?;
            (phone, vault_address, attempts, worker_count, false)
        }
    };
    let package = build_phone_rotation(data_dir, context, new_phone)?;
    ensure!(
        package.new_vault_address == expected_address,
        "vanity rotation address changed while building its proposal"
    );
    Ok(VanityPhoneRotation {
        package,
        attempts,
        worker_count: actual_worker_count,
        resumed,
    })
}

fn load_rotation_context(
    data_dir: &Path,
    backend: &dyn HotWalletBackend,
) -> Result<RotationContext> {
    let old_config = load_config(data_dir)?;
    ensure_backend_network(backend, &old_config)?;
    let utxos = backend.scan_vault(&old_config)?;
    let old_phone = load_device_keys(data_dir, PHONE_DEVICE_FILE)?;
    Ok(RotationContext {
        old_config,
        utxos,
        old_phone,
    })
}

fn load_pending_vanity_phone(
    data_dir: &Path,
    old_config: &VaultConfig,
    hww_pubkey: XOnlyPublicKey,
    suffix: &str,
) -> Result<Option<(DeviceKeys, String)>> {
    let pending_path = data_dir.join(recovery::PENDING_PHONE_ROTATION_FILE);
    if !pending_path.exists() {
        return Ok(None);
    }
    let network = old_config.bitcoin_network()?;
    let pending: DeviceFile = read_json(&pending_path)?;
    if pending.kind != "pending-phone-rotation" || pending.bitcoin_network()? != network {
        bail!("existing pending phone rotation is invalid for this vault");
    }
    let phone = DeviceKeys::parse_for_network_at_index(
        &Secp256k1::new(),
        &pending.mnemonic,
        network,
        pending.vault_key_index,
    )?;
    let policy = VaultPolicy::new_for_network(phone.vault_pubkey, hww_pubkey, network)?;
    let expected_prefix = match network {
        bitcoin::Network::Bitcoin => format!("bc1p{suffix}"),
        bitcoin::Network::Regtest => format!("bcrt1p{suffix}"),
        other => bail!("vanity rotation is unsupported on {other}"),
    };
    let address = policy.address.to_string();
    if !address.starts_with(&expected_prefix) {
        bail!(
            "a non-vanity phone rotation is already pending; finish it before starting a vanity rotation"
        );
    }
    Ok(Some((phone, address)))
}

fn build_phone_rotation(
    data_dir: &Path,
    context: RotationContext,
    new_phone: DeviceKeys,
) -> Result<PhoneRotationPackage> {
    let RotationContext {
        old_config,
        utxos,
        old_phone,
    } = context;
    let network = old_config.bitcoin_network()?;
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

    let sweep =
        recovery::create_cooperative_sweep(&old_config, &utxos, &new_policy.address, &old_phone)?;
    let renewed_policy =
        if old_config.monthly_limit_sats == 0 && old_config.emergency_access_limit_sats == 0 {
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
        emergency_access_limit_sats: old_config.emergency_access_limit_sats,
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
            bail!("HWW approval is missing from the renewed vault policy");
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
        if let Some(emergency) = &policy.manifest.emergency_access {
            ensure!(
                hot.next_receive_address()?.to_string() == emergency.hot_address,
                "renewed emergency access does not match the new phone address sequence"
            );
        }
        for allowance in &policy.manifest.allowances {
            if hot.next_receive_address()?.to_string() != allowance.hot_address {
                bail!("renewed allowance policy does not match the new phone address sequence");
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
        PolicyLimits {
            monthly_limit_sats: old_config.monthly_limit_sats,
            emergency_access_limit_sats: old_config.emergency_access_limit_sats,
        },
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
    if let Some(emergency) = &renewed.manifest.emergency_access {
        ensure!(
            addresses.next_receive_address()?.to_string() == emergency.hot_address,
            "renewed emergency access does not use the new phone's address sequence"
        );
    }
    for allowance in &renewed.manifest.allowances {
        if addresses.next_receive_address()?.to_string() != allowance.hot_address {
            bail!("renewed allowance policy does not use the new phone's address sequence");
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        cold_wallet,
        core::{
            chain::{Blockchain, ChainTip},
            storage::{CONFIG_FILE, initialize_vault, read_json, write_json},
        },
        hot_wallet::{self, HotWalletBackend},
    };
    use anyhow::Result;
    use bitcoin::{
        Address, Amount, BlockHash, Network, OutPoint, Transaction, TxOut, Txid, hashes::Hash as _,
    };
    use std::str::FromStr;

    struct TestBackend {
        network: Network,
        utxos: Vec<VaultUtxo>,
    }

    impl Blockchain for TestBackend {
        fn network(&self) -> Network {
            self.network
        }

        fn backend_description(&self) -> String {
            "test backend".to_owned()
        }

        fn chain_tip(&self) -> Result<ChainTip> {
            Ok(ChainTip {
                network: self.network,
                height: 1,
                median_time: 0,
                best_block_hash: BlockHash::all_zeros(),
            })
        }

        fn scan_vault(&self, _config: &VaultConfig) -> Result<Vec<VaultUtxo>> {
            Ok(self.utxos.clone())
        }

        fn broadcast(&self, transaction: &Transaction) -> Result<Txid> {
            Ok(transaction.compute_txid())
        }
    }

    impl HotWalletBackend for TestBackend {
        fn sync_hot_wallet(&self, _wallet: &mut HotWallet) -> Result<()> {
            Ok(())
        }
    }

    #[test]
    fn vanity_rotation_grinds_resumes_and_preserves_policy() {
        let dir = tempfile::tempdir().unwrap();
        let network = Network::Regtest;
        hot_wallet::initialize(dir.path(), network).unwrap();
        cold_wallet::initialize(dir.path(), network).unwrap();
        let mut config = initialize_vault(dir.path()).unwrap();
        config.monthly_limit_sats = 10_000_000;
        config.emergency_access_limit_sats = 50_000_000;
        write_json(&dir.path().join(CONFIG_FILE), &config).unwrap();

        let script_pubkey = Address::from_str(&config.vault_address)
            .unwrap()
            .require_network(network)
            .unwrap()
            .script_pubkey();
        let backend = TestBackend {
            network,
            utxos: vec![VaultUtxo {
                outpoint: OutPoint::new(Txid::all_zeros(), 0),
                txout: TxOut {
                    value: Amount::from_sat(210_000_000),
                    script_pubkey,
                },
                confirmation_height: 1,
            }],
        };

        let first =
            create_vanity_phone_rotation_with_suffix(dir.path(), &backend, "v", 4, |_| {}).unwrap();
        assert!(!first.resumed);
        assert!(first.attempts > 0);
        assert_eq!(first.worker_count, 4);
        assert!(first.package.new_vault_address.starts_with("bcrt1pv"));
        assert_ne!(first.package.new_vault_address, config.vault_address);
        assert_eq!(first.package.monthly_limit_sats, 10_000_000);
        assert_eq!(first.package.emergency_access_limit_sats, 50_000_000);
        let renewed = first.package.renewed_policy.as_ref().unwrap();
        assert_eq!(renewed.manifest.monthly_limit_sats, 10_000_000);
        assert_eq!(renewed.manifest.allowances.len(), 12);
        assert_eq!(renewed.manifest.emergency_access_limit_sats, 50_000_000);
        assert!(renewed.manifest.emergency_access.is_some());

        let pending: DeviceFile =
            read_json(&dir.path().join(recovery::PENDING_PHONE_ROTATION_FILE)).unwrap();
        let pending_phone = DeviceKeys::parse_for_network_at_index(
            &Secp256k1::new(),
            &pending.mnemonic,
            network,
            pending.vault_key_index,
        )
        .unwrap();
        assert_eq!(
            pending_phone.vault_pubkey.to_string(),
            first.package.new_phone_vault_pubkey
        );

        let second =
            create_vanity_phone_rotation_with_suffix(dir.path(), &backend, "v", 4, |_| {}).unwrap();
        assert!(second.resumed);
        assert_eq!(second.attempts, 0);
        assert_eq!(second.worker_count, 0);
        assert_eq!(
            second.package.new_vault_address,
            first.package.new_vault_address
        );
        assert_eq!(
            second.package.new_phone_vault_pubkey,
            first.package.new_phone_vault_pubkey
        );
    }
}
