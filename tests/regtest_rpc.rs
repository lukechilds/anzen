use anzen::{
    cold_wallet,
    core::{
        chain::{BitcoinCoreBackend, RpcConfig},
        keys::DeviceKeys,
        policy::{SpendPath, VaultPolicy},
        storage::{HWW_DEVICE_FILE, PHONE_DEVICE_FILE, initialize_vault, load_config, load_device},
        transactions::{create_vault_psbt, finalize_vault_psbt, sign_vault_psbt},
    },
    hot_wallet::{self, HotWallet, HotWalletBackend},
};
use bitcoin::{
    Address, Amount, Network, ScriptBuf, Sequence, Transaction, TxIn, TxOut, Witness, absolute,
    key::Secp256k1, transaction::Version,
};
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
    hot_wallet::initialize(dir.path(), Network::Regtest).unwrap();
    cold_wallet::initialize(dir.path(), Network::Regtest).unwrap();
    initialize_vault(dir.path()).unwrap();
    let config = load_config(dir.path()).unwrap();
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

    let utxo = &utxos[0];
    let unsigned = Transaction {
        version: Version::TWO,
        lock_time: absolute::LockTime::ZERO,
        input: vec![TxIn {
            previous_output: utxo.outpoint,
            script_sig: ScriptBuf::new(),
            sequence: Sequence::MAX,
            witness: Witness::new(),
        }],
        output: vec![TxOut {
            value: Amount::from_sat(utxo.txout.value.to_sat() - 1_000),
            script_pubkey: vault_address.script_pubkey(),
        }],
    };
    let policy = VaultPolicy::from_descriptor(&config.vault_descriptor).unwrap();
    let secp = Secp256k1::new();
    let phone = DeviceKeys::parse(
        &secp,
        &load_device(dir.path(), PHONE_DEVICE_FILE).unwrap().mnemonic,
    )
    .unwrap();
    let hww = DeviceKeys::parse(
        &secp,
        &load_device(dir.path(), HWW_DEVICE_FILE).unwrap().mnemonic,
    )
    .unwrap();
    let mut psbt = create_vault_psbt(unsigned, std::slice::from_ref(&utxo.txout), &policy).unwrap();
    sign_vault_psbt(
        &mut psbt,
        &policy,
        SpendPath::Cooperative,
        &phone.vault_keypair,
    )
    .unwrap();
    sign_vault_psbt(
        &mut psbt,
        &policy,
        SpendPath::Cooperative,
        &hww.vault_keypair,
    )
    .unwrap();
    let signed = finalize_vault_psbt(psbt).unwrap();
    let txid = rpc.client.send_raw_transaction(&signed).unwrap();
    assert_eq!(txid, signed.compute_txid());
}
