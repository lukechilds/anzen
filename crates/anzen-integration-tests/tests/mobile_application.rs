//! Consumer-side tests deliberately import only device APIs, never `anzen::core`.
use anyhow::Result;
use anzen_protocol::{crypto::SoftwareCrypto, policy::PolicyGraph};
use anzen_signer_app::application::{SignerApplication, SignerRequest};
use anzen_wallet::{
    HotWallet, HotWalletBackend,
    api::{
        Blockchain, ChainTip, CloudRecoveryBackup, Network, OfflineAction, OutgoingMessageKind,
        PolicyLimits, PolicyRequest, VaultConfig, VaultUtxo,
    },
    application::{RecoveryContact, WalletApplication, WalletRequest},
};
use bitcoin::{Address, Amount, BlockHash, OutPoint, Transaction, TxOut, Txid, hashes::Hash};
use std::{
    cell::{Cell, RefCell},
    str::FromStr,
};
use tempfile::TempDir;

struct Devices {
    phone_dir: TempDir,
    signer_dir: TempDir,
    phone: WalletApplication,
    signer: SignerApplication,
    config: VaultConfig,
}

impl Devices {
    fn paired() -> Self {
        let phone_dir = tempfile::tempdir().unwrap();
        let signer_dir = tempfile::tempdir().unwrap();
        let phone = WalletApplication::new(phone_dir.path().into(), false);
        let signer = SignerApplication::new(signer_dir.path().into(), false);
        phone.initialize(Network::Regtest).unwrap();
        let identity = signer.initialize(Network::Regtest).unwrap();
        let config = phone.pair_signer(&identity).unwrap();
        signer.accept_vault(&config).unwrap();
        Self {
            phone_dir,
            signer_dir,
            phone,
            signer,
            config,
        }
    }
}

struct ContactFixture {
    _directory: TempDir,
    app: WalletApplication,
    public: RecoveryContact,
}

impl ContactFixture {
    fn enroll(devices: &Devices) -> Self {
        let directory = tempfile::tempdir().unwrap();
        let app = WalletApplication::new(directory.path().into(), false);
        app.initialize(Network::Regtest).unwrap();
        let public = app.generate_recovery_contact("Alice").unwrap();
        let request = devices.phone.propose_backup_with_contact(&public).unwrap();
        let review = devices.signer.review_backup(&request).unwrap();
        let approved = devices
            .signer
            .approve_backup(&review.review_id, &request)
            .unwrap();
        devices.phone.store_backup(&approved).unwrap();
        Self {
            _directory: directory,
            app,
            public,
        }
    }

    fn assert_recovers(&self, backup: &CloudRecoveryBackup, config: &VaultConfig) {
        assert_eq!(backup.friends.len(), 1);
        assert_eq!(backup.friends[0].fingerprint, self.public.fingerprint);
        let directory = tempfile::tempdir().unwrap();
        let replacement = WalletApplication::new(directory.path().into(), false);
        replacement.initialize(Network::Regtest).unwrap();
        let request = replacement
            .propose_contact_restore(backup, &self.public)
            .unwrap();
        let review = self.app.review_contact_restore(&request).unwrap();
        let approved = self
            .app
            .approve_contact_restore(&review.review_id, &request)
            .unwrap();
        let recovered = replacement.store_contact_restored_key(&approved).unwrap();
        assert_eq!(recovered.config.vault_descriptor, config.vault_descriptor);
        assert_eq!(
            recovered.config.phone_vault_pubkey,
            config.phone_vault_pubkey
        );
        assert_eq!(recovered.config.hww_vault_pubkey, config.hww_vault_pubkey);
    }
}

struct TestBackend {
    broadcasts: Cell<usize>,
}
impl Blockchain for TestBackend {
    fn network(&self) -> Network {
        Network::Regtest
    }
    fn backend_description(&self) -> String {
        "unit test chain; not consensus verification".into()
    }
    fn chain_tip(&self) -> Result<ChainTip> {
        Ok(ChainTip {
            network: Network::Regtest,
            height: 100,
            median_time: 1_800_000_000,
            best_block_hash: BlockHash::all_zeros(),
        })
    }
    fn scan_vault(&self, config: &VaultConfig) -> Result<Vec<VaultUtxo>> {
        let address =
            Address::from_str(&config.vault_address)?.require_network(Network::Regtest)?;
        Ok(vec![VaultUtxo {
            outpoint: OutPoint {
                txid: Txid::from_byte_array([17; 32]),
                vout: 0,
            },
            txout: TxOut {
                value: Amount::from_sat(210_000_000),
                script_pubkey: address.script_pubkey(),
            },
            confirmation_height: 10,
        }])
    }
    fn scan_connectors(&self, _: &VaultConfig) -> Result<Vec<VaultUtxo>> {
        Ok(vec![])
    }
    fn broadcast(&self, transaction: &Transaction) -> Result<Txid> {
        self.broadcasts.set(self.broadcasts.get() + 1);
        Ok(transaction.compute_txid())
    }
}
impl HotWalletBackend for TestBackend {
    fn sync_hot_wallet(&self, _: &mut HotWallet) -> Result<()> {
        Ok(())
    }
}

/// Rebuild a proposal's graph with the public protocol builders the signer uses.
fn graph(request: &PolicyRequest) -> PolicyGraph {
    request.build(&mut SoftwareCrypto::new()).unwrap().1
}

fn bitcoin_tx(transaction: &anzen_protocol::tx::Transaction) -> Transaction {
    bitcoin::consensus::deserialize(&transaction.serialize()).unwrap()
}

struct OfflineBackend {
    coins: Vec<VaultUtxo>,
    connectors: Vec<VaultUtxo>,
    height: Cell<u64>,
    broadcasts: Cell<usize>,
    live: Cell<bool>,
    lose_ack: Cell<bool>,
}
impl Blockchain for OfflineBackend {
    fn network(&self) -> Network {
        Network::Regtest
    }
    fn backend_description(&self) -> String {
        "offline-action fixture, not consensus verification".into()
    }
    fn chain_tip(&self) -> Result<ChainTip> {
        Ok(ChainTip {
            height: self.height.get(),
            ..TestBackend {
                broadcasts: Cell::new(0),
            }
            .chain_tip()?
        })
    }
    fn scan_vault(&self, _: &VaultConfig) -> Result<Vec<VaultUtxo>> {
        if !self.live.get() {
            return Ok(vec![]);
        }
        Ok(self.coins.clone())
    }
    fn scan_connectors(&self, _: &VaultConfig) -> Result<Vec<VaultUtxo>> {
        if !self.live.get() {
            return Ok(vec![]);
        }
        Ok(self.connectors.clone())
    }
    fn broadcast(&self, transaction: &Transaction) -> Result<Txid> {
        self.broadcasts.set(self.broadcasts.get() + 1);
        self.live.set(false);
        if self.lose_ack.replace(false) {
            anyhow::bail!("simulated accepted broadcast with lost response");
        }
        Ok(transaction.compute_txid())
    }
}
impl HotWalletBackend for OfflineBackend {
    fn sync_hot_wallet(&self, _: &mut HotWallet) -> Result<()> {
        Ok(())
    }
}

/// The vault outputs of a policy's rollover, as if it had confirmed at height 10. A later
/// rotation or renewal spends them, which is how the signer knows the old policy is replaced.
fn confirmed_policy(request: &PolicyRequest) -> OfflineBackend {
    let graph = graph(request);
    let tx = bitcoin_tx(&graph.rollover.transaction);
    let vault = &graph.rollover.transaction.output[graph.remainder_vout as usize].script_pubkey;
    OfflineBackend {
        coins: tx
            .output
            .iter()
            .enumerate()
            .filter(|(_, output)| output.script_pubkey.as_bytes() == vault.as_slice())
            .map(|(vout, output)| VaultUtxo {
                outpoint: OutPoint::new(tx.compute_txid(), vout as u32),
                txout: output.clone(),
                confirmation_height: 10,
            })
            .collect(),
        connectors: vec![],
        height: Cell::new(100),
        broadcasts: Cell::new(0),
        live: Cell::new(true),
        lose_ack: Cell::new(false),
    }
}

/// Vault or connector outputs of a proposed rollover, as if it had confirmed at height 10.
fn offline_fixture(devices: &Devices, action: OfflineAction) -> OfflineBackend {
    let policy = devices
        .phone
        .propose_policy(
            &TestBackend {
                broadcasts: Cell::new(0),
            },
            PolicyLimits {
                monthly_limit_sats: 10_000_000,
                emergency_access_limit_sats: 50_000_000,
            },
        )
        .unwrap();
    let graph = graph(&policy);
    let tx = bitcoin_tx(&graph.rollover.transaction);
    let indexes = match action {
        OfflineAction::HwwRecovery => vec![graph.remainder_vout],
        OfflineAction::RevokeActions => vec![
            graph.monthly_connector_vout.unwrap(),
            graph.emergency_connector_vout.unwrap(),
        ],
    };
    let coins = indexes
        .into_iter()
        .map(|vout| VaultUtxo {
            outpoint: OutPoint::new(tx.compute_txid(), vout),
            txout: tx.output[vout as usize].clone(),
            confirmation_height: 10,
        })
        .collect::<Vec<_>>();
    OfflineBackend {
        connectors: match action {
            OfflineAction::RevokeActions => coins.clone(),
            OfflineAction::HwwRecovery => vec![],
        },
        coins,
        height: Cell::new(100),
        broadcasts: Cell::new(0),
        live: Cell::new(true),
        lose_ack: Cell::new(false),
    }
}

#[test]
fn recovered_hot_wallet_retries_the_exact_transaction_after_a_lost_response() {
    let devices = Devices::paired();
    let contact = ContactFixture::enroll(&devices);
    let backup = devices.phone.export_backup().unwrap();
    let original_address = Address::from_str(&devices.phone.receive_address().unwrap())
        .unwrap()
        .require_network(Network::Regtest)
        .unwrap();
    // This fixture exercises state transitions and BDK signing, not node consensus.
    struct Backend {
        funding: Transaction,
        attempts: RefCell<Vec<Transaction>>,
        lose_response: Cell<bool>,
    }
    impl Blockchain for Backend {
        fn network(&self) -> Network {
            Network::Regtest
        }
        fn backend_description(&self) -> String {
            "recovered-wallet response-loss fixture".into()
        }
        fn chain_tip(&self) -> Result<ChainTip> {
            TestBackend {
                broadcasts: Cell::new(0),
            }
            .chain_tip()
        }
        fn scan_vault(&self, _: &VaultConfig) -> Result<Vec<VaultUtxo>> {
            Ok(vec![])
        }
        fn scan_connectors(&self, _: &VaultConfig) -> Result<Vec<VaultUtxo>> {
            Ok(vec![])
        }
        fn broadcast(&self, transaction: &Transaction) -> Result<Txid> {
            self.attempts.borrow_mut().push(transaction.clone());
            if self.lose_response.replace(false) {
                anyhow::bail!("accepted, then lost response");
            }
            Ok(transaction.compute_txid())
        }
    }
    impl HotWalletBackend for Backend {
        fn sync_hot_wallet(&self, wallet: &mut HotWallet) -> Result<()> {
            wallet
                .wallet
                .apply_unconfirmed_txs([(self.funding.clone(), 1)]);
            Ok(())
        }
    }
    let backend = Backend {
        funding: Transaction {
            version: bitcoin::transaction::Version::TWO,
            lock_time: bitcoin::absolute::LockTime::ZERO,
            input: vec![bitcoin::TxIn {
                previous_output: OutPoint::new(Txid::from_byte_array([81; 32]), 0),
                ..Default::default()
            }],
            output: vec![TxOut {
                value: Amount::from_sat(1_000_000),
                script_pubkey: original_address.script_pubkey(),
            }],
        },
        attempts: RefCell::new(vec![]),
        lose_response: Cell::new(true),
    };
    std::fs::remove_file(devices.phone_dir.path().join("phone/device.json")).unwrap();
    std::fs::remove_file(devices.signer_dir.path().join("hww/device.json")).unwrap();
    let directory = tempfile::tempdir().unwrap();
    let fresh = WalletApplication::new(directory.path().into(), false);
    let identity = fresh.initialize(Network::Regtest).unwrap();
    let request = fresh
        .propose_contact_restore(&backup, &contact.public)
        .unwrap();
    let review = contact.app.review_contact_restore(&request).unwrap();
    let response = contact
        .app
        .approve_contact_restore(&review.review_id, &request)
        .unwrap();
    fresh.store_contact_restored_key(&response).unwrap();
    let cancelled = fresh.review_recovered_wallet(&backend).unwrap();
    fresh.reject_recovered_wallet(&cancelled.review_id).unwrap();
    assert!(
        fresh
            .confirm_recovered_wallet(&backend, &cancelled.review_id)
            .is_err()
    );
    assert!(backend.attempts.borrow().is_empty());
    let review = fresh.review_recovered_wallet(&backend).unwrap();
    assert_eq!(review.amount_sats + review.fee_sats, 1_000_000);
    assert!(
        fresh
            .confirm_recovered_wallet(&backend, &review.review_id)
            .unwrap_err()
            .to_string()
            .contains("lost response")
    );
    assert!(
        fresh
            .reject_recovered_wallet(&review.review_id)
            .unwrap_err()
            .to_string()
            .contains("submission has started")
    );
    let signed_path = directory
        .path()
        .join("phone/recovered-wallet")
        .join(&review.review_id)
        .join("signed.json");
    let original = std::fs::read(&signed_path).unwrap();
    let mut altered: Transaction = serde_json::from_slice(&original).unwrap();
    altered.output[0].value = Amount::from_sat(review.amount_sats - 1);
    std::fs::write(&signed_path, serde_json::to_vec(&altered).unwrap()).unwrap();
    assert!(
        fresh
            .confirm_recovered_wallet(&backend, &review.review_id)
            .unwrap_err()
            .to_string()
            .contains("changed since review")
    );
    assert_eq!(backend.attempts.borrow().len(), 1);
    std::fs::write(&signed_path, original).unwrap();
    let reopened = WalletApplication::new(directory.path().into(), false);
    let txid = reopened
        .confirm_recovered_wallet(&backend, &review.review_id)
        .unwrap();
    assert_eq!(backend.attempts.borrow().len(), 2);
    assert_eq!(backend.attempts.borrow()[0], backend.attempts.borrow()[1]);
    assert_eq!(
        reopened
            .confirm_recovered_wallet(&backend, &review.review_id)
            .unwrap(),
        txid
    );
    assert_eq!(
        backend.attempts.borrow().len(),
        2,
        "receipt prevents another broadcast"
    );
    assert!(reopened.reject_recovered_wallet(&review.review_id).is_err());
    assert_eq!(
        reopened.public_identity().unwrap().vault_pubkey,
        identity.vault_pubkey
    );
}

#[test]
fn protocol_outboxes_survive_restart_without_approving_or_broadcasting() {
    let devices = Devices::paired();
    let backend = TestBackend {
        broadcasts: Cell::new(0),
    };
    assert!(devices.phone.outbox().unwrap().is_empty());
    assert!(devices.signer.outbox().unwrap().is_empty());
    let proposal = devices
        .phone
        .execute(
            WalletRequest::ProposePolicy {
                monthly_limit_sats: 10_000_000,
                emergency_access_limit_sats: 50_000_000,
            },
            Some(&backend),
        )
        .unwrap();
    let items = devices.phone.outbox().unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].kind, OutgoingMessageKind::PolicyProposal);
    let reopened = WalletApplication::new(devices.phone_dir.path().into(), false);
    let message = reopened.read_outbox(&items[0].id).unwrap();
    assert_eq!(message.body, proposal);
    assert_eq!(backend.broadcasts.get(), 0);
    assert!(!devices.phone_dir.path().join("hww/device.json").exists());
    assert!(devices.signer.read_outbox(&items[0].id).is_err());

    let review = devices
        .signer
        .execute(SignerRequest::ReviewPolicy {
            request: Box::new(serde_json::from_value(message.body.clone()).unwrap()),
        })
        .unwrap();
    let review_id = review["review_id"].as_str().unwrap().to_owned();
    assert!(
        devices.signer.outbox().unwrap().is_empty(),
        "review must not emit an approval"
    );
    devices
        .signer
        .execute(SignerRequest::RejectPolicy {
            review_id: review_id.clone(),
        })
        .unwrap();
    assert!(
        devices
            .signer
            .execute(SignerRequest::ApprovePolicy {
                review_id: review_id.clone()
            })
            .is_err()
    );
    assert!(
        devices.signer.outbox().unwrap().is_empty(),
        "failed/rejected operations must not emit messages"
    );

    // Resending the retained request asks for a new explicit review, not an old approval.
    devices
        .signer
        .execute(SignerRequest::ReviewPolicy {
            request: Box::new(serde_json::from_value(message.body).unwrap()),
        })
        .unwrap();
    let approved = devices
        .signer
        .execute(SignerRequest::ApprovePolicy { review_id })
        .unwrap();
    let signer = SignerApplication::new(devices.signer_dir.path().into(), false);
    let signed = signer.outbox().unwrap();
    assert_eq!(signed.len(), 1);
    assert_eq!(signed[0].kind, OutgoingMessageKind::PolicyApproved);
    let message = signer.read_outbox(&signed[0].id).unwrap();
    assert_eq!(message.body, approved);
    assert_eq!(
        backend.broadcasts.get(),
        0,
        "opening a saved approval cannot broadcast"
    );
    reopened
        .execute(
            WalletRequest::ActivatePolicy {
                approval: Box::new(serde_json::from_value(message.body).unwrap()),
            },
            Some(&backend),
        )
        .unwrap();
    let count = backend.broadcasts.get();
    assert!(count > 0);
    signer.read_outbox(&signed[0].id).unwrap();
    assert_eq!(backend.broadcasts.get(), count);

    // Independent devices cannot use each other's saved-message records, even if a host
    // incorrectly copies a file. Nor can a caller choose a path outside the outbox.
    let stranger = Devices::paired();
    let destination = stranger.phone_dir.path().join("phone/outbox");
    std::fs::create_dir_all(&destination).unwrap();
    let source = devices
        .phone_dir
        .path()
        .join("phone/outbox")
        .join(format!("{}.json", items[0].id));
    std::fs::copy(&source, destination.join(format!("{}.json", items[0].id))).unwrap();
    assert!(stranger.phone.read_outbox(&items[0].id).is_err());
    for id in ["../../phone/device", "", &"A".repeat(64)] {
        assert!(reopened.read_outbox(id).is_err());
    }
    let mut record: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&source).unwrap()).unwrap();
    record["message"]["body"]["limits"]["monthly_limit_sats"] = serde_json::json!(1);
    std::fs::write(source, serde_json::to_vec(&record).unwrap()).unwrap();
    assert!(reopened.read_outbox(&items[0].id).is_err());
    assert!(
        reopened.outbox().is_err(),
        "listing cannot silently trust altered metadata"
    );
}

#[test]
fn signer_preserves_signed_recovery_policy_without_claiming_chain_activation() {
    let devices = Devices::paired();
    let initial = devices.signer.export_recovery_bundle().unwrap();
    assert!(initial.last_approved_policy.is_none());
    let backend = TestBackend {
        broadcasts: Cell::new(0),
    };
    let proposal = devices
        .phone
        .propose_policy(
            &backend,
            PolicyLimits {
                monthly_limit_sats: 8_000_000,
                emergency_access_limit_sats: 25_000_000,
            },
        )
        .unwrap();
    let review = devices.signer.review_policy(&proposal).unwrap();
    devices.signer.reject_policy(&review.review_id).unwrap();
    assert!(
        devices
            .signer
            .export_recovery_bundle()
            .unwrap()
            .last_approved_policy
            .is_none()
    );
    let review = devices.signer.review_policy(&proposal).unwrap();
    devices.signer.approve_policy(&review.review_id).unwrap();
    // No broadcasts have occurred: this is last-approved evidence, not active chain state.
    assert_eq!(backend.broadcasts.get(), 0);
    let bundle = devices.signer.export_recovery_bundle().unwrap();
    assert_eq!(bundle.config.monthly_limit_sats, 8_000_000);
    assert_eq!(bundle.config.emergency_access_limit_sats, 25_000_000);
    bundle.validate().unwrap();
    let replacement_dir = tempfile::tempdir().unwrap();
    let replacement = WalletApplication::new(replacement_dir.path().into(), false);
    let identity = replacement.initialize(Network::Regtest).unwrap();
    replacement.import_recovery_bundle(&bundle).unwrap();
    assert_eq!(
        replacement.public_identity().unwrap().vault_pubkey,
        identity.vault_pubkey
    );
    assert_eq!(
        replacement.recovery_source().unwrap().monthly_limit_sats,
        8_000_000
    );

    let mut tampered = bundle.clone();
    tampered.config.monthly_limit_sats = 0;
    assert!(replacement.import_recovery_bundle(&tampered).is_err());
    let mut tampered = bundle.clone();
    tampered.last_approved_policy = None;
    assert!(replacement.import_recovery_bundle(&tampered).is_err());
    let mut tampered = bundle.clone();
    let policy = tampered.last_approved_policy.as_mut().unwrap();
    policy.signatures[0] = policy.signatures[1];
    assert!(replacement.import_recovery_bundle(&tampered).is_err());
    assert_eq!(
        replacement.recovery_source().unwrap().monthly_limit_sats,
        8_000_000
    );
    // A later explicit zero policy supersedes the retained limits, also without a broadcast.
    let zero = devices
        .phone
        .propose_policy(
            &backend,
            PolicyLimits {
                monthly_limit_sats: 0,
                emergency_access_limit_sats: 0,
            },
        )
        .unwrap();
    let review = devices.signer.review_policy(&zero).unwrap();
    devices.signer.approve_policy(&review.review_id).unwrap();
    assert_eq!(
        devices
            .signer
            .export_recovery_bundle()
            .unwrap()
            .config
            .monthly_limit_sats,
        0
    );
}

// Supplies complete previous transactions while keeping consensus simulation explicit.
struct RotationBackend(OfflineBackend);
impl Blockchain for RotationBackend {
    fn network(&self) -> Network {
        Network::Regtest
    }
    fn backend_description(&self) -> String {
        "rotation fixture, not consensus verification".into()
    }
    fn chain_tip(&self) -> Result<ChainTip> {
        self.0.chain_tip()
    }
    fn scan_vault(&self, config: &VaultConfig) -> Result<Vec<VaultUtxo>> {
        self.0.scan_vault(config)
    }
    fn scan_connectors(&self, _: &VaultConfig) -> Result<Vec<VaultUtxo>> {
        Ok(vec![])
    }
    fn previous_transaction(&self, utxo: &VaultUtxo) -> Result<Transaction> {
        self.0.previous_transaction(utxo)
    }
    fn broadcast(&self, tx: &Transaction) -> Result<Txid> {
        self.0.broadcast(tx)
    }
}
impl HotWalletBackend for RotationBackend {
    fn sync_hot_wallet(&self, _: &mut HotWallet) -> Result<()> {
        Ok(())
    }
}

#[test]
fn restored_rotation_is_public_only_preserves_policy_and_retries_the_same_spends() {
    check_restored_rotation(8_000_000, 25_000_000, false);
}

#[test]
fn lost_signer_replacement_preserves_policy_and_cannot_bypass_phone_recovery() {
    check_signer_replacement(8_000_000, 25_000_000, false);
}

#[test]
fn lost_signer_replacement_keeps_a_disabled_policy_disabled() {
    check_signer_replacement(0, 0, false);
}

#[test]
fn restored_rotation_preserves_authenticated_contacts_and_their_ability_to_recover() {
    check_restored_rotation(8_000_000, 25_000_000, true);
}

#[test]
fn lost_signer_replacement_preserves_contacts_and_their_ability_to_recover() {
    check_signer_replacement(8_000_000, 25_000_000, true);
}

#[test]
fn signer_replacement_moves_mature_cohorts_and_recovers_later_deposits_after_restart() {
    let devices = Devices::paired();
    let mut source = RotationBackend(offline_fixture(&devices, OfflineAction::HwwRecovery));
    let oldest = source.0.coins[0].clone();
    let youngest = VaultUtxo {
        outpoint: OutPoint::new(Txid::from_byte_array([23; 32]), oldest.outpoint.vout),
        confirmation_height: 20,
        ..oldest.clone()
    };
    let coin_sats = oldest.txout.value.to_sat();
    source.0.coins.push(youngest.clone());
    source.0.height.set(61_208); // Oldest is one block early; the later deposit must not postpone it.
    let directory = tempfile::tempdir().unwrap();
    let signer = SignerApplication::new(directory.path().into(), false);
    let identity = signer.initialize(Network::Regtest).unwrap();
    let candidate = devices
        .phone
        .inspect_signer_replacement(&source, &identity)
        .unwrap();
    assert_eq!(candidate.recovery_next_block_height, Some(61_210));
    assert_eq!(candidate.recovery_blocks_remaining, Some(1));
    assert_eq!(candidate.selected_input_count, 1);
    assert_eq!(candidate.selected_sats, coin_sats);
    assert_eq!(candidate.total_sats, coin_sats * 2);
    let request = devices
        .phone
        .propose_signer_replacement(&source, &identity)
        .unwrap();
    assert_eq!(request.replacement.recovery_inputs.len(), 1);
    assert_eq!(
        request.replacement.recovery_inputs[0].outpoint.to_string(),
        oldest.outpoint.to_string()
    );
    let review = signer.review_signer_replacement(&request).unwrap();
    assert_eq!(review.input_count, 1);
    let approved = signer
        .approve_signer_replacement(&review.review_id, &request)
        .unwrap();
    assert!(
        devices
            .phone
            .activate_signer_replacement(&source, &approved)
            .map(drop)
            .expect_err("oldest input is one block early")
            .to_string()
            .contains("not mature")
    );
    assert_eq!(source.0.broadcasts.get(), 0);
    source.0.height.set(61_209);
    source.0.live.set(false);
    assert!(
        devices
            .phone
            .activate_signer_replacement(&source, &approved)
            .map(drop)
            .expect_err("input was spent")
            .to_string()
            .contains("no longer confirmed and unspent")
    );
    assert_eq!(source.0.broadcasts.get(), 0);
    source.0.live.set(true);
    devices
        .phone
        .activate_signer_replacement(&source, &approved)
        .unwrap();

    // The fixture now exposes only the young input. The real-node browser test checks the
    // actual UTXO transition; this fixture isolates review/retry/persistence failures.
    source.0.coins = vec![youngest.clone()];
    source.0.live.set(true);
    let reopened = WalletApplication::new(devices.phone_dir.path().into(), false);
    let current = reopened.configuration().unwrap();
    assert_ne!(current.vault_address, devices.config.vault_address);
    assert_eq!(
        current.phone_vault_pubkey,
        devices.config.phone_vault_pubkey
    );
    let retired = reopened.previous_vaults(&source).unwrap();
    assert_eq!(retired.len(), 1);
    let retired = &retired[0];
    assert_eq!(retired.vault_address, devices.config.vault_address);
    assert!(retired.can_recover);
    assert_eq!(retired.phone.total_sats, coin_sats);
    assert_eq!(retired.phone.blocks_remaining, Some(10));
    assert_eq!(retired.hww.blocks_remaining, Some(4_345));
    assert!(
        reopened
            .review_previous_vault_recovery(&source, "../device")
            .is_err()
    );
    let first = reopened
        .review_previous_vault_recovery(&source, &retired.vault_id)
        .unwrap();
    assert_eq!(first.destination, current.vault_address);
    assert_eq!(first.source_address, devices.config.vault_address);
    assert_eq!(first.input_count, 1);
    assert_eq!(first.amount_sats + first.fee_sats, coin_sats);
    assert_eq!(first.blocks_remaining, 10);
    reopened
        .reject_previous_vault_recovery(&first.review_id)
        .unwrap();
    assert!(
        reopened
            .confirm_previous_vault_recovery(&source, &first.review_id)
            .is_err()
    );
    let next = reopened
        .review_previous_vault_recovery(&source, &retired.vault_id)
        .unwrap();
    let review_path = devices
        .phone_dir
        .path()
        .join("phone/previous-vault-sweeps")
        .join(&next.review_id)
        .join("review.json");
    let original = std::fs::read(&review_path).unwrap();
    let mut changed: serde_json::Value = serde_json::from_slice(&original).unwrap();
    // The review fixes the exact transaction ID; nothing is signed until confirmation.
    assert!(changed["transfer"]["txid"].is_string());
    assert!(changed["transfer"].get("psbt").is_none());
    changed["destination"] = serde_json::json!(devices.config.vault_address);
    std::fs::write(&review_path, serde_json::to_vec(&changed).unwrap()).unwrap();
    assert!(
        reopened
            .confirm_previous_vault_recovery(&source, &next.review_id)
            .is_err()
    );
    std::fs::write(&review_path, original).unwrap();
    let config_path = devices.phone_dir.path().join("anzen.json");
    let current_bytes = std::fs::read(&config_path).unwrap();
    std::fs::write(&config_path, serde_json::to_vec(&devices.config).unwrap()).unwrap();
    assert!(
        reopened
            .confirm_previous_vault_recovery(&source, &next.review_id)
            .expect_err("current destination changed")
            .to_string()
            .contains("changed since review")
    );
    std::fs::write(&config_path, current_bytes).unwrap();
    source.0.height.set(61_218);
    assert!(
        reopened
            .confirm_previous_vault_recovery(&source, &next.review_id)
            .expect_err("late deposit is one block early")
            .to_string()
            .contains("not mature")
    );
    source.0.height.set(61_219);
    source.0.live.set(false);
    assert!(
        reopened
            .confirm_previous_vault_recovery(&source, &next.review_id)
            .expect_err("late deposit was spent")
            .to_string()
            .contains("no longer confirmed and unspent")
    );
    source.0.live.set(true);
    source.0.lose_ack.set(true);
    assert!(
        reopened
            .confirm_previous_vault_recovery(&source, &next.review_id)
            .expect_err("accepted broadcast lost its response")
            .to_string()
            .contains("lost response")
    );
    assert!(
        reopened
            .reject_previous_vault_recovery(&next.review_id)
            .is_err()
    );
    let reopened = WalletApplication::new(devices.phone_dir.path().into(), false);
    let txid = reopened
        .confirm_previous_vault_recovery(&source, &next.review_id)
        .unwrap();
    let calls = source.0.broadcasts.get();
    assert_eq!(
        reopened
            .confirm_previous_vault_recovery(&source, &next.review_id)
            .unwrap(),
        txid
    );
    assert_eq!(source.0.broadcasts.get(), calls);
    let retired = reopened.previous_vaults(&source).unwrap();
    assert_eq!(retired[0].phone.total_sats, 0);
    assert_eq!(retired[0].phone.blocks_remaining, None);
    assert!(
        reopened
            .review_previous_vault_recovery(&source, &retired[0].vault_id)
            .is_err()
    );
    assert_eq!(
        reopened.configuration().unwrap().vault_address,
        current.vault_address
    );

    // Monitoring is retained even when empty: later sends to the retired address reappear.
    source.0.live.set(true);
    source.0.coins[0].confirmation_height = 61_219;
    let retired = reopened.previous_vaults(&source).unwrap();
    assert_eq!(retired[0].phone.total_sats, coin_sats);
    assert_eq!(retired[0].phone.blocks_remaining, Some(61_199));
    assert_eq!(retired[0].hww.blocks_remaining, Some(65_534));
}

fn check_signer_replacement(
    monthly_limit_sats: u64,
    emergency_access_limit_sats: u64,
    with_contact: bool,
) {
    let devices = Devices::paired();
    let backend = TestBackend {
        broadcasts: Cell::new(0),
    };
    if monthly_limit_sats > 0 || emergency_access_limit_sats > 0 {
        let policy = devices
            .phone
            .propose_policy(
                &backend,
                PolicyLimits {
                    monthly_limit_sats,
                    emergency_access_limit_sats,
                },
            )
            .unwrap();
        let review = devices.signer.review_policy(&policy).unwrap();
        let approved = devices.signer.approve_policy(&review.review_id).unwrap();
        devices.phone.activate_policy(&backend, &approved).unwrap();
    }
    let contact = with_contact.then(|| ContactFixture::enroll(&devices));
    let source = RotationBackend(offline_fixture(&devices, OfflineAction::HwwRecovery));
    let old_config = devices.phone.configuration().unwrap();
    let phone_identity = devices.phone.public_identity().unwrap();
    let original_device =
        std::fs::read(devices.phone_dir.path().join("phone/device.json")).unwrap();
    let replacement_dir = tempfile::tempdir().unwrap();
    let replacement = SignerApplication::new(replacement_dir.path().into(), false);
    let replacement_identity = replacement.initialize(Network::Regtest).unwrap();
    let mut wrong_identity = replacement_identity.clone();
    wrong_identity.vault_pubkey = phone_identity.vault_pubkey.clone();
    assert!(
        devices
            .phone
            .propose_signer_replacement(&source, &wrong_identity)
            .is_err()
    );
    // The unavailable device is actually inaccessible to this ceremony, not copied to its replacement.
    std::fs::remove_file(devices.signer_dir.path().join("hww/device.json")).unwrap();
    let candidate = devices
        .phone
        .inspect_signer_replacement(&source, &replacement_identity)
        .unwrap();
    assert_eq!(candidate.recovery_next_block_height, Some(61_210));
    let request = devices
        .phone
        .propose_signer_replacement(&source, &replacement_identity)
        .unwrap();
    assert_eq!(
        request.new_config.phone_vault_pubkey,
        phone_identity.vault_pubkey
    );
    assert_eq!(
        request.new_config.hww_vault_pubkey,
        replacement_identity.vault_pubkey
    );
    assert_eq!(request.new_config.monthly_limit_sats, monthly_limit_sats);
    let json = serde_json::to_string(&request).unwrap();
    assert!(!json.contains("mnemonic"));
    assert!(!json.contains("encrypted_payload"));
    assert!(!json.contains("symmetric_key"));
    let review = replacement.review_signer_replacement(&request).unwrap();
    assert_eq!(
        review.friend_fingerprints,
        contact
            .as_ref()
            .map(|c| c.public.fingerprint.clone())
            .into_iter()
            .collect::<Vec<_>>()
    );
    assert_eq!(review.recovery_delay_blocks, 61_200);
    assert_eq!(review.monthly_limit_sats, monthly_limit_sats);
    replacement
        .reject_signer_replacement(&review.review_id)
        .unwrap();
    assert!(
        replacement
            .approve_signer_replacement(&review.review_id, &request)
            .is_err()
    );
    let mut wrong = request.clone();
    wrong.backup_request.payload.ciphertext[0] ^= 1;
    assert!(replacement.review_signer_replacement(&wrong).is_err());
    if let Some(policy) = &request.replacement.renewed_policy {
        // The renewed policy must spend exactly the phone's recovery output.
        let mut wrong = request.clone();
        wrong
            .replacement
            .renewed_policy
            .as_mut()
            .unwrap()
            .vault_inputs[0]
            .value_sats += 1;
        assert!(replacement.review_signer_replacement(&wrong).is_err());
        let mut wrong = request.clone();
        wrong.replacement.renewed_policy.as_mut().unwrap().limits =
            anzen_protocol::policy::Limits {
                monthly_limit_sats: policy.limits.monthly_limit_sats + 1,
                ..policy.limits
            };
        assert!(replacement.review_signer_replacement(&wrong).is_err());
    }
    let mut wrong = request.clone();
    wrong.new_config.monthly_limit_sats += 1;
    assert!(replacement.review_signer_replacement(&wrong).is_err());
    let mut wrong = request.clone();
    let duplicate = wrong.replacement.recovery_inputs[0];
    wrong.replacement.recovery_inputs.push(duplicate);
    assert!(replacement.review_signer_replacement(&wrong).is_err());
    let review = replacement.review_signer_replacement(&request).unwrap();
    assert!(
        replacement
            .approve_signer_replacement(&review.review_id, &wrong)
            .is_err()
    );
    let approval = replacement
        .approve_signer_replacement(&review.review_id, &request)
        .unwrap();
    assert_eq!(source.0.broadcasts.get(), 0);
    assert!(
        devices
            .phone
            .activate_signer_replacement(&source, &approval)
            .is_err()
    );
    source.0.height.set(61_208); // Still one block too early for next-block validity.
    assert!(
        devices
            .phone
            .activate_signer_replacement(&source, &approval)
            .is_err()
    );
    assert_eq!(source.0.broadcasts.get(), 0);
    assert_eq!(
        devices.phone.configuration().unwrap().vault_address,
        old_config.vault_address
    );
    source.0.height.set(61_209);
    source.0.lose_ack.set(true);
    assert!(
        devices
            .phone
            .activate_signer_replacement(&source, &approval)
            .is_err()
    );
    assert_eq!(
        devices.phone.configuration().unwrap().vault_address,
        old_config.vault_address
    );
    let result = devices
        .phone
        .activate_signer_replacement(&source, &approval)
        .unwrap();
    let count = source.0.broadcasts.get();
    assert_eq!(
        devices
            .phone
            .activate_signer_replacement(&source, &approval)
            .unwrap()
            .sweep_txid,
        result.sweep_txid
    );
    assert_eq!(source.0.broadcasts.get(), count);
    assert_eq!(
        std::fs::read(devices.phone_dir.path().join("phone/device.json")).unwrap(),
        original_device
    );
    assert_eq!(
        devices.phone.configuration().unwrap().monthly_limit_sats,
        monthly_limit_sats
    );
    assert_eq!(
        devices
            .phone
            .configuration()
            .unwrap()
            .emergency_access_limit_sats,
        emergency_access_limit_sats
    );
    assert_eq!(
        result.renewed_schedule.is_some(),
        monthly_limit_sats > 0 || emergency_access_limit_sats > 0
    );
    assert!(devices.phone.export_backup().is_ok());
    if let Some(contact) = &contact {
        contact.assert_recovers(
            &devices.phone.export_backup().unwrap(),
            &devices.phone.configuration().unwrap(),
        );
    }
    assert!(
        !replacement_dir
            .path()
            .join("hww/vault-config.json")
            .exists()
    );
    replacement
        .adopt_signer_replacement(&result.request_id)
        .unwrap();
    assert_eq!(
        replacement
            .export_recovery_bundle()
            .unwrap()
            .config
            .vault_address,
        result.new_vault_address
    );
    assert!(!replacement_dir.path().join("phone").exists());
    assert!(!replacement_dir.path().join("cloud").exists());
    assert!(!devices.phone_dir.path().join("hww/device.json").exists());
    // A delayed completion message must not erase a later policy change on the new signer.
    let renewed_chain = request
        .replacement
        .renewed_policy
        .as_ref()
        .map(confirmed_policy);
    let chain: &dyn HotWalletBackend = match &renewed_chain {
        Some(chain) => chain,
        None => &backend,
    };
    let next = devices
        .phone
        .propose_policy(
            chain,
            PolicyLimits {
                monthly_limit_sats: 7_000_000,
                emergency_access_limit_sats: 20_000_000,
            },
        )
        .unwrap();
    let review = replacement.review_policy(&next).unwrap();
    replacement.approve_policy(&review.review_id).unwrap();
    assert_eq!(
        replacement
            .adopt_signer_replacement(&result.request_id)
            .unwrap()
            .monthly_limit_sats,
        7_000_000
    );
}

#[test]
fn restored_rotation_preserves_a_disabled_policy_without_creating_presigned_withdrawals() {
    check_restored_rotation(0, 0, false);
}

fn check_restored_rotation(
    monthly_limit_sats: u64,
    emergency_access_limit_sats: u64,
    with_contact: bool,
) {
    let devices = Devices::paired();
    let policy = devices
        .phone
        .propose_policy(
            &TestBackend {
                broadcasts: Cell::new(0),
            },
            PolicyLimits {
                monthly_limit_sats,
                emergency_access_limit_sats,
            },
        )
        .unwrap();
    let review = devices.signer.review_policy(&policy).unwrap();
    devices.signer.approve_policy(&review.review_id).unwrap();
    let source = RotationBackend(confirmed_policy(&policy));
    let contact = with_contact.then(|| ContactFixture::enroll(&devices));
    let request = devices.phone.propose_backup().unwrap();
    let review = devices.signer.review_backup(&request).unwrap();
    let approval = devices
        .signer
        .approve_backup(&review.review_id, &request)
        .unwrap();
    devices.phone.store_backup(&approval).unwrap();
    let cloud = devices.phone.export_backup().unwrap();
    let replacement_dir = tempfile::tempdir().unwrap();
    let phone = WalletApplication::new(replacement_dir.path().into(), false);
    let new_identity = phone.initialize(Network::Regtest).unwrap();
    phone
        .import_recovery_bundle(&devices.signer.export_recovery_bundle().unwrap())
        .unwrap();
    let request = phone.propose_restore(&cloud).unwrap();
    let review = devices.signer.review_restore(&request).unwrap();
    phone
        .store_restored_key(&devices.signer.approve_restore(&review.review_id).unwrap())
        .unwrap();
    // Prove that the operation does not read either private key from the old combined CLI layout.
    std::fs::remove_file(devices.phone_dir.path().join("phone/device.json")).unwrap();
    if with_contact {
        // An unapproved replacement cloud object must not silently change the authenticated
        // contact list that will be used by a subsequent rotation.
        let mut changed_cloud = cloud.clone();
        changed_cloud.friends.clear();
        phone.propose_restore(&changed_cloud).unwrap();
    }
    let request = phone.propose_restored_rotation(&source).unwrap();
    let request_json = serde_json::to_string(&request).unwrap();
    assert!(!request_json.contains("phone_mnemonic"));
    assert!(!request_json.contains("encrypted_payload"));
    assert!(!request_json.contains("symmetric_key"));
    let review = devices.signer.review_phone_rotation(&request).unwrap();
    assert_eq!(
        review.friend_fingerprints,
        contact
            .as_ref()
            .map(|c| c.public.fingerprint.clone())
            .into_iter()
            .collect::<Vec<_>>()
    );
    assert_eq!(review.monthly_limit_sats, monthly_limit_sats);
    assert_eq!(
        review.emergency_access_limit_sats,
        emergency_access_limit_sats
    );
    assert_eq!(review.new_phone_public_key, new_identity.vault_pubkey);
    let path = devices
        .signer_dir
        .path()
        .join("hww/rotations/reviews")
        .join(format!("{}.json", review.review_id));
    assert!(
        !std::fs::read_to_string(path)
            .unwrap()
            .contains("backup_request")
    );
    devices
        .signer
        .reject_phone_rotation(&review.review_id)
        .unwrap();
    assert!(
        devices
            .signer
            .approve_phone_rotation(&review.review_id, &request)
            .is_err()
    );

    let mut wrong = request.clone();
    wrong.backup_request.payload.ciphertext[0] ^= 1;
    assert!(devices.signer.review_phone_rotation(&wrong).is_err());
    let mut wrong = request.clone();
    wrong.new_config.monthly_limit_sats = monthly_limit_sats + 1;
    assert!(devices.signer.review_phone_rotation(&wrong).is_err());
    let mut wrong = request.clone();
    wrong.rotation.renewed_policy = if request.rotation.renewed_policy.is_some() {
        None
    } else {
        Some(policy.clone())
    };
    assert!(devices.signer.review_phone_rotation(&wrong).is_err());
    let mut wrong = request.clone();
    wrong.rotation.sweep_inputs.clear();
    assert!(devices.signer.review_phone_rotation(&wrong).is_err());
    let mut wrong = request.clone();
    wrong.rotation.new_phone = anzen_protocol::script::XOnlyKey(
        bitcoin::secp256k1::XOnlyPublicKey::from_str(&devices.config.hww_vault_pubkey)
            .unwrap()
            .serialize(),
    );
    assert!(devices.signer.review_phone_rotation(&wrong).is_err());
    let review = devices.signer.review_phone_rotation(&request).unwrap();
    assert!(
        devices
            .signer
            .approve_phone_rotation(&review.review_id, &wrong)
            .is_err()
    );
    let approval = devices
        .signer
        .approve_phone_rotation(&review.review_id, &request)
        .unwrap();
    assert_eq!(source.0.broadcasts.get(), 0);
    let mut changed = approval.clone();
    changed.new_config.monthly_limit_sats = monthly_limit_sats + 1;
    assert!(phone.activate_rotation(&source, &changed).is_err());
    let mut changed = approval.clone();
    changed.approval.sweep_signatures[0].0[0] ^= 1;
    assert!(phone.activate_rotation(&source, &changed).is_err());
    assert_eq!(source.0.broadcasts.get(), 0);
    source.0.lose_ack.set(true);
    assert!(phone.activate_rotation(&source, &approval).is_err());
    assert!(!replacement_dir.path().join("anzen.json").exists());
    let result = phone.activate_rotation(&source, &approval).unwrap();
    let broadcasts = source.0.broadcasts.get();
    let retry = phone.activate_rotation(&source, &approval).unwrap();
    assert_eq!(result.sweep_txid, retry.sweep_txid);
    assert_eq!(source.0.broadcasts.get(), broadcasts);
    assert_eq!(
        phone.configuration().unwrap().monthly_limit_sats,
        monthly_limit_sats
    );
    assert_eq!(
        result.renewed_schedule.is_some(),
        monthly_limit_sats > 0 || emergency_access_limit_sats > 0
    );
    assert_eq!(
        phone.public_identity().unwrap().vault_pubkey,
        new_identity.vault_pubkey
    );
    assert_ne!(result.new_vault_address, devices.config.vault_address);
    assert!(phone.export_backup().is_ok());
    if let Some(contact) = &contact {
        contact.assert_recovers(
            &phone.export_backup().unwrap(),
            &phone.configuration().unwrap(),
        );
    }
    // No automatic pairing change on the signer: adoption is a separate explicit user action.
    assert_eq!(
        devices
            .signer
            .export_recovery_bundle()
            .unwrap()
            .config
            .vault_address,
        devices.config.vault_address
    );
    devices
        .signer
        .adopt_phone_rotation(&result.request_id)
        .unwrap();
    assert_eq!(
        devices
            .signer
            .export_recovery_bundle()
            .unwrap()
            .config
            .vault_address,
        result.new_vault_address
    );
    assert_eq!(
        devices
            .signer
            .export_recovery_bundle()
            .unwrap()
            .config
            .monthly_limit_sats,
        monthly_limit_sats
    );
    assert!(!devices.signer_dir.path().join("phone").exists());
    assert!(!devices.signer_dir.path().join("cloud").exists());
    assert!(!replacement_dir.path().join("hww/device.json").exists());
    // The renewed rollover's outputs are the live vault a later policy change spends.
    let renewed_chain = request
        .rotation
        .renewed_policy
        .as_ref()
        .map(confirmed_policy);
    let plain = TestBackend {
        broadcasts: Cell::new(0),
    };
    let chain: &dyn HotWalletBackend = match &renewed_chain {
        Some(backend) => backend,
        None => &plain,
    };
    let changed_policy = phone
        .propose_policy(
            chain,
            PolicyLimits {
                monthly_limit_sats: 7_000_000,
                emergency_access_limit_sats: 20_000_000,
            },
        )
        .unwrap();
    let changed_review = devices.signer.review_policy(&changed_policy).unwrap();
    devices
        .signer
        .approve_policy(&changed_review.review_id)
        .unwrap();
    // Re-delivery of an old completion message cannot roll back a later signed policy.
    assert_eq!(
        devices
            .signer
            .adopt_phone_rotation(&result.request_id)
            .unwrap()
            .monthly_limit_sats,
        7_000_000
    );
    assert_eq!(
        devices
            .signer
            .export_recovery_bundle()
            .unwrap()
            .config
            .monthly_limit_sats,
        7_000_000
    );
}

#[test]
fn offline_signer_rejects_unbound_tampered_and_unreviewed_actions() {
    let devices = Devices::paired();
    let backend = offline_fixture(&devices, OfflineAction::RevokeActions);
    let destination = devices.phone.receive_address().unwrap();
    let request = devices
        .phone
        .propose_offline_action(&backend, OfflineAction::RevokeActions, &destination)
        .unwrap();
    let mut duplicate = request.clone();
    duplicate.inputs.push(duplicate.inputs[0]);
    assert!(devices.signer.review_offline_action(&duplicate).is_err());
    let mut missing = request.clone();
    missing.inputs.clear();
    assert!(devices.signer.review_offline_action(&missing).is_err());
    let mut wrong_network = request.clone();
    wrong_network.destination =
        "bc1p5d7rjq7g6rdk2yhzks9smlaqtedr4dekq08ge8ztwac72sfr9rusxg3297".into();
    assert!(
        devices
            .signer
            .review_offline_action(&wrong_network)
            .is_err()
    );
    let mut wrong_vault = request.clone();
    wrong_vault.keys.phone = wrong_vault.keys.hww;
    assert!(devices.signer.review_offline_action(&wrong_vault).is_err());
    let mut dust = request.clone();
    dust.inputs.truncate(1);
    dust.inputs[0].value_sats = 400;
    assert!(devices.signer.review_offline_action(&dust).is_err());
    assert!(
        devices
            .signer
            .approve_offline_action(&"0".repeat(64))
            .is_err()
    );
    let review = devices.signer.review_offline_action(&request).unwrap();
    assert_eq!(review.input_count, 2);
    assert_eq!(review.amount_sats + review.fee_sats, 2000);
    assert!(review.recovery_delay_blocks.is_none());
    devices
        .signer
        .reject_offline_action(&review.review_id)
        .unwrap();
    assert!(
        devices
            .signer
            .approve_offline_action(&review.review_id)
            .is_err()
    );
}

#[test]
fn replacement_phone_coordinates_both_hww_actions_without_the_lost_phone_key() {
    for action in [OfflineAction::RevokeActions, OfflineAction::HwwRecovery] {
        let devices = Devices::paired();
        let backend = offline_fixture(&devices, action);
        let replacement_dir = tempfile::tempdir().unwrap();
        let replacement = WalletApplication::new(replacement_dir.path().into(), false);
        replacement.initialize(Network::Regtest).unwrap();
        replacement.import_recovery_vault(&devices.config).unwrap();
        assert!(replacement.configuration().is_err()); // Watch-only import does not pair/overwrite.
        std::fs::remove_file(devices.phone_dir.path().join("phone/device.json")).unwrap();
        let destination = replacement.receive_address().unwrap();
        let proposal = replacement
            .propose_offline_action(&backend, action, &destination)
            .unwrap();
        let review = devices.signer.review_offline_action(&proposal).unwrap();
        let approved = devices
            .signer
            .approve_offline_action(&review.review_id)
            .unwrap();
        let mut tampered = approved.clone();
        tampered.signatures[0].0[0] ^= 1;
        assert!(
            replacement
                .activate_offline_action(&backend, &tampered)
                .is_err()
        );
        let mut redirected = approved.clone();
        redirected.request.destination = devices.config.vault_address.clone();
        assert!(
            replacement
                .activate_offline_action(&backend, &redirected)
                .is_err()
        );
        let mut unsigned = approved.clone();
        unsigned.signatures.clear();
        assert!(
            replacement
                .activate_offline_action(&backend, &unsigned)
                .is_err()
        );
        assert_eq!(backend.broadcasts.get(), 0);
        if action == OfflineAction::HwwRecovery {
            assert_eq!(review.recovery_delay_blocks, Some(65_535));
            assert!(
                replacement
                    .activate_offline_action(&backend, &approved)
                    .unwrap_err()
                    .to_string()
                    .contains("not mature")
            );
            assert_eq!(backend.broadcasts.get(), 0);
            backend.height.set(65_544);
            backend.lose_ack.set(true);
            assert!(
                replacement
                    .activate_offline_action(&backend, &approved)
                    .is_err()
            );
            assert!(!backend.live.get()); // Retry must work even after accepted inputs disappear.
        }
        let txid = replacement
            .activate_offline_action(&backend, &approved)
            .unwrap();
        let count = backend.broadcasts.get();
        assert_eq!(
            replacement
                .activate_offline_action(&backend, &approved)
                .unwrap(),
            txid
        );
        assert_eq!(backend.broadcasts.get(), count);
        assert!(!replacement_dir.path().join("hww").exists());
        assert!(!devices.signer_dir.path().join("phone").exists());
    }
}

#[test]
fn hww_recovery_timing_and_requests_keep_mature_savings_separate_from_new_deposits() {
    let devices = Devices::paired();
    let mut backend = offline_fixture(&devices, OfflineAction::HwwRecovery);
    let oldest = backend.coins[0].clone();
    backend.coins.push(VaultUtxo {
        outpoint: OutPoint::new(Txid::from_byte_array([29; 32]), oldest.outpoint.vout),
        txout: TxOut {
            value: Amount::from_sat(1_000),
            ..oldest.txout.clone()
        },
        confirmation_height: 100,
    });
    let directory = tempfile::tempdir().unwrap();
    let phone = WalletApplication::new(directory.path().into(), false);
    phone.initialize(Network::Regtest).unwrap();
    phone
        .import_recovery_bundle(&devices.signer.export_recovery_bundle().unwrap())
        .unwrap();
    std::fs::remove_file(devices.phone_dir.path().join("phone/device.json")).unwrap();
    let snapshot = phone.recovery_snapshot(&backend).unwrap();
    assert_eq!(snapshot.phone.earliest_next_block_height, Some(61_210));
    assert_eq!(snapshot.hww.earliest_next_block_height, Some(65_545));
    assert_eq!(snapshot.hww.mature_sats, 0);
    let destination = phone.receive_address().unwrap();
    let request = phone
        .propose_offline_action(&backend, OfflineAction::HwwRecovery, &destination)
        .unwrap();
    assert_eq!(request.inputs.len(), 1);
    assert_eq!(
        request.inputs[0].outpoint.to_string(),
        oldest.outpoint.to_string()
    );
    let review = devices.signer.review_offline_action(&request).unwrap();
    let signed = devices
        .signer
        .approve_offline_action(&review.review_id)
        .unwrap();
    backend.height.set(65_543);
    assert_eq!(
        phone
            .recovery_snapshot(&backend)
            .unwrap()
            .hww
            .blocks_remaining,
        Some(1)
    );
    assert!(
        phone
            .activate_offline_action(&backend, &signed)
            .unwrap_err()
            .to_string()
            .contains("not mature")
    );
    backend.height.set(65_544);
    let snapshot = phone.recovery_snapshot(&backend).unwrap();
    assert_eq!(snapshot.hww.blocks_remaining, Some(0));
    assert_eq!(snapshot.hww.total_sats - snapshot.hww.mature_sats, 1_000);
    assert_eq!(
        phone
            .propose_offline_action(&backend, OfflineAction::HwwRecovery, &destination)
            .unwrap()
            .inputs
            .iter()
            .map(|input| input.outpoint.to_string())
            .collect::<Vec<_>>(),
        vec![oldest.outpoint.to_string()]
    );
    phone.activate_offline_action(&backend, &signed).unwrap();
    assert_eq!(
        phone
            .recovery_snapshot(&backend)
            .unwrap()
            .hww
            .blocks_remaining,
        None
    );
}

#[test]
fn backup_ceremony_never_exports_plaintext_or_reads_the_other_device_store() {
    let devices = Devices::paired();
    let request = devices.phone.propose_backup().unwrap();
    let json = serde_json::to_string(&request).unwrap();
    assert!(!json.contains("mnemonic"));
    assert!(!json.contains("phone_mnemonic"));
    let mut altered = request.clone();
    altered.payload.ciphertext[0] ^= 1;
    assert!(devices.signer.review_backup(&altered).is_err());
    let review = devices.signer.review_backup(&request).unwrap();
    devices.signer.reject_backup(&review.review_id).unwrap();
    assert!(
        devices
            .signer
            .approve_backup(&review.review_id, &request)
            .is_err()
    );
    let review = devices.signer.review_backup(&request).unwrap();
    assert!(
        devices
            .signer
            .approve_backup(&review.review_id, &altered)
            .is_err()
    );
    let response = devices
        .signer
        .approve_backup(&review.review_id, &request)
        .unwrap();
    // Direction/purpose binding rejects replaying a request as an approval.
    assert!(devices.phone.store_backup(&request).is_err());
    devices.phone.store_backup(&response).unwrap();
    let exported = devices.phone.export_backup().unwrap();
    assert_eq!(exported.kind, "vault-cloud-recovery");
    assert!(!devices.signer_dir.path().join("phone").exists());
    assert!(!devices.signer_dir.path().join("cloud").exists());
    let review_file: serde_json::Value = serde_json::from_slice(
        &std::fs::read(
            devices
                .signer_dir
                .path()
                .join(format!("hww/backup-reviews/{}.json", review.review_id)),
        )
        .unwrap(),
    )
    .unwrap();
    assert!(review_file.get("message").is_none());
    assert!(review_file.get("request_hash").is_some());
    assert!(!devices.phone_dir.path().join("hww/device.json").exists());
    // The signer never holds the seed backup. A fresh phone presents the cloud object and the
    // signer releases only its key, sealed to that phone.
    let fresh_dir = tempfile::tempdir().unwrap();
    let fresh = WalletApplication::new(fresh_dir.path().into(), false);
    fresh.initialize(Network::Regtest).unwrap();
    fresh
        .import_recovery_bundle(&devices.signer.export_recovery_bundle().unwrap())
        .unwrap();
    let restore = fresh.propose_restore(&exported).unwrap();
    let review = devices.signer.review_restore(&restore).unwrap();
    let restored = fresh
        .store_restored_key(&devices.signer.approve_restore(&review.review_id).unwrap())
        .unwrap();
    assert_eq!(
        restored.old_phone_public_key,
        devices.config.phone_vault_pubkey
    );
    // A newer request supersedes an older response instead of replacing the current backup.
    devices.phone.propose_backup().unwrap();
    assert!(devices.phone.store_backup(&response).is_err());
}

#[test]
fn recovery_contacts_are_independent_and_backup_updates_require_fingerprint_review() {
    let devices = Devices::paired();
    let friend_dir = tempfile::tempdir().unwrap();
    let friend = WalletApplication::new(friend_dir.path().into(), false);
    friend.initialize(Network::Regtest).unwrap();
    assert!(friend.recovery_contact().unwrap().is_none());
    let alice = friend.generate_recovery_contact("Alice").unwrap();
    assert!(
        friend
            .generate_recovery_contact("Replacement Alice")
            .is_err()
    );
    assert_eq!(
        friend.recovery_contact().unwrap().unwrap().fingerprint,
        alice.fingerprint
    );
    let shared = serde_json::to_string(&alice).unwrap();
    assert!(!shared.contains("PRIVATE KEY"));
    assert!(!shared.contains("private_key"));
    assert!(!shared.contains("mnemonic"));
    let private = std::fs::read(friend_dir.path().join("phone/recovery-contact.json")).unwrap();
    let mut invalid = alice.clone();
    invalid.fingerprint.push('0');
    assert!(devices.phone.propose_backup_with_contact(&invalid).is_err());
    let message = devices.phone.propose_backup_with_contact(&alice).unwrap();
    let review = devices.signer.review_backup(&message).unwrap();
    assert_eq!(review.friend_fingerprints, vec![alice.fingerprint.clone()]);
    assert!(devices.phone.export_backup().is_err());
    devices.signer.reject_backup(&review.review_id).unwrap();
    assert!(
        devices
            .signer
            .approve_backup(&review.review_id, &message)
            .is_err()
    );
    let review = devices.signer.review_backup(&message).unwrap();
    let approved = devices
        .signer
        .approve_backup(&review.review_id, &message)
        .unwrap();
    devices.phone.store_backup(&approved).unwrap();
    let backup = devices.phone.export_backup().unwrap();
    assert_eq!(backup.friends.len(), 1);
    assert_eq!(backup.friends[0].fingerprint, alice.fingerprint);
    assert_eq!(
        backup.friends[0].public_key_armored,
        alice.public_key_armored
    );
    assert!(devices.phone.propose_backup_with_contact(&alice).is_err());
    assert_eq!(devices.phone.export_backup().unwrap(), backup);
    // Recreating a backup must preserve existing contacts, never silently reset the list.
    let renewed = devices.phone.propose_backup().unwrap();
    let review = devices.signer.review_backup(&renewed).unwrap();
    assert_eq!(review.friend_fingerprints, vec![alice.fingerprint.clone()]);
    let approved = devices
        .signer
        .approve_backup(&review.review_id, &renewed)
        .unwrap();
    devices.phone.store_backup(&approved).unwrap();
    let next = devices.phone.export_backup().unwrap();
    assert_eq!(next.friends[0].fingerprint, alice.fingerprint);
    assert_ne!(next.encrypted_payload, backup.encrypted_payload);
    assert_ne!(
        next.friends[0].encrypted_symmetric_key,
        backup.friends[0].encrypted_symmetric_key
    );
    assert_eq!(
        private,
        std::fs::read(friend_dir.path().join("phone/recovery-contact.json")).unwrap()
    );
    assert!(
        !devices
            .phone_dir
            .path()
            .join("phone/recovery-contact.json")
            .exists()
    );
    assert!(!devices.signer_dir.path().join("phone").exists());
    // An unapproved contact proposal cannot mutate the active backup.
    let bob_dir = tempfile::tempdir().unwrap();
    let bob = WalletApplication::new(bob_dir.path().into(), false);
    bob.initialize(Network::Regtest).unwrap();
    let bob = bob.generate_recovery_contact("Bob").unwrap();
    let proposal = devices.phone.propose_backup_with_contact(&bob).unwrap();
    assert_eq!(devices.phone.export_backup().unwrap(), next);
    let review = devices.signer.review_backup(&proposal).unwrap();
    let mut fingerprints = vec![alice.fingerprint, bob.fingerprint];
    fingerprints.sort();
    assert_eq!(review.friend_fingerprints, fingerprints);
    assert!(
        devices
            .signer
            .approve_backup(&review.review_id, &renewed)
            .is_err()
    );
    let approved = devices
        .signer
        .approve_backup(&review.review_id, &proposal)
        .unwrap();
    devices.phone.store_backup(&approved).unwrap();
    assert_eq!(devices.phone.export_backup().unwrap().friends.len(), 2);
}

#[test]
fn contact_recovery_needs_no_original_devices_and_keeps_the_real_sweep_delay() {
    let devices = Devices::paired();
    let mut source = RotationBackend(offline_fixture(&devices, OfflineAction::HwwRecovery));
    let oldest = source.0.coins[0].clone();
    let original_amount = oldest.txout.value.to_sat();
    let youngest = VaultUtxo {
        outpoint: OutPoint::new(Txid::from_byte_array([31; 32]), oldest.outpoint.vout),
        txout: TxOut {
            value: Amount::from_sat(1_000),
            ..oldest.txout.clone()
        },
        confirmation_height: 100,
    };
    source.0.coins.push(youngest.clone());
    let contact_dir = tempfile::tempdir().unwrap();
    let contact = WalletApplication::new(contact_dir.path().into(), false);
    contact.initialize(Network::Regtest).unwrap();
    let public = contact.generate_recovery_contact("Alice").unwrap();
    let proposal = devices.phone.propose_backup_with_contact(&public).unwrap();
    let review = devices.signer.review_backup(&proposal).unwrap();
    let approval = devices
        .signer
        .approve_backup(&review.review_id, &proposal)
        .unwrap();
    devices.phone.store_backup(&approval).unwrap();
    let backup = devices.phone.export_backup().unwrap();
    let expected_phone = devices.phone.public_identity().unwrap();
    // Only the cloud object and contact survive. There is no public config import workaround.
    std::fs::remove_file(devices.phone_dir.path().join("phone/device.json")).unwrap();
    std::fs::remove_file(devices.signer_dir.path().join("hww/device.json")).unwrap();
    let replacement_dir = tempfile::tempdir().unwrap();
    let replacement = WalletApplication::new(replacement_dir.path().into(), false);
    let identity = replacement.initialize(Network::Regtest).unwrap();
    let fresh_key = std::fs::read(replacement_dir.path().join("phone/device.json")).unwrap();
    let request = replacement
        .propose_contact_restore(&backup, &public)
        .unwrap();
    let wire = serde_json::to_string(&request).unwrap();
    assert!(!wire.contains("encrypted_payload"));
    assert!(!wire.contains("phone_mnemonic"));
    assert!(!wire.contains("\"symmetric_key\":")); // Only the encrypted OpenPGP wrapper is present.
    let reviewed = contact.review_contact_restore(&request).unwrap();
    assert_eq!(reviewed.recipient_public_key, identity.vault_pubkey);
    contact.reject_contact_restore(&reviewed.review_id).unwrap();
    assert!(
        contact
            .approve_contact_restore(&reviewed.review_id, &request)
            .is_err()
    );
    let reviewed = contact.review_contact_restore(&request).unwrap();
    let mut altered = request.clone();
    altered.backup_id = "a".repeat(64);
    assert!(
        contact
            .approve_contact_restore(&reviewed.review_id, &altered)
            .is_err()
    );
    let response = contact
        .approve_contact_restore(&reviewed.review_id, &request)
        .unwrap();
    let mut corrupt = response.clone();
    corrupt.payload.ciphertext[0] ^= 1;
    assert!(replacement.store_contact_restored_key(&corrupt).is_err());
    let state = replacement.store_contact_restored_key(&response).unwrap();
    assert_eq!(state.config.vault_address, devices.config.vault_address);
    assert_eq!(state.config.phone_vault_pubkey, expected_phone.vault_pubkey);
    assert!(!state.spending_policy_available);
    assert_eq!(state.recipient_public_key, identity.vault_pubkey);
    assert!(replacement.configuration().is_err()); // Watch-only source, never a replacement of this phone's own vault/key.
    assert_eq!(
        fresh_key,
        std::fs::read(replacement_dir.path().join("phone/device.json")).unwrap()
    );
    assert_eq!(
        replacement.contact_recovery_state().unwrap().request_id,
        state.request_id
    );
    assert!(!contact_dir.path().join("cloud").exists());
    assert!(!contact_dir.path().join("phone/restored-keys").exists());
    assert!(!replacement_dir.path().join("hww").exists());
    // A contact cannot authenticate a payload it never sees. The recipient must reject
    // a changed cloud object without damaging a previously recovered key or identity.
    let mut corrupt_backup = backup.clone();
    corrupt_backup.encrypted_payload.ciphertext[0] ^= 1;
    let changed_request = replacement
        .propose_contact_restore(&corrupt_backup, &public)
        .unwrap();
    assert!(replacement.store_contact_restored_key(&response).is_err()); // Superseded request.
    let reviewed = contact.review_contact_restore(&changed_request).unwrap();
    let changed_response = contact
        .approve_contact_restore(&reviewed.review_id, &changed_request)
        .unwrap();
    assert!(
        replacement
            .store_contact_restored_key(&changed_response)
            .is_err()
    );
    assert_eq!(
        replacement.contact_recovery_state().unwrap().request_id,
        state.request_id
    );
    assert_eq!(
        replacement.public_identity().unwrap().vault_pubkey,
        identity.vault_pubkey
    );
    let status = replacement.contact_recovery_status(&source).unwrap();
    assert_eq!(status.balance_sats, original_amount + 1_000);
    assert_eq!(status.mature_balance_sats, 0);
    assert_eq!(status.next_block_height, Some(61_210));
    let destination = replacement.receive_address().unwrap();
    let review = replacement
        .review_contact_sweep(&source, &destination)
        .unwrap();
    assert_eq!(review.input_count, 1);
    assert!(review.blocks_remaining > 0);
    let record: serde_json::Value = serde_json::from_slice(
        &std::fs::read(replacement_dir.path().join(format!(
            "phone/contact-sweeps/{}/review.json",
            review.review_id
        )))
        .unwrap(),
    )
    .unwrap();
    // The review fixes the exact transaction ID; the recovered key signs only on confirmation.
    assert!(record["transfer"]["txid"].is_string());
    assert!(record["transfer"].get("psbt").is_none());
    replacement.reject_contact_sweep(&review.review_id).unwrap();
    assert!(
        replacement
            .confirm_contact_sweep(&source, &review.review_id)
            .is_err()
    );
    let review = replacement
        .review_contact_sweep(&source, &destination)
        .unwrap();
    assert!(
        replacement
            .confirm_contact_sweep(&source, &review.review_id)
            .unwrap_err()
            .to_string()
            .contains("not mature")
    );
    source.0.height.set(61_208);
    assert!(
        replacement
            .confirm_contact_sweep(&source, &review.review_id)
            .unwrap_err()
            .to_string()
            .contains("not mature")
    );
    assert_eq!(source.0.broadcasts.get(), 0);
    source.0.height.set(61_209);
    // An attacker sending newer deposits cannot postpone recovery of mature savings.
    let status = replacement.contact_recovery_status(&source).unwrap();
    assert_eq!(status.mature_balance_sats, original_amount);
    assert_eq!(status.blocks_remaining, Some(0));
    let at_maturity = replacement
        .review_contact_sweep(&source, &destination)
        .unwrap();
    assert_eq!(at_maturity.input_count, 1);
    assert_eq!(at_maturity.blocks_remaining, 0);
    assert_eq!(at_maturity.review_id, review.review_id);
    assert_eq!(
        at_maturity.amount_sats + at_maturity.fee_sats,
        original_amount
    );
    source.0.lose_ack.set(true);
    assert!(
        replacement
            .confirm_contact_sweep(&source, &review.review_id)
            .is_err()
    );
    assert!(replacement.reject_contact_sweep(&review.review_id).is_err());
    let txid = replacement
        .confirm_contact_sweep(&source, &review.review_id)
        .unwrap();
    let count = source.0.broadcasts.get();
    assert_eq!(
        replacement
            .confirm_contact_sweep(&source, &review.review_id)
            .unwrap(),
        txid
    );
    assert_eq!(source.0.broadcasts.get(), count);
}

#[test]
fn cloud_restore_keeps_the_new_phone_key_independent_and_seed_off_the_signer() {
    let devices = Devices::paired();
    let backup_request = devices.phone.propose_backup().unwrap();
    let review = devices.signer.review_backup(&backup_request).unwrap();
    let response = devices
        .signer
        .approve_backup(&review.review_id, &backup_request)
        .unwrap();
    devices.phone.store_backup(&response).unwrap();
    let backup = devices.phone.export_backup().unwrap();
    // The original phone cannot restore into the same key; this operation requires a new device.
    assert!(devices.phone.propose_restore(&backup).is_err());
    let replacement_dir = tempfile::tempdir().unwrap();
    let replacement = WalletApplication::new(replacement_dir.path().into(), false);
    let identity = replacement.initialize(Network::Regtest).unwrap();
    let original_device_bytes =
        std::fs::read(replacement_dir.path().join("phone/device.json")).unwrap();
    replacement.import_recovery_vault(&devices.config).unwrap();
    let request = replacement.propose_restore(&backup).unwrap();
    let serialized = serde_json::to_string(&request).unwrap();
    assert!(!serialized.contains("phone_mnemonic"));
    assert!(!serialized.contains("encrypted_payload"));
    assert!(!serialized.contains("symmetric_key"));
    assert_eq!(request.recipient.vault_pubkey, identity.vault_pubkey);
    std::fs::remove_file(devices.phone_dir.path().join("phone/device.json")).unwrap();
    assert!(replacement.restored_phone().is_err());

    let mut bad = request.clone();
    bad.recipient.vault_pubkey = devices.config.hww_vault_pubkey.clone();
    assert!(devices.signer.review_restore(&bad).is_err());
    let mut bad = request.clone();
    bad.wrapped_key.ciphertext[0] ^= 1;
    assert!(devices.signer.review_restore(&bad).is_err());
    let review = devices.signer.review_restore(&request).unwrap();
    assert_eq!(review.recipient_public_key, identity.vault_pubkey);
    devices.signer.reject_restore(&review.review_id).unwrap();
    assert!(devices.signer.approve_restore(&review.review_id).is_err());
    let review = devices.signer.review_restore(&request).unwrap();
    let response = devices.signer.approve_restore(&review.review_id).unwrap();
    let mut bad = response.clone();
    bad.payload.ciphertext[0] ^= 1;
    assert!(replacement.store_restored_key(&bad).is_err());
    let summary = replacement.store_restored_key(&response).unwrap();
    assert_eq!(
        summary.old_phone_public_key,
        devices.config.phone_vault_pubkey
    );
    assert_eq!(summary.new_phone_public_key, identity.vault_pubkey);
    assert!(summary.requires_rotation);
    assert_eq!(
        replacement.public_identity().unwrap().vault_pubkey,
        identity.vault_pubkey
    );
    assert_eq!(
        std::fs::read(replacement_dir.path().join("phone/device.json")).unwrap(),
        original_device_bytes
    );
    assert!(!replacement_dir.path().join("hww").exists());
    assert!(!devices.signer_dir.path().join("phone").exists());
    assert!(!devices.signer_dir.path().join("cloud").exists());
    let review_file = std::fs::read_to_string(
        devices
            .signer_dir
            .path()
            .join(format!("hww/restore-reviews/{}.json", review.review_id)),
    )
    .unwrap();
    assert!(!review_file.contains("encrypted_payload"));
    assert!(!review_file.contains("phone_mnemonic"));
    let encrypted = std::fs::read_dir(replacement_dir.path().join("phone/restored-keys"))
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    let before = std::fs::read_to_string(&encrypted).unwrap();
    assert!(!before.contains("phone_mnemonic"));

    // HWW cannot authenticate an absent payload. The replacement must authenticate it locally
    // and retain the previously recovered key intact if the cloud payload has been altered.
    let mut bad_backup = backup.clone();
    bad_backup.encrypted_payload.ciphertext[0] ^= 1;
    let bad_request = replacement.propose_restore(&bad_backup).unwrap();
    assert!(replacement.store_restored_key(&response).is_err()); // Superseded request.
    let review = devices.signer.review_restore(&bad_request).unwrap();
    let approval = devices.signer.approve_restore(&review.review_id).unwrap();
    assert!(replacement.store_restored_key(&approval).is_err());
    assert_eq!(std::fs::read_to_string(&encrypted).unwrap(), before);
    assert_eq!(
        replacement.restored_phone().unwrap().old_phone_public_key,
        devices.config.phone_vault_pubkey
    );
}

#[test]
fn cooperative_sweep_requires_the_exact_review_and_both_separate_devices() {
    let devices = Devices::paired();
    let backend = TestBackend {
        broadcasts: Cell::new(0),
    };
    let destination = devices.phone.receive_address().unwrap();
    let package = devices.phone.propose_sweep(&backend, &destination).unwrap();
    let unsigned = anzen_wallet::api::SweepApproval {
        version: package.version,
        request: package.clone(),
        signatures: vec![],
    };
    assert!(devices.phone.activate_sweep(&backend, &unsigned).is_err());
    assert_eq!(backend.broadcasts.get(), 0);
    let mut tampered = package.clone();
    tampered.inputs.push(tampered.inputs[0]);
    assert!(devices.signer.review_sweep(&tampered).is_err());
    let review = devices.signer.review_sweep(&package).unwrap();
    assert_eq!(review.destination, destination);
    assert_eq!(review.amount_sats + review.fee_sats, 210_000_000);
    assert!(devices.signer.approve_sweep(&"0".repeat(64)).is_err());
    let approved = devices.signer.approve_sweep(&review.review_id).unwrap();
    devices.phone.activate_sweep(&backend, &approved).unwrap();
    assert_eq!(backend.broadcasts.get(), 1);
    assert!(!devices.signer_dir.path().join("phone/device.json").exists());
}

#[test]
fn phone_recovery_does_not_broadcast_before_maturity() {
    let devices = Devices::paired();
    let backend = TestBackend {
        broadcasts: Cell::new(0),
    };
    let error = devices
        .phone
        .phone_recovery(&backend, &devices.phone.receive_address().unwrap())
        .unwrap_err();
    assert!(error.to_string().contains("no vault UTXOs are mature"));
    assert_eq!(backend.broadcasts.get(), 0);
}

#[test]
fn pairing_never_requires_or_copies_the_other_devices_private_key() {
    let devices = Devices::paired();
    assert!(!devices.phone_dir.path().join("hww/device.json").exists());
    assert!(!devices.signer_dir.path().join("phone").exists());
    assert_eq!(devices.config.monthly_limit_sats, 0);
    assert_eq!(devices.config.emergency_access_limit_sats, 0);
    devices.signer.accept_vault(&devices.config).unwrap(); // Same public pairing is idempotent.
    let json = serde_json::to_string(&devices.phone.public_identity().unwrap()).unwrap();
    assert!(!json.contains("mnemonic"));
    assert!(!json.contains("seed"));
    assert!(
        devices
            .phone
            .receive_address()
            .unwrap()
            .starts_with("bcrt1p")
    );
}

#[test]
fn signer_reconstructs_policy_and_rejects_changes_to_an_accepted_vault() {
    let devices = Devices::paired();
    for field in [
        "address",
        "descriptor",
        "phone-delay",
        "hww-delay",
        "hww-key",
        "limit",
    ] {
        let mut altered = devices.config.clone();
        match field {
            "address" => altered.vault_address = devices.phone.receive_address().unwrap(),
            "descriptor" => altered.vault_descriptor.push(' '),
            "phone-delay" => altered.phone_recovery_blocks -= 1,
            "hww-delay" => altered.hww_recovery_blocks -= 1,
            "hww-key" => altered.hww_vault_pubkey = altered.phone_vault_pubkey.clone(),
            "limit" => altered.monthly_limit_sats = 1000,
            _ => unreachable!(),
        }
        assert!(
            devices.signer.accept_vault(&altered).is_err(),
            "accepted altered {field}"
        );
    }
}

#[test]
fn mainnet_access_is_explicit_and_checked_after_reopening() {
    let dir = tempfile::tempdir().unwrap();
    let denied = WalletApplication::new(dir.path().into(), false);
    assert!(denied.initialize(Network::Bitcoin).is_err());
    assert!(!dir.path().join("phone").exists());
    let enabled = WalletApplication::new(dir.path().into(), true);
    enabled.initialize(Network::Bitcoin).unwrap();
    assert!(denied.public_identity().is_err());
    let cold_dir = tempfile::tempdir().unwrap();
    assert!(
        SignerApplication::new(cold_dir.path().into(), false)
            .initialize(Network::Bitcoin)
            .is_err()
    );
}

#[test]
fn public_pairing_rejects_wrong_network_and_message_kind_without_initializing() {
    let phone_dir = tempfile::tempdir().unwrap();
    let signer_dir = tempfile::tempdir().unwrap();
    let phone = WalletApplication::new(phone_dir.path().into(), false);
    phone.initialize(Network::Regtest).unwrap();
    let signer = SignerApplication::new(signer_dir.path().into(), false);
    let mut identity = signer.initialize(Network::Regtest).unwrap();
    identity.network = "bitcoin".into();
    assert!(phone.pair_signer(&identity).is_err());
    identity.network = "regtest".into();
    identity.kind = "phone-public-key".into();
    assert!(phone.pair_signer(&identity).is_err());
    assert!(!phone_dir.path().join("anzen.json").exists());
}

#[test]
fn policy_review_and_approval_work_across_isolated_devices() {
    let devices = Devices::paired();
    let backend = TestBackend {
        broadcasts: Cell::new(0),
    };
    let package = devices
        .phone
        .propose_policy(
            &backend,
            PolicyLimits {
                monthly_limit_sats: 10_000_000,
                emergency_access_limit_sats: 50_000_000,
            },
        )
        .unwrap();
    let review = devices.signer.review_policy(&package).unwrap();
    assert_eq!(review.funded_withdrawals, 12);
    assert_eq!(review.monthly_limit_sats, 10_000_000);
    assert_eq!(
        review.allowance_destinations,
        package.allowance_destinations
    );
    assert_eq!(backend.broadcasts.get(), 0);
    // An unapproved proposal cannot be activated even if a caller bypasses the UI.
    let unsigned = anzen_wallet::api::PolicyApproval {
        version: package.version,
        request: package.clone(),
        signatures: vec![],
    };
    assert!(devices.phone.activate_policy(&backend, &unsigned).is_err());
    assert_eq!(backend.broadcasts.get(), 0);
    let approved = devices.signer.approve_policy(&review.review_id).unwrap();
    assert_eq!(approved.signatures.len(), 15);
    assert_eq!(backend.broadcasts.get(), 0);
    let schedule = devices.phone.activate_policy(&backend, &approved).unwrap();
    assert_eq!(schedule.entries.len(), 12);
    assert_eq!(backend.broadcasts.get(), 1);
    let snapshot = devices.phone.vault_snapshot(&backend).unwrap();
    assert_eq!(snapshot.phone_recovery_next_block_height, Some(61_210));
    let wallet_snapshot =
        serde_json::to_string(&devices.phone.wallet_snapshot(&backend).unwrap()).unwrap();
    assert!(!wallet_snapshot.contains("vault"));
    assert!(!wallet_snapshot.contains("policy"));
    let renewed = devices.phone.refresh_policy(&backend).unwrap();
    assert_eq!(renewed.limits.monthly_limit_sats, 10_000_000);
    assert_eq!(renewed.limits.emergency_access_limit_sats, 50_000_000);
}

#[test]
fn altered_rejected_or_unreviewed_requests_cannot_be_approved() {
    let devices = Devices::paired();
    let backend = TestBackend {
        broadcasts: Cell::new(0),
    };
    let package = devices
        .phone
        .propose_policy(
            &backend,
            PolicyLimits {
                monthly_limit_sats: 10_000_000,
                emergency_access_limit_sats: 0,
            },
        )
        .unwrap();
    let mut tampered = package.clone();
    tampered.allowance_destinations.pop();
    assert!(devices.signer.review_policy(&tampered).is_err());
    let mut tampered = package.clone();
    tampered.keys.phone = tampered.keys.hww;
    assert!(devices.signer.review_policy(&tampered).is_err());
    assert!(devices.signer.approve_policy(&"0".repeat(64)).is_err());
    assert!(devices.signer.approve_policy("../../phone/device").is_err());
    let review = devices.signer.review_policy(&package).unwrap();
    devices.signer.reject_policy(&review.review_id).unwrap();
    assert!(devices.signer.approve_policy(&review.review_id).is_err());
    assert_eq!(backend.broadcasts.get(), 0);
}
