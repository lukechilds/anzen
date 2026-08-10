use anzen::{
    cold_wallet,
    core::{
        EMERGENCY_ACCESS_DELAY_SECONDS, MONTHLY_ALLOWANCE_DELAY_SECONDS,
        ceremony::TransactionKind,
        chain::{BitcoinCoreBackend, RpcConfig},
        storage::{initialize_vault, set_policy_limits},
    },
    hot_wallet::{self, HotWallet, HotWalletBackend},
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
fn real_regtest_runs_sequential_allowances_whole_chain_revocation_and_soft_limit() {
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
        hot_wallet::propose_policy(dir.path(), &rpc, now, 10_000_000, 0, &batch_dir).unwrap();
    assert_eq!(prepared.allowance_count, 12);
    let approved = cold_wallet::approve_policy(dir.path(), &batch_dir).unwrap();
    assert!(approved.hww_approved);
    let schedule = hot_wallet::activate_policy(dir.path(), &rpc, &batch_dir).unwrap();
    let first_delay_base = rpc.chain_info().unwrap().median_time;
    rpc.mine(1, &mining_address).unwrap();
    assert_eq!(rpc.scan_vault(&config).unwrap().len(), 2);

    let first = schedule.entries[0].clone();
    let second = schedule.entries[1].clone();
    let third = schedule.entries[2].clone();
    let fourth = schedule.entries[3].clone();
    let premature =
        hot_wallet::broadcast_monthly(dir.path(), &rpc, first.step, TransactionKind::Authorization)
            .unwrap_err();
    let premature = format!("{premature:#}");
    assert!(premature.contains("non-BIP68-final") || premature.contains("non-final"));

    let first_unlock = u32::try_from(first_delay_base).unwrap() + MONTHLY_ALLOWANCE_DELAY_SECONDS;
    advance_mtp(&rpc, first_unlock, &mining_address);
    hot_wallet::broadcast_monthly(dir.path(), &rpc, first.step, TransactionKind::Authorization)
        .unwrap();
    let second_delay_base = rpc.chain_info().unwrap().median_time;
    rpc.mine(1, &mining_address).unwrap();
    let soft_return = hot_wallet::apply_soft_limit(dir.path(), &rpc, first.step, 1_000_000)
        .unwrap()
        .unwrap();
    assert_ne!(soft_return.to_string(), first.authorization_txid);
    rpc.mine(1, &mining_address).unwrap();

    let premature_second = hot_wallet::broadcast_monthly(
        dir.path(),
        &rpc,
        second.step,
        TransactionKind::Authorization,
    )
    .unwrap_err();
    let premature_second = format!("{premature_second:#}");
    assert!(premature_second.contains("non-BIP68-final") || premature_second.contains("non-final"));

    let second_unlock = u32::try_from(second_delay_base).unwrap() + MONTHLY_ALLOWANCE_DELAY_SECONDS;
    advance_mtp(&rpc, second_unlock, &mining_address);
    hot_wallet::broadcast_monthly(
        dir.path(),
        &rpc,
        second.step,
        TransactionKind::Authorization,
    )
    .unwrap();
    let third_delay_base = rpc.chain_info().unwrap().median_time;
    rpc.mine(1, &mining_address).unwrap();

    hot_wallet::broadcast_monthly(dir.path(), &rpc, third.step, TransactionKind::Revocation)
        .unwrap();
    rpc.mine(1, &mining_address).unwrap();

    let third_unlock = u32::try_from(third_delay_base).unwrap() + MONTHLY_ALLOWANCE_DELAY_SECONDS;
    advance_mtp(&rpc, third_unlock, &mining_address);
    for revoked_step in [third.step, fourth.step] {
        let revoked = hot_wallet::broadcast_monthly(
            dir.path(),
            &rpc,
            revoked_step,
            TransactionKind::Authorization,
        )
        .unwrap_err();
        assert!(format!("{revoked:#}").contains("missingorspent"));
    }
}

#[test]
#[ignore = "requires a disposable Bitcoin Core regtest node"]
fn real_regtest_enforces_emergency_delay_and_cancellation() {
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

    let chain_time = rpc.chain_info().unwrap().median_time as i64;
    let captured_now = Utc::now().timestamp().max(chain_time);
    let now = DateTime::from_timestamp(captured_now, 0).unwrap();
    let batch_dir = dir.path().join("emergency-cancel-batch");
    hot_wallet::propose_policy(dir.path(), &rpc, now, 10_000_000, 50_000_000, &batch_dir).unwrap();
    cold_wallet::approve_policy(dir.path(), &batch_dir).unwrap();
    hot_wallet::activate_policy(dir.path(), &rpc, &batch_dir).unwrap();
    set_policy_limits(dir.path(), 10_000_000, 50_000_000).unwrap();
    rpc.mine(1, &mining_address).unwrap();

    hot_wallet::initiate_emergency_access(dir.path(), &rpc).unwrap();
    rpc.mine(1, &mining_address).unwrap();

    let premature = hot_wallet::withdraw_emergency_access(dir.path(), &rpc).unwrap_err();
    let premature = format!("{premature:#}");
    assert!(premature.contains("non-BIP68-final") || premature.contains("non-final"));

    hot_wallet::cancel_emergency_access(dir.path(), &rpc).unwrap();
    rpc.mine(1, &mining_address).unwrap();
    let cancelled = hot_wallet::withdraw_emergency_access(dir.path(), &rpc).unwrap_err();
    assert!(format!("{cancelled:#}").contains("missingorspent"));
}

#[test]
#[ignore = "requires a disposable Bitcoin Core regtest node"]
fn real_regtest_releases_emergency_access_after_one_week() {
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
    let chain_time = rpc.chain_info().unwrap().median_time as i64;
    let captured_now = Utc::now().timestamp().max(chain_time);
    let now = DateTime::from_timestamp(captured_now, 0).unwrap();
    let batch_dir = dir.path().join("emergency-withdraw-batch");
    hot_wallet::propose_policy(dir.path(), &rpc, now, 10_000_000, 50_000_000, &batch_dir).unwrap();
    cold_wallet::approve_policy(dir.path(), &batch_dir).unwrap();
    hot_wallet::activate_policy(dir.path(), &rpc, &batch_dir).unwrap();
    set_policy_limits(dir.path(), 10_000_000, 50_000_000).unwrap();
    rpc.mine(1, &mining_address).unwrap();

    let relative_lock_base = rpc.chain_info().unwrap().median_time;
    hot_wallet::initiate_emergency_access(dir.path(), &rpc).unwrap();
    rpc.mine(1, &mining_address).unwrap();
    let premature = hot_wallet::withdraw_emergency_access(dir.path(), &rpc).unwrap_err();
    let premature = format!("{premature:#}");
    assert!(premature.contains("non-BIP68-final") || premature.contains("non-final"));

    let unlock_time = u32::try_from(relative_lock_base).unwrap() + EMERGENCY_ACCESS_DELAY_SECONDS;
    advance_mtp(&rpc, unlock_time, &mining_address);
    hot_wallet::withdraw_emergency_access(dir.path(), &rpc).unwrap();
    rpc.mine(1, &mining_address).unwrap();

    let mut hot = HotWallet::open_or_create(dir.path()).unwrap();
    rpc.sync_hot_wallet(&mut hot).unwrap();
    assert!(hot.wallet.balance().total().to_sat() >= 50_000_000);
}

fn advance_mtp(rpc: &BitcoinCoreBackend, timestamp: u32, mining_address: &Address) {
    rpc.set_mock_time(u64::from(timestamp) + 60).unwrap();
    while rpc.chain_info().unwrap().median_time <= u64::from(timestamp) {
        rpc.mine(1, mining_address).unwrap();
    }
}
