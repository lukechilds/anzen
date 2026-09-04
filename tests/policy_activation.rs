use anyhow::{Result, bail};
use anzen::{
    cold_wallet,
    core::{
        ceremony::{self, SCHEDULE_FILE, Schedule},
        chain::{Blockchain, ChainTip},
        storage::{CONFIG_FILE, VaultConfig, initialize_vault, load_config},
        types::VaultUtxo,
    },
    hot_wallet::{self, HotWallet, HotWalletBackend},
};
use bitcoin::{
    Address, Amount, BlockHash, Network, OutPoint, Transaction, TxOut, Txid, hashes::Hash,
};
use chrono::Utc;
use std::{
    cell::{Cell, RefCell},
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    str::FromStr,
};

struct Backend {
    coins: Vec<VaultUtxo>,
    reject: Cell<bool>,
    broadcasts: RefCell<Vec<Transaction>>,
}

impl Blockchain for Backend {
    fn network(&self) -> Network {
        Network::Regtest
    }
    fn backend_description(&self) -> String {
        "test backend".into()
    }
    fn chain_tip(&self) -> Result<ChainTip> {
        Ok(ChainTip {
            network: Network::Regtest,
            height: 101,
            median_time: 0,
            best_block_hash: BlockHash::all_zeros(),
        })
    }
    fn scan_vault(&self, _: &VaultConfig) -> Result<Vec<VaultUtxo>> {
        Ok(self.coins.clone())
    }
    fn scan_connectors(&self, _: &VaultConfig) -> Result<Vec<VaultUtxo>> {
        Ok(Vec::new())
    }
    fn broadcast(&self, transaction: &Transaction) -> Result<Txid> {
        if self.reject.get() {
            bail!("test rollover rejected");
        }
        self.broadcasts.borrow_mut().push(transaction.clone());
        Ok(transaction.compute_txid())
    }
}

impl HotWalletBackend for Backend {
    fn sync_hot_wallet(&self, _: &mut HotWallet) -> Result<()> {
        Ok(())
    }
}

fn setup() -> (tempfile::TempDir, Backend) {
    let dir = tempfile::tempdir().unwrap();
    hot_wallet::initialize(dir.path(), Network::Regtest).unwrap();
    cold_wallet::initialize(dir.path(), Network::Regtest).unwrap();
    let config = initialize_vault(dir.path()).unwrap();
    let backend = Backend {
        coins: vec![VaultUtxo {
            outpoint: OutPoint::new(Txid::all_zeros(), 0),
            txout: TxOut {
                value: Amount::from_sat(210_000_000),
                script_pubkey: config
                    .vault_address
                    .parse::<Address<_>>()
                    .unwrap()
                    .require_network(Network::Regtest)
                    .unwrap()
                    .script_pubkey(),
            },
            confirmation_height: 1,
        }],
        reject: Cell::new(false),
        broadcasts: RefCell::default(),
    };
    (dir, backend)
}

fn approved_batch(dir: &Path, backend: &Backend, name: &str, limit: u64) -> PathBuf {
    let batch = dir.join(name);
    hot_wallet::propose_policy(dir, backend, Utc::now(), limit, 50_000_000, &batch).unwrap();
    cold_wallet::approve_policy(dir, &batch).unwrap();
    batch
}

fn active_files(dir: &Path, schedule: &Schedule) -> BTreeMap<PathBuf, Vec<u8>> {
    let mut names = vec![SCHEDULE_FILE.to_owned(), CONFIG_FILE.to_owned()];
    names.extend(
        schedule
            .entries
            .iter()
            .map(|entry| entry.authorization_file.clone()),
    );
    if let Some(emergency) = &schedule.emergency_access {
        names.extend([
            emergency.trigger_file.clone(),
            emergency.withdrawal_file.clone(),
        ]);
    }
    names
        .into_iter()
        .map(|name| {
            let path = dir.join(name);
            (path.clone(), fs::read(path).unwrap())
        })
        .collect()
}

#[test]
fn rejected_rollover_preserves_active_schedule_limits_and_encrypted_transactions() {
    let (dir, backend) = setup();
    let old = approved_batch(dir.path(), &backend, "old", 10_000_000);
    let old_schedule = hot_wallet::activate_policy(dir.path(), &backend, &old).unwrap();
    let before = active_files(dir.path(), &old_schedule);
    let new = approved_batch(dir.path(), &backend, "new", 5_000_000);
    backend.reject.set(true);
    assert!(hot_wallet::activate_policy(dir.path(), &backend, &new).is_err());
    assert_eq!(active_files(dir.path(), &old_schedule), before);
    assert_eq!(backend.broadcasts.borrow().len(), 1);

    backend.reject.set(false);
    let new_schedule = hot_wallet::activate_policy(dir.path(), &backend, &new).unwrap();
    assert_ne!(new_schedule.rollover_txid, old_schedule.rollover_txid);
    assert_eq!(
        load_config(dir.path()).unwrap().monthly_limit_sats,
        5_000_000
    );
    for entry in old_schedule.entries {
        let path = dir.path().join(entry.authorization_file);
        assert_eq!(fs::read(&path).unwrap(), before[&path]);
    }
    // A later replay of an old, already-confirmed policy must not replace the current schedule.
    assert!(
        hot_wallet::activate_policy(dir.path(), &backend, &old)
            .unwrap_err()
            .to_string()
            .contains("superseded")
    );
    assert_eq!(
        hot_wallet::load_schedule(dir.path()).unwrap().rollover_txid,
        new_schedule.rollover_txid
    );
}

#[test]
fn upgrading_a_legacy_schedule_still_prevents_reactivating_the_superseded_epoch() {
    let (dir, backend) = setup();
    let old = approved_batch(dir.path(), &backend, "old", 10_000_000);
    let old_schedule = hot_wallet::activate_policy(dir.path(), &backend, &old).unwrap();
    // Legacy schedules have the same public format, but predate durable activation markers.
    let marker = dir
        .path()
        .join("phone/transactions")
        .join(&old_schedule.rollover_txid)
        .join("activated.json");
    fs::remove_file(&marker).unwrap();
    let new = approved_batch(dir.path(), &backend, "new", 5_000_000);
    let current = hot_wallet::activate_policy(dir.path(), &backend, &new).unwrap();
    assert!(marker.exists());
    assert!(
        hot_wallet::activate_policy(dir.path(), &backend, &old)
            .unwrap_err()
            .to_string()
            .contains("superseded")
    );
    assert_eq!(
        hot_wallet::load_schedule(dir.path()).unwrap().rollover_txid,
        current.rollover_txid
    );
}

#[test]
fn failed_initial_activation_does_not_enable_a_policy() {
    let (dir, backend) = setup();
    let batch = approved_batch(dir.path(), &backend, "proposal", 10_000_000);
    backend.reject.set(true);
    assert!(hot_wallet::activate_policy(dir.path(), &backend, &batch).is_err());
    assert!(!dir.path().join(SCHEDULE_FILE).exists());
    assert_eq!(load_config(dir.path()).unwrap().monthly_limit_sats, 0);
    assert!(backend.broadcasts.borrow().is_empty());
}

#[test]
fn accepted_rollover_can_resume_after_schedule_write_failure() {
    let (dir, backend) = setup();
    let batch = approved_batch(dir.path(), &backend, "proposal", 10_000_000);
    let schedule_path = dir.path().join(SCHEDULE_FILE);
    fs::create_dir(&schedule_path).unwrap();
    let error = hot_wallet::activate_policy(dir.path(), &backend, &batch).unwrap_err();
    assert!(error.to_string().contains("retry the same approved policy"));
    let txid = backend.broadcasts.borrow()[0].compute_txid();
    let epoch = dir.path().join("phone/transactions").join(txid.to_string());
    assert!(epoch.join("approved-policy.json").is_file());
    assert!(epoch.join("rollover.json").is_file());
    assert!(epoch.join("schedule.json").is_file());
    let stored = fs::read(epoch.join("approved-policy.json")).unwrap();
    assert!(serde_json::from_slice::<ceremony::PolicyPackage>(&stored).is_err());
    let input = serde_json::from_slice(&stored).unwrap();
    let package = hot_wallet::open_approved_policy(dir.path(), input).unwrap();
    let retry_batch = dir.path().join("retry-from-encrypted-backup");
    ceremony::materialize_policy_package(&package, &retry_batch).unwrap();
    let (other_phone, _) = setup();
    assert!(
        hot_wallet::open_approved_policy(
            other_phone.path(),
            serde_json::from_slice(&stored).unwrap()
        )
        .is_err()
    );
    fs::remove_dir(&schedule_path).unwrap();
    let schedule = hot_wallet::activate_policy(dir.path(), &backend, &retry_batch).unwrap();
    assert_eq!(schedule.rollover_txid, txid.to_string());
    assert_eq!(
        load_config(dir.path()).unwrap().monthly_limit_sats,
        10_000_000
    );
    assert!(epoch.join("activated.json").is_file());
}

#[test]
fn every_future_vault_signature_is_checked_before_funding_a_policy() {
    let (dir, backend) = setup();
    let old = approved_batch(dir.path(), &backend, "old", 10_000_000);
    let old_schedule = hot_wallet::activate_policy(dir.path(), &backend, &old).unwrap();
    let before = active_files(dir.path(), &old_schedule);
    let batch = approved_batch(dir.path(), &backend, "new", 5_000_000);
    let manifest = ceremony::load_manifest(&batch).unwrap();
    let rollover = ceremony::read_psbt(&batch.join(&manifest.rollover.psbt_file)).unwrap();
    for transaction in ceremony::manifest_transactions(&manifest)
        .into_iter()
        .skip(1)
    {
        let path = batch.join(&transaction.psbt_file);
        let original = ceremony::read_psbt(&path).unwrap();
        assert_eq!(original.inputs[0].tap_script_sigs.len(), 2);
        for key in original.inputs[0].tap_script_sigs.keys() {
            for missing in [true, false] {
                let mut changed = original.clone();
                if missing {
                    changed.inputs[0].tap_script_sigs.remove(key);
                } else {
                    // A well-formed signature for a different transaction must also be rejected.
                    changed.inputs[0]
                        .tap_script_sigs
                        .insert(*key, rollover.inputs[0].tap_script_sigs[key]);
                }
                ceremony::write_psbt(&path, &changed).unwrap();
                let error = hot_wallet::activate_policy(dir.path(), &backend, &batch).unwrap_err();
                assert!(error.to_string().contains("invalid vault signatures"));
                assert_eq!(backend.broadcasts.borrow().len(), 1);
                assert_eq!(active_files(dir.path(), &old_schedule), before);
            }
        }
        ceremony::write_psbt(&path, &original).unwrap();
    }
}

#[test]
fn future_vault_inputs_must_be_finalizable_without_signing_their_connectors() {
    let (dir, backend) = setup();
    let batch = approved_batch(dir.path(), &backend, "proposal", 10_000_000);
    let manifest = ceremony::load_manifest(&batch).unwrap();
    let path = batch.join(&manifest.allowances[0].authorization.psbt_file);
    let original = ceremony::read_psbt(&path).unwrap();
    let mut missing_script = original.clone();
    missing_script.inputs[0].tap_scripts.clear();
    ceremony::write_psbt(&path, &missing_script).unwrap();
    assert!(
        hot_wallet::activate_policy(dir.path(), &backend, &batch)
            .unwrap_err()
            .to_string()
            .contains("cannot finalize")
    );
    assert!(backend.broadcasts.borrow().is_empty());
    assert!(!dir.path().join(SCHEDULE_FILE).exists());

    ceremony::write_psbt(&path, &original).unwrap();
    let schedule = hot_wallet::activate_policy(dir.path(), &backend, &batch).unwrap();
    let phone =
        anzen::core::storage::load_device_keys(dir.path(), anzen::core::storage::PHONE_DEVICE_FILE)
            .unwrap();
    for entry in schedule.entries {
        let artifact: ceremony::EncryptedTransaction =
            anzen::core::storage::read_json(&dir.path().join(entry.authorization_file)).unwrap();
        let blob = &artifact.encrypted_psbt;
        let plaintext = anzen::core::crypto::decrypt(&phone.seed, &blob.purpose, blob).unwrap();
        let psbt = bitcoin::Psbt::from_str(std::str::from_utf8(&plaintext).unwrap()).unwrap();
        assert_eq!(psbt.inputs[0].tap_script_sigs.len(), 2);
        assert!(psbt.inputs[0].final_script_witness.is_none());
        assert!(psbt.inputs[1].tap_script_sigs.is_empty());
        assert!(psbt.inputs[1].final_script_witness.is_none());
    }
}

#[test]
fn rotation_rejects_an_incompletely_signed_renewal_before_any_broadcast() {
    let (dir, backend) = setup();
    let config = load_config(dir.path()).unwrap();
    cold_wallet::create_cloud_recovery_backup(dir.path(), &config).unwrap();
    anzen::core::storage::set_policy_limits(dir.path(), 10_000_000, 50_000_000).unwrap();
    let proposal = hot_wallet::create_phone_rotation(dir.path(), &backend).unwrap();
    let mut approved = cold_wallet::approve_phone_rotation(dir.path(), &proposal).unwrap();
    let renewal = approved.renewed_policy.as_mut().unwrap();
    let path = &renewal
        .manifest
        .emergency_access
        .as_ref()
        .unwrap()
        .withdrawal
        .psbt_file;
    let serialized = renewal.psbts.get_mut(path).unwrap();
    let mut psbt = bitcoin::Psbt::from_str(serialized).unwrap();
    psbt.inputs[0].tap_script_sigs.clear();
    *serialized = psbt.to_string();
    let error = hot_wallet::activate_phone_rotation(dir.path(), &backend, &approved).unwrap_err();
    assert!(error.to_string().contains("invalid vault signatures"));
    assert!(backend.broadcasts.borrow().is_empty());
    assert_eq!(
        load_config(dir.path()).unwrap().vault_descriptor,
        config.vault_descriptor
    );
}
