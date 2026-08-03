use bitcoin::{Address, Network};
use std::{env, fs, str::FromStr};
use vault_cli::{
    HWW_RECOVERY_BLOCKS, PHONE_RECOVERY_BLOCKS,
    hot::HotWallet,
    recovery::{SweepPath, restore_phone_from_hww_backup, rotate_phone, sweep},
    rpc::{RegtestRpc, RpcConfig},
    state::{
        HWW_DEVICE_FILE, PHONE_BACKUP_FILE, PHONE_DEVICE_FILE, initialize, load_config,
        recover_phone_mnemonic,
    },
};

fn rpc_from_env() -> RegtestRpc {
    RegtestRpc::connect(&RpcConfig {
        url: env::var("VAULT_RPC_URL").unwrap_or_else(|_| "http://127.0.0.1:18443".to_owned()),
        user: env::var("VAULT_RPC_USER").unwrap_or_else(|_| "vault".to_owned()),
        password: env::var("VAULT_RPC_PASSWORD").unwrap_or_else(|_| "vault".to_owned()),
    })
    .unwrap()
}

fn address(text: &str) -> Address {
    Address::from_str(text)
        .unwrap()
        .require_network(Network::Regtest)
        .unwrap()
}

#[test]
#[ignore = "requires a disposable Bitcoin Core regtest node and mines 65,535 blocks"]
fn real_regtest_enforces_both_recovery_delays_and_rotates_the_phone_epoch() {
    let rpc = rpc_from_env();
    let phone_dir = tempfile::tempdir().unwrap();
    let hww_dir = tempfile::tempdir().unwrap();
    let rotation_dir = tempfile::tempdir().unwrap();
    let phone_vault = initialize(phone_dir.path()).unwrap();
    let hww_vault = initialize(hww_dir.path()).unwrap();
    let rotation_vault = initialize(rotation_dir.path()).unwrap();
    let mut phone_hot = HotWallet::open_or_create(phone_dir.path()).unwrap();
    let destination = phone_hot.next_receive_address().unwrap();

    rpc.mine(1, &address(&phone_vault.config.vault_address))
        .unwrap();
    let phone_confirmation = rpc.chain_info().unwrap().blocks;
    rpc.mine(1, &address(&hww_vault.config.vault_address))
        .unwrap();
    let hww_confirmation = rpc.chain_info().unwrap().blocks;
    rpc.mine(1, &address(&rotation_vault.config.vault_address))
        .unwrap();
    rpc.mine(100, &destination).unwrap();

    let early_phone = sweep(
        phone_dir.path(),
        &rpc,
        SweepPath::PhoneRecovery,
        &destination,
    )
    .unwrap_err();
    assert!(
        format!("{early_phone:#}").contains("blocks remaining"),
        "{early_phone:#}"
    );

    mine_until_next_height(
        &rpc,
        phone_confirmation + u64::from(PHONE_RECOVERY_BLOCKS),
        &destination,
    );
    let phone_sweep = sweep(
        phone_dir.path(),
        &rpc,
        SweepPath::PhoneRecovery,
        &destination,
    )
    .unwrap();
    assert_eq!(phone_sweep.input_count, 1);
    rpc.mine(1, &destination).unwrap();

    fs::remove_file(hww_dir.path().join(PHONE_DEVICE_FILE)).unwrap();
    fs::remove_file(hww_dir.path().join(PHONE_BACKUP_FILE)).unwrap();
    let early_hww = sweep(hww_dir.path(), &rpc, SweepPath::HwwRecovery, &destination).unwrap_err();
    assert!(
        format!("{early_hww:#}").contains("blocks remaining"),
        "{early_hww:#}"
    );

    mine_until_next_height(
        &rpc,
        hww_confirmation + u64::from(HWW_RECOVERY_BLOCKS),
        &destination,
    );
    let hww_sweep = sweep(hww_dir.path(), &rpc, SweepPath::HwwRecovery, &destination).unwrap();
    assert_eq!(hww_sweep.input_count, 1);
    rpc.mine(1, &destination).unwrap();

    fs::remove_file(rotation_dir.path().join(PHONE_DEVICE_FILE)).unwrap();
    let restored = restore_phone_from_hww_backup(rotation_dir.path()).unwrap();
    assert_eq!(restored, rotation_vault.phone_mnemonic);
    let old_config = rotation_vault.config;
    let rotation = rotate_phone(rotation_dir.path(), &rpc).unwrap();
    assert_ne!(rotation.old_address, rotation.new_address);
    assert_ne!(rotation.new_phone_mnemonic, restored);
    assert_eq!(
        recover_phone_mnemonic(rotation_dir.path()).unwrap(),
        rotation.new_phone_mnemonic
    );
    assert!(rotation_dir.path().join(HWW_DEVICE_FILE).exists());
    assert!(
        rotation_dir
            .path()
            .join("history")
            .join(format!("rotation-{}", rotation.sweep.txid))
            .join("vault.json")
            .exists()
    );
    rpc.mine(1, &destination).unwrap();
    let new_config = load_config(rotation_dir.path()).unwrap();
    assert_eq!(rpc.scan_vault(&old_config).unwrap().len(), 0);
    assert_eq!(rpc.scan_vault(&new_config).unwrap().len(), 1);
}

fn mine_until_next_height(rpc: &RegtestRpc, target_next_height: u64, address: &Address) {
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
