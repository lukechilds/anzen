use anzen_chain::{BitcoinCoreBackend, Blockchain, RpcConfig};
use anzen_common::EMERGENCY_ACCESS_DELAY_SECONDS;
use anzen_common::MONTHLY_ALLOWANCE_DELAY_SECONDS;
use anzen_common::storage::VaultConfig;
use anzen_signer_app::{self, application::SignerApplication};
use anzen_wallet::ceremony::TransactionKind;
use anzen_wallet::offline_action::{self, OfflineAction};
use anzen_wallet::{
    self, HotWallet, HotWalletBackend,
    api::{PolicyApproval, PolicyLimits},
    application::WalletApplication,
};
use bitcoin::{Address, Network};
use std::{env, path::Path, str::FromStr};

/// Both simulated devices in one directory, paired through their applications.
fn devices(dir: &Path) -> (WalletApplication, SignerApplication, VaultConfig) {
    anzen_wallet::initialize(dir, Network::Regtest).unwrap();
    anzen_signer_app::initialize(dir, Network::Regtest).unwrap();
    let wallet = WalletApplication::new(dir.into(), false);
    let signer = SignerApplication::new(dir.into(), false);
    let config = wallet
        .pair_signer(&signer.public_identity().unwrap())
        .unwrap();
    signer.accept_vault(&config).unwrap();
    (wallet, signer, config)
}

fn approve(
    wallet: &WalletApplication,
    signer: &SignerApplication,
    rpc: &BitcoinCoreBackend,
    monthly_limit_sats: u64,
    emergency_access_limit_sats: u64,
) -> PolicyApproval {
    let request = wallet
        .propose_policy(
            rpc,
            PolicyLimits {
                monthly_limit_sats,
                emergency_access_limit_sats,
            },
        )
        .unwrap();
    let review = signer.review_policy(&request).unwrap();
    signer.approve_policy(&review.review_id).unwrap()
}

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
    let (wallet, signer, config) = devices(dir.path());
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

    let approved = approve(&wallet, &signer, &rpc, 10_000_000, 0);
    assert_eq!(approved.request.allowance_destinations.len(), 12);
    let schedule = wallet.activate_policy(&rpc, &approved).unwrap();
    let first_delay_base = rpc.chain_info().unwrap().median_time;
    rpc.mine(1, &mining_address).unwrap();
    let retried = wallet.activate_policy(&rpc, &approved).unwrap();
    assert_eq!(retried.rollover_txid, schedule.rollover_txid);
    let encrypted_backup = dir
        .path()
        .join("phone/transactions")
        .join(&schedule.rollover_txid)
        .join("approved-policy.json");
    assert_cmd::Command::cargo_bin("anzen")
        .unwrap()
        .arg("--data-dir")
        .arg(dir.path())
        .args(["phone", "activate-policy"])
        .arg(encrypted_backup)
        .assert()
        .success();
    assert_eq!(rpc.scan_vault(&config).unwrap().len(), 2);
    assert_eq!(rpc.scan_connectors(&config).unwrap().len(), 1);

    let first = schedule.entries[0].clone();
    let second = schedule.entries[1].clone();
    let third = schedule.entries[2].clone();
    let fourth = schedule.entries[3].clone();
    let premature = anzen_wallet::broadcast_monthly(
        dir.path(),
        &rpc,
        first.step,
        TransactionKind::Authorization,
    )
    .unwrap_err();
    let premature = format!("{premature:#}");
    assert!(premature.contains("non-BIP68-final") || premature.contains("non-final"));

    let first_unlock = u32::try_from(first_delay_base).unwrap() + MONTHLY_ALLOWANCE_DELAY_SECONDS;
    advance_mtp(&rpc, first_unlock, &mining_address);
    anzen_wallet::broadcast_monthly(dir.path(), &rpc, first.step, TransactionKind::Authorization)
        .unwrap();
    let second_delay_base = rpc.chain_info().unwrap().median_time;
    rpc.mine(1, &mining_address).unwrap();
    let soft_return = anzen_wallet::apply_soft_limit(dir.path(), &rpc, first.step, 1_000_000)
        .unwrap()
        .unwrap();
    assert_ne!(soft_return.to_string(), first.authorization_txid);
    rpc.mine(1, &mining_address).unwrap();

    let premature_second = anzen_wallet::broadcast_monthly(
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
    anzen_wallet::broadcast_monthly(
        dir.path(),
        &rpc,
        second.step,
        TransactionKind::Authorization,
    )
    .unwrap();
    let third_delay_base = rpc.chain_info().unwrap().median_time;
    rpc.mine(1, &mining_address).unwrap();

    let balance_before_revocation = vault_balance(&rpc, &config);
    anzen_wallet::broadcast_monthly(dir.path(), &rpc, third.step, TransactionKind::Revocation)
        .unwrap();
    rpc.mine(1, &mining_address).unwrap();
    assert!(rpc.scan_connectors(&config).unwrap().is_empty());
    assert_eq!(vault_balance(&rpc, &config), balance_before_revocation);

    let third_unlock = u32::try_from(third_delay_base).unwrap() + MONTHLY_ALLOWANCE_DELAY_SECONDS;
    advance_mtp(&rpc, third_unlock, &mining_address);
    for revoked_step in [third.step, fourth.step] {
        let revoked = anzen_wallet::broadcast_monthly(
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
    let (wallet, signer, config) = devices(dir.path());
    let rpc = rpc_from_env();
    let vault_address = Address::from_str(&config.vault_address)
        .unwrap()
        .require_network(Network::Regtest)
        .unwrap();
    let mut hot = HotWallet::open_or_create(dir.path()).unwrap();
    let mining_address = hot.next_receive_address().unwrap();

    rpc.mine(1, &vault_address).unwrap();
    rpc.mine(100, &mining_address).unwrap();

    let approved = approve(&wallet, &signer, &rpc, 10_000_000, 50_000_000);
    wallet.activate_policy(&rpc, &approved).unwrap();
    rpc.mine(1, &mining_address).unwrap();
    assert_eq!(rpc.scan_connectors(&config).unwrap().len(), 2);

    anzen_wallet::initiate_emergency_access(dir.path(), &rpc).unwrap();
    rpc.mine(1, &mining_address).unwrap();

    let premature = anzen_wallet::withdraw_emergency_access(dir.path(), &rpc).unwrap_err();
    let premature = format!("{premature:#}");
    assert!(premature.contains("non-BIP68-final") || premature.contains("non-final"));

    let balance_before_cancellation = vault_balance(&rpc, &config);
    anzen_wallet::cancel_emergency_access(dir.path(), &rpc).unwrap();
    rpc.mine(1, &mining_address).unwrap();
    assert_eq!(rpc.scan_connectors(&config).unwrap().len(), 1);
    assert_eq!(vault_balance(&rpc, &config), balance_before_cancellation);
    let cancelled = anzen_wallet::withdraw_emergency_access(dir.path(), &rpc).unwrap_err();
    assert!(format!("{cancelled:#}").contains("missingorspent"));
}

#[test]
#[ignore = "requires a disposable Bitcoin Core regtest node"]
fn real_regtest_releases_emergency_access_after_one_week() {
    let dir = tempfile::tempdir().unwrap();
    let (wallet, signer, config) = devices(dir.path());
    let rpc = rpc_from_env();
    let vault_address = Address::from_str(&config.vault_address)
        .unwrap()
        .require_network(Network::Regtest)
        .unwrap();
    let mut hot = HotWallet::open_or_create(dir.path()).unwrap();
    let mining_address = hot.next_receive_address().unwrap();

    rpc.mine(1, &vault_address).unwrap();
    rpc.mine(100, &mining_address).unwrap();
    let approved = approve(&wallet, &signer, &rpc, 10_000_000, 50_000_000);
    wallet.activate_policy(&rpc, &approved).unwrap();
    rpc.mine(1, &mining_address).unwrap();

    let relative_lock_base = rpc.chain_info().unwrap().median_time;
    anzen_wallet::initiate_emergency_access(dir.path(), &rpc).unwrap();
    rpc.mine(1, &mining_address).unwrap();
    let premature = anzen_wallet::withdraw_emergency_access(dir.path(), &rpc).unwrap_err();
    let premature = format!("{premature:#}");
    assert!(premature.contains("non-BIP68-final") || premature.contains("non-final"));

    let unlock_time = u32::try_from(relative_lock_base).unwrap() + EMERGENCY_ACCESS_DELAY_SECONDS;
    advance_mtp(&rpc, unlock_time, &mining_address);
    anzen_wallet::withdraw_emergency_access(dir.path(), &rpc).unwrap();
    rpc.mine(1, &mining_address).unwrap();
    assert_eq!(rpc.scan_connectors(&config).unwrap().len(), 1);

    let mut hot = HotWallet::open_or_create(dir.path()).unwrap();
    rpc.sync_hot_wallet(&mut hot).unwrap();
    assert!(hot.wallet.balance().total().to_sat() >= 50_000_000);
}

#[test]
#[ignore = "requires a disposable Bitcoin Core regtest node"]
fn real_regtest_hww_revokes_every_live_policy_controller() {
    let dir = tempfile::tempdir().unwrap();
    let (wallet, signer, config) = devices(dir.path());
    let rpc = rpc_from_env();
    let vault_address = Address::from_str(&config.vault_address)
        .unwrap()
        .require_network(Network::Regtest)
        .unwrap();
    let mut hot = HotWallet::open_or_create(dir.path()).unwrap();
    let mining_address = hot.next_receive_address().unwrap();
    let safe_destination = hot.next_change_address().unwrap();
    rpc.mine(1, &vault_address).unwrap();
    rpc.mine(100, &mining_address).unwrap();

    let approved = approve(&wallet, &signer, &rpc, 10_000_000, 50_000_000);
    let schedule = wallet.activate_policy(&rpc, &approved).unwrap();
    rpc.mine(1, &mining_address).unwrap();
    let controllers = rpc.scan_connectors(&config).unwrap();
    assert_eq!(controllers.len(), 2);
    let balance_before_revocation = vault_balance(&rpc, &config);

    // The host scans; the offline signer reviews and signs through its own controller leaf.
    let request = offline_action::request(
        &config,
        OfflineAction::RevokeActions,
        &controllers,
        &safe_destination.to_string(),
    )
    .unwrap();
    let review = signer.review_offline_action(&request).unwrap();
    let approval = signer.approve_offline_action(&review.review_id).unwrap();
    let revocation = offline_action::finalize(&config, &approval).unwrap();
    assert_eq!(revocation.output.len(), 1);
    assert_eq!(
        revocation.output[0].script_pubkey,
        safe_destination.script_pubkey()
    );
    assert_ne!(
        revocation.output[0].script_pubkey,
        controllers[0].txout.script_pubkey
    );
    rpc.broadcast(&revocation).unwrap();
    rpc.mine(1, &mining_address).unwrap();
    assert!(rpc.scan_connectors(&config).unwrap().is_empty());
    assert_eq!(vault_balance(&rpc, &config), balance_before_revocation);

    let first_unlock = u32::try_from(rpc.chain_info().unwrap().median_time).unwrap()
        + MONTHLY_ALLOWANCE_DELAY_SECONDS;
    advance_mtp(&rpc, first_unlock, &mining_address);
    let allowance = anzen_wallet::broadcast_monthly(
        dir.path(),
        &rpc,
        schedule.entries[0].step,
        TransactionKind::Authorization,
    )
    .unwrap_err();
    assert!(format!("{allowance:#}").contains("missingorspent"));
    let emergency = anzen_wallet::initiate_emergency_access(dir.path(), &rpc).unwrap_err();
    assert!(format!("{emergency:#}").contains("missingorspent"));
}

fn vault_balance(rpc: &BitcoinCoreBackend, config: &VaultConfig) -> u64 {
    rpc.scan_vault(config)
        .unwrap()
        .iter()
        .map(|utxo| utxo.txout.value.to_sat())
        .sum()
}

fn advance_mtp(rpc: &BitcoinCoreBackend, timestamp: u32, mining_address: &Address) {
    rpc.set_mock_time(u64::from(timestamp) + 60).unwrap();
    while rpc.chain_info().unwrap().median_time <= u64::from(timestamp) {
        rpc.mine(1, mining_address).unwrap();
    }
}
