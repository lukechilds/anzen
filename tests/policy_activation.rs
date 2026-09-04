use anyhow::{Result, bail};
use anzen::{
    cold_wallet,
    core::{
        ceremony::{SCHEDULE_FILE, Schedule},
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
    fs::remove_dir(&schedule_path).unwrap();
    let schedule = hot_wallet::activate_policy(dir.path(), &backend, &batch).unwrap();
    assert_eq!(schedule.rollover_txid, txid.to_string());
    assert_eq!(
        load_config(dir.path()).unwrap().monthly_limit_sats,
        10_000_000
    );
    assert!(epoch.join("activated.json").is_file());
}
