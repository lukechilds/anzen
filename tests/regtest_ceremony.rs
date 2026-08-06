use anzen::{
    cold_wallet,
    core::{
        ceremony::TransactionKind,
        chain::{BitcoinCoreBackend, RpcConfig},
        storage::initialize_vault,
    },
    hot_wallet::{self, HotWallet},
};
use bitcoin::{Address, Network};
use chrono::{DateTime, Utc};
use std::{env, str::FromStr};

fn rpc_from_env() -> BitcoinCoreBackend {
    BitcoinCoreBackend::connect(
        &RpcConfig {
            url: env::var("ANZEN_RPC_URL").unwrap_or_else(|_| "http://127.0.0.1:18443".to_owned()),
            user: env::var("ANZEN_RPC_USER").unwrap_or_else(|_| "anzen".to_owned()),
            password: env::var("ANZEN_RPC_PASSWORD").unwrap_or_else(|_| "anzen".to_owned()),
        },
        Network::Regtest,
    )
    .unwrap()
}

#[test]
#[ignore = "requires a disposable Bitcoin Core regtest node"]
fn real_regtest_runs_rollover_authorization_revocation_and_soft_limit() {
    let dir = tempfile::tempdir().unwrap();
    hot_wallet::initialize(dir.path(), Network::Regtest).unwrap();
    cold_wallet::initialize(dir.path(), Network::Regtest).unwrap();
    let config = initialize_vault(dir.path()).unwrap();
    let rpc = rpc_from_env();
    let vault_address = Address::from_str(&config.vault_address)
        .unwrap()
        .require_network(Network::Regtest)
        .unwrap();
    let mut hot = HotWallet::open_or_create(dir.path()).unwrap();
    let mining_address = hot.next_receive_address().unwrap();

    rpc.mine(1, &vault_address).unwrap();
    rpc.mine(100, &mining_address).unwrap();
    assert_eq!(rpc.scan_vault(&config).unwrap().len(), 1);

    let chain_time = rpc.chain_info().unwrap().median_time as i64;
    let captured_now = Utc::now().timestamp().max(chain_time);
    let now = DateTime::from_timestamp(captured_now, 0).unwrap();
    let batch_dir = dir.path().join("batch");
    let prepared =
        hot_wallet::propose_policy(dir.path(), &rpc, now, 10_000_000, &batch_dir).unwrap();
    assert_eq!(prepared.chunk_count, 12);
    let approved = cold_wallet::approve_policy(dir.path(), &batch_dir).unwrap();
    assert!(approved.hww_approved);
    let schedule = hot_wallet::activate_policy(dir.path(), &rpc, &batch_dir).unwrap();
    rpc.mine(1, &mining_address).unwrap();
    assert_eq!(rpc.scan_vault(&config).unwrap().len(), 1);

    let first = schedule.entries[0].clone();
    let second = schedule.entries[1].clone();
    let premature = hot_wallet::broadcast_monthly(
        dir.path(),
        &rpc,
        &first.month,
        TransactionKind::Authorization,
    )
    .unwrap_err();
    assert!(format!("{premature:#}").contains("non-final"));

    hot_wallet::broadcast_monthly(dir.path(), &rpc, &second.month, TransactionKind::Revocation)
        .unwrap();
    rpc.mine(1, &mining_address).unwrap();

    advance_mtp(&rpc, first.unlock_timestamp, &mining_address);
    hot_wallet::broadcast_monthly(
        dir.path(),
        &rpc,
        &first.month,
        TransactionKind::Authorization,
    )
    .unwrap();
    rpc.mine(1, &mining_address).unwrap();
    let soft_return = hot_wallet::apply_soft_limit(dir.path(), &rpc, &first.month, 1_000_000)
        .unwrap()
        .unwrap();
    assert_ne!(soft_return.to_string(), first.authorization_txid);
    rpc.mine(1, &mining_address).unwrap();

    advance_mtp(&rpc, second.unlock_timestamp, &mining_address);
    let revoked = hot_wallet::broadcast_monthly(
        dir.path(),
        &rpc,
        &second.month,
        TransactionKind::Authorization,
    )
    .unwrap_err();
    assert!(format!("{revoked:#}").contains("missingorspent"));
}

fn advance_mtp(rpc: &BitcoinCoreBackend, timestamp: u32, mining_address: &Address) {
    rpc.set_mock_time(u64::from(timestamp) + 60).unwrap();
    while rpc.chain_info().unwrap().median_time <= u64::from(timestamp) {
        rpc.mine(1, mining_address).unwrap();
    }
}
