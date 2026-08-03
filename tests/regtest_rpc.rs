use bitcoin::{Address, Network};
use std::{env, str::FromStr};
use vault_cli::{
    hot::HotWallet,
    rpc::{RegtestRpc, RpcConfig},
    state::{initialize, load_config},
};

fn rpc_from_env() -> RegtestRpc {
    RegtestRpc::connect(&RpcConfig {
        url: env::var("VAULT_RPC_URL").unwrap_or_else(|_| "http://127.0.0.1:18443".to_owned()),
        user: env::var("VAULT_RPC_USER").unwrap_or_else(|_| "vault".to_owned()),
        password: env::var("VAULT_RPC_PASSWORD").unwrap_or_else(|_| "vault".to_owned()),
    })
    .unwrap()
}

#[test]
#[ignore = "requires a disposable Bitcoin Core regtest node"]
fn real_regtest_scans_vault_and_syncs_bdk_hot_wallet() {
    let dir = tempfile::tempdir().unwrap();
    initialize(dir.path(), 10_000_000).unwrap();
    let config = load_config(dir.path()).unwrap();
    let rpc = rpc_from_env();

    let mut hot = HotWallet::open_or_create(dir.path()).unwrap();
    let hot_address = hot.next_receive_address().unwrap();
    rpc.mine(1, &hot_address).unwrap();
    hot.sync(&rpc.client).unwrap();
    assert!(hot.wallet.balance().total().to_sat() > 0);

    let vault_address = Address::from_str(&config.vault_address)
        .unwrap()
        .require_network(Network::Regtest)
        .unwrap();
    rpc.mine(1, &vault_address).unwrap();
    let utxos = rpc.scan_vault(&config).unwrap();
    assert_eq!(utxos.len(), 1);
    assert_eq!(utxos[0].txout.script_pubkey, vault_address.script_pubkey());
    assert!(rpc.vault_balance(&config).unwrap().to_sat() > 0);
}
