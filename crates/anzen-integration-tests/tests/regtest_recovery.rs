use anzen_chain::{BitcoinCoreBackend, Blockchain, RpcConfig};
use anzen_common::HWW_RECOVERY_BLOCKS;
use anzen_common::PHONE_RECOVERY_BLOCKS;
use anzen_common::social::{self, CloudRecoveryBackup};
use anzen_common::storage::{
    HWW_DEVICE_FILE, PHONE_BACKUP_FILE, PHONE_DEVICE_FILE, VaultConfig, load_config, load_device,
    read_json,
};
use anzen_signer_app::{self, application::SignerApplication};
use anzen_wallet::offline_action;
use anzen_wallet::recovery::SweepResult;
use anzen_wallet::{
    self, HotWallet, HotWalletBackend,
    api::PolicyLimits,
    application::{RecoveryContact, WalletApplication},
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

fn phone_recover(
    devices: &Devices,
    rpc: &BitcoinCoreBackend,
    destination: &Address,
) -> anyhow::Result<SweepResult> {
    devices.wallet.phone_recovery(rpc, &destination.to_string())
}

/// The host scans and broadcasts; the offline signer only reviews and signs.
fn hww_recover(
    devices: &Devices,
    rpc: &BitcoinCoreBackend,
    destination: &Address,
) -> anyhow::Result<bitcoin::Txid> {
    let config = devices.signer.export_recovery_bundle()?.config;
    let request = offline_action::mature_recovery_request(
        &config,
        &rpc.scan_vault(&config)?,
        rpc.chain_tip()?.height,
        &destination.to_string(),
    )?;
    let review = devices.signer.review_offline_action(&request)?;
    let approval = devices.signer.approve_offline_action(&review.review_id)?;
    let transaction = offline_action::finalize(&config, &approval)?;
    rpc.broadcast(&transaction)
}

#[test]
#[ignore = "requires a disposable Bitcoin Core regtest node and mines 65,535 blocks"]
fn real_regtest_enforces_both_recovery_delays_and_rotates_the_phone_epoch() {
    let rpc = rpc_from_env();
    let phone_dir = tempfile::tempdir().unwrap();
    let hww_dir = tempfile::tempdir().unwrap();
    let rotation_dir = tempfile::tempdir().unwrap();
    let phone_vault = initialize(phone_dir.path());
    let hww_vault = initialize(hww_dir.path());
    let rotation_vault = initialize(rotation_dir.path());
    let recovery_friend = social::generate_friend_key("Rotation recovery friend").unwrap();
    let contact = RecoveryContact {
        version: 1,
        name: "Rotation recovery friend".into(),
        fingerprint: recovery_friend.fingerprint.clone(),
        public_key_armored: recovery_friend.public_key_armored.clone(),
        transport_public_key: String::new(),
    };
    let backup_request = rotation_vault
        .wallet
        .propose_backup_with_contact(&contact)
        .unwrap();
    let backup_review = rotation_vault
        .signer
        .review_backup(&backup_request)
        .unwrap();
    rotation_vault
        .wallet
        .store_backup(
            &rotation_vault
                .signer
                .approve_backup(&backup_review.review_id, &backup_request)
                .unwrap(),
        )
        .unwrap();
    let mut phone_hot = HotWallet::open_or_create(phone_dir.path()).unwrap();
    let destination = phone_hot.next_receive_address().unwrap();

    // Fund both halves of the rotating wallet before the recovery delay. Regtest halves its
    // subsidy every 150 blocks, so fresh mining after 65,535 blocks cannot fund another test.
    let rotation_hot_address = HotWallet::open_or_create(rotation_dir.path())
        .unwrap()
        .next_receive_address()
        .unwrap();
    rpc.mine(1, &rotation_hot_address).unwrap();

    rpc.mine(1, &address(&phone_vault.config.vault_address))
        .unwrap();
    let phone_confirmation = rpc.chain_info().unwrap().blocks;
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

    let early_phone = phone_recover(&phone_vault, &rpc, &destination).unwrap_err();
    assert!(
        format!("{early_phone:#}").contains("blocks remaining"),
        "{early_phone:#}"
    );

    mine_until_next_height(
        &rpc,
        phone_confirmation + u64::from(PHONE_RECOVERY_BLOCKS),
        &destination,
    );
    let phone_sweep = phone_recover(&phone_vault, &rpc, &destination).unwrap();
    assert_eq!(phone_sweep.input_count, 1);
    rpc.mine(1, &destination).unwrap();

    fs::remove_file(hww_dir.path().join(PHONE_DEVICE_FILE)).unwrap();
    fs::remove_file(hww_dir.path().join(PHONE_BACKUP_FILE)).unwrap();
    let early_hww = hww_recover(&hww_vault, &rpc, &destination).unwrap_err();
    assert!(
        format!("{early_hww:#}").contains("blocks remaining"),
        "{early_hww:#}"
    );

    mine_until_next_height(
        &rpc,
        hww_confirmation + u64::from(HWW_RECOVERY_BLOCKS),
        &destination,
    );
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
    let friend_payload = social::decrypt_with_friend(
        &rotated_backup,
        recovery_friend.private_key_armored.as_bytes(),
    )
    .unwrap();
    friend_payload
        .validate_against(&load_config(rotation_dir.path()).unwrap())
        .unwrap();
    assert_eq!(friend_payload.phone_mnemonic, new_phone_mnemonic);
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

fn mine_until_next_height(rpc: &BitcoinCoreBackend, target_next_height: u64, address: &Address) {
    let tip = rpc.chain_info().unwrap().blocks;
    let mut remaining = target_next_height.saturating_sub(tip + 1);
    while remaining > 0 {
        let batch = remaining.min(5_000);
        let median_time = rpc.chain_info().unwrap().median_time;
        rpc.set_mock_time(median_time + batch + 60).unwrap();
        let mined = rpc.mine(batch, address).unwrap();
        assert_eq!(mined.len() as u64, batch);
        remaining -= batch;
    }
    assert_eq!(rpc.chain_info().unwrap().blocks + 1, target_next_height);
}
