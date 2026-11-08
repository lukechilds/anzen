use anzen_chain::{BitcoinCoreBackend, Blockchain, RpcConfig};
use anzen_common::HWW_RECOVERY_SECONDS;
use anzen_common::cloud_backup::{self, CloudRecoveryBackup};
use anzen_common::storage::{
    HWW_DEVICE_FILE, PHONE_BACKUP_FILE, PHONE_DEVICE_FILE, VaultConfig, load_config, load_device,
    load_device_keys, read_json,
};
use anzen_protocol::wire::OfflineAction;
use anzen_signer_app::{self, application::SignerApplication};
use anzen_wallet::offline_action;
use anzen_wallet::recovery::RecoveryClock;
use anzen_wallet::{
    self, HotWallet, HotWalletBackend, api::PolicyLimits, application::WalletApplication,
};
use bitcoin::{Address, Network};
use bitcoincore_rpc::RpcApi;
use std::{env, fs, path::Path, str::FromStr};

struct Devices {
    wallet: WalletApplication,
    signer: SignerApplication,
    config: VaultConfig,
    phone_mnemonic: String,
}

/// Both simulated devices in one directory, paired and holding the first cloud backup.
fn initialize(data_dir: &Path) -> Devices {
    let phone = anzen_wallet::initialize(data_dir, Network::Regtest).unwrap();
    anzen_signer_app::initialize(data_dir, Network::Regtest).unwrap();
    let wallet = WalletApplication::new(data_dir.into(), false);
    let signer = SignerApplication::new(data_dir.into(), false);
    let config = wallet
        .pair_signer(&signer.public_identity().unwrap())
        .unwrap();
    signer.accept_vault(&config).unwrap();
    let request = wallet.propose_backup().unwrap();
    let review = signer.review_backup(&request).unwrap();
    wallet
        .store_backup(&signer.approve_backup(&review.review_id, &request).unwrap())
        .unwrap();
    Devices {
        wallet,
        signer,
        config,
        phone_mnemonic: phone.mnemonic,
    }
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

fn address(text: &str) -> Address {
    Address::from_str(text)
        .unwrap()
        .require_network(Network::Regtest)
        .unwrap()
}

/// The host scans and broadcasts; the offline signer only reviews and signs.
fn hww_recover(
    devices: &Devices,
    rpc: &BitcoinCoreBackend,
    destination: &Address,
) -> anyhow::Result<bitcoin::Txid> {
    let config = devices.signer.export_recovery_bundle()?.config;
    let utxos = rpc.scan_vault(&config)?;
    let clock = RecoveryClock::read(rpc, &rpc.chain_tip()?, &utxos)?;
    let request =
        offline_action::mature_recovery_request(&config, &utxos, &clock, &destination.to_string())?;
    sign_and_broadcast(devices, rpc, &config, &request)
}

fn sign_and_broadcast(
    devices: &Devices,
    rpc: &BitcoinCoreBackend,
    config: &VaultConfig,
    request: &anzen_protocol::wire::OfflineActionRequest,
) -> anyhow::Result<bitcoin::Txid> {
    let review = devices.signer.review_offline_action(request)?;
    let approval = devices.signer.approve_offline_action(&review.review_id)?;
    let transaction = offline_action::finalize(config, &approval)?;
    rpc.broadcast(&transaction)
}

/// Mine eleven blocks at mock time `time`, which moves the chain's median time past to it, or
/// one second past it when earlier blocks already carry that time. Returns the new value.
fn advance_median_time(rpc: &BitcoinCoreBackend, time: u64, address: &Address) -> u64 {
    rpc.set_mock_time(time).unwrap();
    rpc.mine(11, address).unwrap();
    let median_time = rpc.chain_info().unwrap().median_time;
    assert!((time..=time + 1).contains(&median_time), "{median_time}");
    median_time
}

#[test]
#[ignore = "requires a disposable Bitcoin Core regtest node"]
fn real_regtest_enforces_the_signer_time_lock_and_rotates_the_phone_epoch() {
    let rpc = rpc_from_env();
    let hww_dir = tempfile::tempdir().unwrap();
    let rotation_dir = tempfile::tempdir().unwrap();
    let hww_vault = initialize(hww_dir.path());
    let rotation_vault = initialize(rotation_dir.path());
    let mut destination_hot = HotWallet::open_or_create(hww_dir.path()).unwrap();
    let destination = destination_hot.next_receive_address().unwrap();

    // Fund the rotating wallet first: regtest halves its subsidy every 150 blocks.
    let rotation_hot_address = HotWallet::open_or_create(rotation_dir.path())
        .unwrap()
        .next_receive_address()
        .unwrap();
    rpc.mine(1, &rotation_hot_address).unwrap();

    rpc.mine(1, &address(&hww_vault.config.vault_address))
        .unwrap();
    let hww_confirmation = rpc.chain_info().unwrap().blocks;
    rpc.mine(1, &address(&rotation_vault.config.vault_address))
        .unwrap();
    rpc.mine(100, &destination).unwrap();
    // Activate a real annual policy so the rotation must carry it to the new keys.
    let policy = rotation_vault
        .wallet
        .propose_policy(
            &rpc,
            PolicyLimits {
                monthly_limit_sats: 10_000_000,
                emergency_access_limit_sats: 20_000_000,
            },
        )
        .unwrap();
    let policy_review = rotation_vault.signer.review_policy(&policy).unwrap();
    let approved_policy = rotation_vault
        .signer
        .approve_policy(&policy_review.review_id)
        .unwrap();
    rotation_vault
        .wallet
        .activate_policy(&rpc, &approved_policy)
        .unwrap();
    rpc.mine(1, &destination).unwrap();
    let hot_balance_before = {
        let mut hot = HotWallet::open_or_create(rotation_dir.path()).unwrap();
        rpc.sync_hot_wallet(&mut hot).unwrap();
        hot.wallet.balance().confirmed
    };
    assert!(hot_balance_before.to_sat() > 1_000);

    // The phone has no path of its own: with the phone gone, only the signer's time lock is left.
    fs::remove_file(hww_dir.path().join(PHONE_DEVICE_FILE)).unwrap();
    fs::remove_file(hww_dir.path().join(PHONE_BACKUP_FILE)).unwrap();
    let early_hww = hww_recover(&hww_vault, &rpc, &destination).unwrap_err();
    assert!(
        format!("{early_hww:#}").contains("not mature"),
        "{early_hww:#}"
    );

    // BIP68 counts from the median time past of the block before the coin's.
    let coin_hash = rpc.client.get_block_hash(hww_confirmation - 1).unwrap();
    let coin_time = rpc
        .client
        .get_block_header_info(&coin_hash)
        .unwrap()
        .median_time
        .unwrap() as u64;
    let opens_at = coin_time + u64::from(HWW_RECOVERY_SECONDS);
    let config = hww_vault.signer.export_recovery_bundle().unwrap().config;
    let request = offline_action::request(
        &config,
        OfflineAction::HwwRecovery,
        &rpc.scan_vault(&config).unwrap(),
        &destination.to_string(),
    )
    .unwrap();

    // Seconds short of ~388 days: the wallet refuses, and so does consensus.
    assert!(advance_median_time(&rpc, opens_at - 2, &destination) < opens_at);
    let early_hww = hww_recover(&hww_vault, &rpc, &destination).unwrap_err();
    assert!(
        format!("{early_hww:#}").contains("not mature"),
        "{early_hww:#}"
    );
    let early_consensus = sign_and_broadcast(&hww_vault, &rpc, &config, &request).unwrap_err();
    assert!(
        format!("{early_consensus:#}").contains("non-BIP68-final"),
        "{early_consensus:#}"
    );

    advance_median_time(&rpc, opens_at, &destination);
    let hww_sweep = hww_recover(&hww_vault, &rpc, &destination).unwrap();
    rpc.mine(1, &destination).unwrap();
    assert_eq!(
        rpc.client
            .get_raw_transaction(&hww_sweep, None)
            .map(|transaction| transaction.input.len())
            .unwrap_or(1),
        1
    );

    // A working phone rotates to a fresh key; the HWW never sees either seed.
    let old_config = load_config(rotation_dir.path()).unwrap();
    let proposal = rotation_vault
        .wallet
        .propose_rotation(&rpc, false, |_| {})
        .unwrap();
    let review = rotation_vault
        .signer
        .review_phone_rotation(&proposal)
        .unwrap();
    assert!(review.revokes_connectors);
    let approved = rotation_vault
        .signer
        .approve_phone_rotation(&review.review_id, &proposal)
        .unwrap();
    let rotation = rotation_vault
        .wallet
        .activate_rotation(&rpc, &approved)
        .unwrap();
    rotation_vault
        .signer
        .adopt_phone_rotation(&rotation.request_id)
        .unwrap();
    assert!(rotation.controller_revocation_txid.is_some());
    let hot_sweep_txid = rotation.hot_wallet_sweep_txid.unwrap();
    assert_ne!(rotation.old_vault_address, rotation.new_vault_address);
    let new_phone_mnemonic = load_device(rotation_dir.path(), PHONE_DEVICE_FILE)
        .unwrap()
        .mnemonic;
    assert_ne!(new_phone_mnemonic, rotation_vault.phone_mnemonic);
    let rotated_backup: CloudRecoveryBackup =
        read_json(&rotation_dir.path().join(PHONE_BACKUP_FILE)).unwrap();
    let hww_keys = load_device_keys(rotation_dir.path(), HWW_DEVICE_FILE).unwrap();
    let payload = cloud_backup::decrypt_with_hww(&rotated_backup, &hww_keys.seed).unwrap();
    payload
        .validate_against(&load_config(rotation_dir.path()).unwrap())
        .unwrap();
    assert_eq!(payload.phone_mnemonic, new_phone_mnemonic);
    assert!(rotation_dir.path().join(HWW_DEVICE_FILE).exists());
    let renewed_schedule = rotation.renewed_schedule.as_ref().unwrap();
    assert_eq!(renewed_schedule.monthly_limit_sats, 10_000_000);
    assert_eq!(renewed_schedule.entries.len(), 12);
    assert_eq!(renewed_schedule.emergency_access_limit_sats, 20_000_000);
    assert_eq!(
        renewed_schedule
            .emergency_access
            .as_ref()
            .unwrap()
            .amount_sats,
        20_000_000
    );
    assert!(
        rotation_dir
            .path()
            .join("history")
            .join(format!("rotation-{}", rotation.sweep_txid))
            .join("anzen.json")
            .exists()
    );
    rpc.mine(1, &destination).unwrap();
    let new_config = load_config(rotation_dir.path()).unwrap();
    assert_eq!(new_config.monthly_limit_sats, 10_000_000);
    assert_eq!(new_config.emergency_access_limit_sats, 20_000_000);
    assert_eq!(rpc.scan_vault(&old_config).unwrap().len(), 0);
    let renewed_utxos = rpc.scan_vault(&new_config).unwrap();
    let revocation_txid = rotation.controller_revocation_txid.unwrap();
    assert_eq!(
        renewed_utxos
            .iter()
            .filter(|utxo| utxo.outpoint.txid.to_string() == renewed_schedule.rollover_txid)
            .count(),
        2,
        "the renewed rollover must create one allowance-chain head and one remainder"
    );
    // The HWW returned the revoked connectors' small remainder to the new vault it can verify.
    assert!(
        renewed_utxos
            .iter()
            .any(|utxo| utxo.outpoint.txid == revocation_txid)
    );
    assert_eq!(renewed_utxos.len(), 3);
    assert!(renewed_utxos.iter().any(|utxo| {
        utxo.outpoint.txid.to_string() == renewed_schedule.rollover_txid && utxo.outpoint.vout == 0
    }));
    assert!(renewed_utxos.iter().any(|utxo| {
        utxo.outpoint.txid.to_string() == renewed_schedule.rollover_txid && utxo.outpoint.vout == 1
    }));

    let replacement = HotWallet::open_or_create(rotation_dir.path()).unwrap();
    let expected = replacement
        .wallet
        .peek_address(bdk_wallet::KeychainKind::Internal, 0)
        .address
        .script_pubkey();
    let swept = rpc
        .client
        .get_tx_out(&hot_sweep_txid, 0, Some(true))
        .unwrap()
        .unwrap();
    assert_eq!(swept.script_pub_key.hex, expected.as_bytes());
    assert!(swept.confirmations > 0);
    assert!(swept.value.to_sat() > 0 && swept.value < hot_balance_before);
    let archive = rotation_dir
        .path()
        .join("history")
        .join(format!("rotation-{}", rotation.sweep_txid));
    let mut old_hot = HotWallet::open_or_create(&archive).unwrap();
    rpc.sync_hot_wallet(&mut old_hot).unwrap();
    assert_eq!(old_hot.wallet.balance().total().to_sat(), 0);
    let old_keys = anzen_common::storage::load_device_keys(&archive, PHONE_DEVICE_FILE).unwrap();
    assert_eq!(old_keys.mnemonic.to_string(), rotation_vault.phone_mnemonic);
    assert!(archive.join(PHONE_BACKUP_FILE).exists());
    // The signer now pairs with the rotated vault and its renewed policy limits.
    assert_eq!(
        rotation_vault
            .signer
            .export_recovery_bundle()
            .unwrap()
            .config
            .vault_address,
        rotation.new_vault_address
    );
}
