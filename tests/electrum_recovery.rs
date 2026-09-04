use anzen::{
    cold_wallet,
    core::{
        chain::ElectrumBackend,
        storage::{PHONE_BACKUP_FILE, initialize_vault},
    },
    hot_wallet::{self, HotWallet, HotWalletBackend},
};
use bdk_electrum::electrum_client::ToElectrumScriptHash;
use bdk_wallet::KeychainKind;
use bitcoin::{
    Amount, Network, OutPoint, Transaction, TxIn, TxOut, Txid, absolute,
    blockdata::constants::genesis_block, consensus::encode::serialize_hex, hashes::Hash,
    transaction::Version,
};
use serde_json::{Value, json};
use std::{
    fs,
    io::{BufRead, BufReader, Write},
    net::TcpListener,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    thread,
    time::Duration,
};

// A loopback-only Electrum fixture exercises the real client/BDK discovery path without relying
// on a public server. Its ordinary unconfirmed payment has outputs on both wallet keychains.
struct ElectrumFixture {
    address: String,
    history_requests: Arc<AtomicUsize>,
    reject_history: Arc<AtomicBool>,
    handle: thread::JoinHandle<()>,
}

impl ElectrumFixture {
    fn start(payment: Transaction) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = format!("tcp://{}", listener.local_addr().unwrap());
        let history_requests = Arc::new(AtomicUsize::new(0));
        let reject_history = Arc::new(AtomicBool::new(false));
        let requests = history_requests.clone();
        let rejected = reject_history.clone();
        let handle = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(20)))
                .unwrap();
            let reader = BufReader::new(stream.try_clone().unwrap());
            let header = serialize_hex(&genesis_block(Network::Regtest).header);
            let scripts = payment
                .output
                .iter()
                .map(|output| json!(output.script_pubkey.to_electrum_scripthash()))
                .collect::<Vec<_>>();
            for line in reader.lines() {
                let Ok(line) = line else { break };
                let request: Value = serde_json::from_str(&line).unwrap();
                let method = request["method"].as_str().unwrap();
                let result = match method {
                    "server.version" => json!(["anzen-test", "1.4"]),
                    "blockchain.block.header" => json!(header),
                    "blockchain.headers.subscribe" => json!({"height": 0, "hex": header}),
                    "blockchain.block.headers" => json!({"count": 1, "hex": header, "max": 2016}),
                    "blockchain.scripthash.get_history" => {
                        requests.fetch_add(1, Ordering::SeqCst);
                        if rejected.load(Ordering::SeqCst) {
                            writeln!(stream, "{}", json!({"id": request["id"], "error": {"code": -1, "message": "test scan failure"}})).unwrap();
                            continue;
                        }
                        if scripts.iter().any(|script| request["params"][0] == *script) {
                            json!([{"tx_hash": payment.compute_txid(), "height": 0}])
                        } else {
                            json!([])
                        }
                    }
                    "blockchain.transaction.get" => {
                        assert_eq!(request["params"][0], payment.compute_txid().to_string());
                        json!(serialize_hex(&payment))
                    }
                    other => panic!("unexpected Electrum fixture call: {other}"),
                };
                writeln!(stream, "{}", json!({"id": request["id"], "result": result})).unwrap();
            }
        });
        Self {
            address,
            history_requests,
            reject_history,
            handle,
        }
    }
}

#[test]
fn electrum_restores_both_keychains_without_the_original_wallet_database() {
    check_recovery(false);
}

#[test]
fn failed_electrum_discovery_is_retried_after_reopening_the_wallet() {
    check_recovery(true);
}

fn check_recovery(fail_first_scan: bool) {
    let dir = tempfile::tempdir().unwrap();
    hot_wallet::initialize(dir.path(), Network::Regtest).unwrap();
    cold_wallet::initialize(dir.path(), Network::Regtest).unwrap();
    let config = initialize_vault(dir.path()).unwrap();
    cold_wallet::create_cloud_recovery_backup(dir.path(), &config).unwrap();
    let (external, internal) = {
        let mut wallet = HotWallet::open_or_create(dir.path()).unwrap();
        let mut external = wallet.next_receive_address().unwrap();
        for _ in 0..27 {
            external = wallet.next_receive_address().unwrap();
        }
        let mut internal = wallet.next_change_address().unwrap();
        for _ in 0..7 {
            internal = wallet.next_change_address().unwrap();
        }
        (external, internal)
    };
    let payment = Transaction {
        version: Version::TWO,
        lock_time: absolute::LockTime::ZERO,
        input: vec![TxIn {
            previous_output: OutPoint::new(Txid::all_zeros(), 0),
            ..Default::default()
        }],
        output: vec![
            TxOut {
                value: Amount::from_sat(1_000_000),
                script_pubkey: external.script_pubkey(),
            },
            TxOut {
                value: Amount::from_sat(2_000_000),
                script_pubkey: internal.script_pubkey(),
            },
        ],
    };
    let recovery =
        cold_wallet::decrypt_phone_backup_package(dir.path(), &dir.path().join(PHONE_BACKUP_FILE))
            .unwrap();
    // Lose the complete phone directory, not just device.json as in the original recovery tests.
    fs::rename(dir.path().join("phone"), dir.path().join("lost-phone")).unwrap();
    hot_wallet::restore_phone(dir.path(), &recovery).unwrap();
    let fixture = ElectrumFixture::start(payment);
    let backend = ElectrumBackend::connect(Network::Regtest, &[&fixture.address]).unwrap();
    if fail_first_scan {
        fixture.reject_history.store(true, Ordering::SeqCst);
        let mut phone = HotWallet::open_or_create(dir.path()).unwrap();
        assert!(backend.sync_hot_wallet(&mut phone).is_err());
        drop(phone);
        fixture.reject_history.store(false, Ordering::SeqCst);
    }
    let mut phone = HotWallet::open_or_create(dir.path()).unwrap();
    backend.sync_hot_wallet(&mut phone).unwrap();
    assert_eq!(phone.wallet.balance().total().to_sat(), 3_000_000);
    assert_eq!(phone.wallet.list_unspent().count(), 2);
    assert_eq!(
        phone.wallet.derivation_index(KeychainKind::External),
        Some(27)
    );
    assert_eq!(
        phone.wallet.derivation_index(KeychainKind::Internal),
        Some(7)
    );
    let discovered_queries = fixture.history_requests.load(Ordering::SeqCst);
    assert!(discovered_queries >= 200);
    drop(phone);

    let mut phone = HotWallet::open_or_create(dir.path()).unwrap();
    backend.sync_hot_wallet(&mut phone).unwrap();
    assert_eq!(phone.wallet.balance().total().to_sat(), 3_000_000);
    assert!(fixture.history_requests.load(Ordering::SeqCst) - discovered_queries < 100);
    assert_ne!(phone.next_receive_address().unwrap(), external);
    assert_ne!(phone.next_change_address().unwrap(), internal);
    drop(backend);
    fixture.handle.join().unwrap();
}
