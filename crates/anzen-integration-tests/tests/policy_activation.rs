use anyhow::{Result, bail};
use anzen_chain::{Blockchain, ChainTip};
use anzen_common::storage::{CONFIG_FILE, VaultConfig, load_config};
use anzen_common::types::VaultUtxo;
use anzen_signer_app::{self, application::SignerApplication};
use anzen_wallet::ceremony::{self, SCHEDULE_FILE, Schedule};
use anzen_wallet::{
    self, HotWallet, HotWalletBackend,
    api::{PolicyApproval, PolicyLimits},
    application::WalletApplication,
};
use bitcoin::{
    Address, Amount, BlockHash, Network, OutPoint, Transaction, TxOut, Txid, hashes::Hash,
};
use std::{
    cell::{Cell, RefCell},
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    str::FromStr,
};

struct Backend {
    coins: Vec<VaultUtxo>,
    connectors: RefCell<Vec<VaultUtxo>>,
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
    fn median_time_past(&self, _: u64) -> Result<u64> {
        Ok(0)
    }
    fn scan_vault(&self, _: &VaultConfig) -> Result<Vec<VaultUtxo>> {
        Ok(self.coins.clone())
    }
    fn scan_connectors(&self, _: &VaultConfig) -> Result<Vec<VaultUtxo>> {
        Ok(self.connectors.borrow().clone())
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

struct Devices {
    dir: tempfile::TempDir,
    wallet: WalletApplication,
    signer: SignerApplication,
}

fn setup() -> (Devices, Backend) {
    let dir = tempfile::tempdir().unwrap();
    anzen_wallet::initialize(dir.path(), Network::Regtest).unwrap();
    anzen_signer_app::initialize(dir.path(), Network::Regtest).unwrap();
    let wallet = WalletApplication::new(dir.path().into(), false);
    let signer = SignerApplication::new(dir.path().into(), false);
    let config = wallet
        .pair_signer(&signer.public_identity().unwrap())
        .unwrap();
    signer.accept_vault(&config).unwrap();
    let request = wallet.propose_backup().unwrap();
    let review = signer.review_backup(&request).unwrap();
    wallet
        .store_backup(&signer.approve_backup(&review.review_id, &request).unwrap())
        .unwrap();
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
        connectors: RefCell::default(),
        reject: Cell::new(false),
        broadcasts: RefCell::default(),
    };
    (
        Devices {
            dir,
            wallet,
            signer,
        },
        backend,
    )
}

fn approved(devices: &Devices, backend: &Backend, limit: u64) -> PolicyApproval {
    let request = devices
        .wallet
        .propose_policy(
            backend,
            PolicyLimits {
                monthly_limit_sats: limit,
                emergency_access_limit_sats: 50_000_000,
            },
        )
        .unwrap();
    let review = devices.signer.review_policy(&request).unwrap();
    devices.signer.approve_policy(&review.review_id).unwrap()
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
    let (devices, backend) = setup();
    let dir = devices.dir.path();
    let old = approved(&devices, &backend, 10_000_000);
    let old_schedule = devices.wallet.activate_policy(&backend, &old).unwrap();
    let before = active_files(dir, &old_schedule);
    // The fake chain still reports the original coin, so the renewal conflicts with the old
    // rollover: the signer accepts it as a replacement rather than a parallel policy.
    let new = approved(&devices, &backend, 5_000_000);
    backend.reject.set(true);
    assert!(devices.wallet.activate_policy(&backend, &new).is_err());
    assert_eq!(active_files(dir, &old_schedule), before);
    assert_eq!(backend.broadcasts.borrow().len(), 1);

    backend.reject.set(false);
    let new_schedule = devices.wallet.activate_policy(&backend, &new).unwrap();
    assert_ne!(new_schedule.rollover_txid, old_schedule.rollover_txid);
    assert_eq!(load_config(dir).unwrap().monthly_limit_sats, 5_000_000);
    for entry in old_schedule.entries {
        let path = dir.join(entry.authorization_file);
        assert_eq!(fs::read(&path).unwrap(), before[&path]);
    }
    // A later replay of an old, already-confirmed policy must not replace the current schedule.
    assert!(
        devices
            .wallet
            .activate_policy(&backend, &old)
            .unwrap_err()
            .to_string()
            .contains("superseded")
    );
    assert_eq!(
        anzen_wallet::load_schedule(dir).unwrap().rollover_txid,
        new_schedule.rollover_txid
    );
}

#[test]
fn upgrading_a_legacy_schedule_still_prevents_reactivating_the_superseded_epoch() {
    let (devices, backend) = setup();
    let dir = devices.dir.path();
    let old = approved(&devices, &backend, 10_000_000);
    let old_schedule = devices.wallet.activate_policy(&backend, &old).unwrap();
    // Legacy schedules have the same public format, but predate durable activation markers.
    let marker = dir
        .join("phone/transactions")
        .join(&old_schedule.rollover_txid)
        .join("activated.json");
    fs::remove_file(&marker).unwrap();
    let new = approved(&devices, &backend, 5_000_000);
    let current = devices.wallet.activate_policy(&backend, &new).unwrap();
    assert!(marker.exists());
    assert!(
        devices
            .wallet
            .activate_policy(&backend, &old)
            .unwrap_err()
            .to_string()
            .contains("superseded")
    );
    assert_eq!(
        anzen_wallet::load_schedule(dir).unwrap().rollover_txid,
        current.rollover_txid
    );
}

#[test]
fn failed_initial_activation_does_not_enable_a_policy() {
    let (devices, backend) = setup();
    let approval = approved(&devices, &backend, 10_000_000);
    backend.reject.set(true);
    assert!(devices.wallet.activate_policy(&backend, &approval).is_err());
    assert!(!devices.dir.path().join(SCHEDULE_FILE).exists());
    assert_eq!(
        load_config(devices.dir.path()).unwrap().monthly_limit_sats,
        0
    );
    assert!(backend.broadcasts.borrow().is_empty());
}

#[test]
fn accepted_rollover_can_resume_after_schedule_write_failure() {
    let (devices, backend) = setup();
    let dir = devices.dir.path();
    let approval = approved(&devices, &backend, 10_000_000);
    let schedule_path = dir.join(SCHEDULE_FILE);
    fs::create_dir(&schedule_path).unwrap();
    let error = devices
        .wallet
        .activate_policy(&backend, &approval)
        .unwrap_err();
    assert!(error.to_string().contains("retry the same approved policy"));
    let txid = backend.broadcasts.borrow()[0].compute_txid();
    let epoch = dir.join("phone/transactions").join(txid.to_string());
    assert!(epoch.join("approved-policy.json").is_file());
    assert!(epoch.join("rollover.json").is_file());
    assert!(epoch.join("schedule.json").is_file());
    let stored = fs::read(epoch.join("approved-policy.json")).unwrap();
    assert!(serde_json::from_slice::<PolicyApproval>(&stored).is_err());
    let retry =
        anzen_wallet::open_approved_policy(dir, serde_json::from_slice(&stored).unwrap()).unwrap();
    assert_eq!(retry, approval);
    let (other_phone, _) = setup();
    assert!(
        anzen_wallet::open_approved_policy(
            other_phone.dir.path(),
            serde_json::from_slice(&stored).unwrap()
        )
        .is_err()
    );
    fs::remove_dir(&schedule_path).unwrap();
    let schedule = devices.wallet.activate_policy(&backend, &retry).unwrap();
    assert_eq!(schedule.rollover_txid, txid.to_string());
    assert_eq!(load_config(dir).unwrap().monthly_limit_sats, 10_000_000);
    assert!(epoch.join("activated.json").is_file());
}

#[test]
fn every_signer_signature_is_checked_before_funding_a_policy() {
    let (devices, backend) = setup();
    let dir = devices.dir.path();
    let old = approved(&devices, &backend, 10_000_000);
    let old_schedule = devices.wallet.activate_policy(&backend, &old).unwrap();
    let before = active_files(dir, &old_schedule);
    let approval = approved(&devices, &backend, 5_000_000);
    for index in 0..approval.signatures.len() {
        for missing in [true, false] {
            let mut changed = approval.clone();
            if missing {
                changed.signatures.remove(index);
            } else {
                // A well-formed signature for a different transaction must also be rejected.
                let other = (index + 1) % changed.signatures.len();
                changed.signatures[index] = changed.signatures[other];
            }
            assert!(devices.wallet.activate_policy(&backend, &changed).is_err());
            assert_eq!(backend.broadcasts.borrow().len(), 1);
            assert_eq!(active_files(dir, &old_schedule), before);
        }
    }
}

#[test]
fn an_approval_cannot_be_redirected_to_other_parameters() {
    let (devices, backend) = setup();
    let approval = approved(&devices, &backend, 10_000_000);
    let mut changed_limit = approval.clone();
    changed_limit.request.limits.monthly_limit_sats += 1;
    let mut changed_destination = approval.clone();
    changed_destination
        .request
        .allowance_destinations
        .swap(0, 1);
    for changed in [changed_limit, changed_destination] {
        let error = devices
            .wallet
            .activate_policy(&backend, &changed)
            .unwrap_err()
            .to_string();
        assert!(
            error.contains("invalid signature") || error.contains("destinations"),
            "{error}"
        );
    }
    assert!(backend.broadcasts.borrow().is_empty());
    assert!(!devices.dir.path().join(SCHEDULE_FILE).exists());
}

#[test]
fn future_vault_inputs_are_fully_signed_and_connectors_stay_unsigned() {
    let (devices, backend) = setup();
    let dir = devices.dir.path();
    let approval = approved(&devices, &backend, 10_000_000);
    let schedule = devices.wallet.activate_policy(&backend, &approval).unwrap();
    let phone =
        anzen_common::storage::load_device_keys(dir, anzen_common::storage::PHONE_DEVICE_FILE)
            .unwrap();
    for entry in schedule.entries {
        let artifact: ceremony::EncryptedTransaction =
            anzen_common::storage::read_json(&dir.join(entry.authorization_file)).unwrap();
        let blob = &artifact.encrypted_psbt;
        let plaintext = anzen_common::crypto::decrypt(&phone.seed, &blob.purpose, blob).unwrap();
        let psbt = bitcoin::Psbt::from_str(std::str::from_utf8(&plaintext).unwrap()).unwrap();
        assert_eq!(psbt.inputs[0].tap_script_sigs.len(), 2);
        assert!(psbt.inputs[0].final_script_witness.is_none());
        assert!(psbt.inputs[1].tap_script_sigs.is_empty());
        assert!(psbt.inputs[1].final_script_witness.is_none());
    }
}

#[test]
fn rotation_rejects_an_incompletely_signed_renewal_before_any_broadcast() {
    let (devices, backend) = setup();
    let dir = devices.dir.path();
    let approval = approved(&devices, &backend, 10_000_000);
    let schedule = devices.wallet.activate_policy(&backend, &approval).unwrap();
    let config = load_config(dir).unwrap();
    // The rollover's outputs are now the live vault and connector state.
    let rollover = &backend.broadcasts.borrow()[0].clone();
    let backend = Backend {
        coins: rollover.output[..2]
            .iter()
            .enumerate()
            .map(|(vout, output)| VaultUtxo {
                outpoint: OutPoint::new(rollover.compute_txid(), vout as u32),
                txout: output.clone(),
                confirmation_height: 2,
            })
            .collect(),
        connectors: RefCell::new(
            rollover.output[2..]
                .iter()
                .enumerate()
                .map(|(index, output)| VaultUtxo {
                    outpoint: OutPoint::new(rollover.compute_txid(), index as u32 + 2),
                    txout: output.clone(),
                    confirmation_height: 2,
                })
                .collect(),
        ),
        reject: Cell::new(false),
        broadcasts: RefCell::default(),
    };
    assert_eq!(schedule.entries.len(), 12);
    let proposal = devices
        .wallet
        .propose_rotation(&backend, false, |_| {})
        .unwrap();
    let review = devices.signer.review_phone_rotation(&proposal).unwrap();
    let mut approved = devices
        .signer
        .approve_phone_rotation(&review.review_id, &proposal)
        .unwrap();
    approved.approval.policy_signatures.pop();
    let error = devices
        .wallet
        .activate_rotation(&backend, &approved)
        .unwrap_err();
    assert!(error.to_string().contains("sign"), "{error}");
    assert!(backend.broadcasts.borrow().is_empty());
    assert_eq!(
        load_config(dir).unwrap().vault_descriptor,
        config.vault_descriptor
    );
}
