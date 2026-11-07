use anzen_chain::{BitcoinCoreBackend, RpcConfig};
use anzen_signer_app::{self, application::SignerApplication};
use anzen_wallet::{self, HotWallet, HotWalletBackend, application::WalletApplication};
use bitcoin::{Address, Network};
use bitcoincore_rpc::RpcApi;
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
fn real_regtest_scans_vault_and_syncs_bdk_hot_wallet() {
    let dir = tempfile::tempdir().unwrap();
    anzen_wallet::initialize(dir.path(), Network::Regtest).unwrap();
    anzen_signer_app::initialize(dir.path(), Network::Regtest).unwrap();
    let wallet = WalletApplication::new(dir.path().into(), false);
    let signer = SignerApplication::new(dir.path().into(), false);
    let config = wallet
        .pair_signer(&signer.public_identity().unwrap())
        .unwrap();
    signer.accept_vault(&config).unwrap();
    let rpc = rpc_from_env();

    let mut hot = HotWallet::open_or_create(dir.path()).unwrap();
    let hot_address = hot.next_receive_address().unwrap();
    rpc.mine(1, &hot_address).unwrap();
    rpc.sync_hot_wallet(&mut hot).unwrap();
    assert!(hot.wallet.balance().total().to_sat() > 0);

    let vault_address = Address::from_str(&config.vault_address)
        .unwrap()
        .require_network(Network::Regtest)
        .unwrap();
    rpc.mine(1, &vault_address).unwrap();
    rpc.mine(100, &hot_address).unwrap();
    let utxos = rpc.scan_vault(&config).unwrap();
    assert_eq!(utxos.len(), 1);
    assert_eq!(utxos[0].txout.script_pubkey, vault_address.script_pubkey());
    assert!(rpc.vault_balance(&config).unwrap().to_sat() > 0);

    // A protocol-built cooperative sweep, signed by both devices, is valid on a real node.
    let request = wallet.propose_sweep(&rpc, &config.vault_address).unwrap();
    let review = signer.review_sweep(&request).unwrap();
    assert_eq!(
        review.amount_sats + review.fee_sats,
        utxos[0].txout.value.to_sat()
    );
    let approval = signer.approve_sweep(&review.review_id).unwrap();
    let result = wallet.activate_sweep(&rpc, &approval).unwrap();
    let confirmed = rpc.client.get_raw_transaction(&result.txid, None).unwrap();
    assert_eq!(confirmed.compute_txid(), result.txid);
}
