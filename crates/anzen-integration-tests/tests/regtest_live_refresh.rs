//! Live updates on regtest: automatic refreshes must stay cheap however long the chain is.
use anzen_chain::{BitcoinCoreBackend, RpcConfig};
use anzen_wallet::application::WalletApplication;
use bitcoin::{Address, Network};
use std::{env, str::FromStr, time::Instant};

#[test]
#[ignore = "requires a disposable Bitcoin Core regtest node"]
fn background_refresh_never_walks_a_long_regtest_catch_up() {
    let rpc = BitcoinCoreBackend::connect(
        &RpcConfig {
            url: env::var("ANZEN_RPC_URL").unwrap_or_else(|_| "http://127.0.0.1:18443".to_owned()),
            user: env::var("ANZEN_RPC_USER").unwrap_or_else(|_| "anzen".to_owned()),
            password: env::var("ANZEN_RPC_PASSWORD").unwrap_or_else(|_| "anzen".to_owned()),
        },
        Network::Regtest,
    )
    .unwrap();
    // Make the chain long enough that walking it from genesis is a long catch-up.
    let miner_dir = tempfile::tempdir().unwrap();
    let miner = WalletApplication::new(miner_dir.path().into(), false);
    miner.initialize(Network::Regtest).unwrap();
    let mined_to = Address::from_str(&miner.receive_address().unwrap())
        .unwrap()
        .require_network(Network::Regtest)
        .unwrap();
    let tip = rpc.chain_info().unwrap().blocks;
    if tip < 1_100 {
        rpc.mine(1_100 - tip, &mined_to).unwrap();
    }
    let dir = tempfile::tempdir().unwrap();
    let app = WalletApplication::new(dir.path().into(), false);
    app.initialize(Network::Regtest).unwrap();

    // A seed generated here with no address revealed yet cannot have been paid, so its first
    // sync starts at the tip.
    let started = Instant::now();
    let first = app.background_wallet_snapshot(&rpc).unwrap();
    assert!(first.behind_blocks.is_none() && started.elapsed().as_secs() < 10);

    // The same seed restored elsewhere may have history: the automatic refresh leaves the
    // genesis walk to an explicit sync instead of stalling the device.
    let restored = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(restored.path().join("phone")).unwrap();
    std::fs::copy(
        dir.path().join("phone/device.json"),
        restored.path().join("phone/device.json"),
    )
    .unwrap();
    let started = Instant::now();
    let copy = WalletApplication::new(restored.path().into(), false);
    assert!(
        copy.background_wallet_snapshot(&rpc)
            .unwrap()
            .behind_blocks
            .is_some()
    );
    assert!(started.elapsed().as_secs() < 10);

    // A small change is caught up automatically.
    let address = Address::from_str(&app.receive_address().unwrap())
        .unwrap()
        .require_network(Network::Regtest)
        .unwrap();
    rpc.mine(10, &address).unwrap();
    assert!(
        app.background_wallet_snapshot(&rpc)
            .unwrap()
            .behind_blocks
            .is_none()
    );

    // A time jump is left for an explicit sync instead of stalling the device.
    rpc.mine(1_001, &address).unwrap();
    let started = Instant::now();
    assert_eq!(
        app.background_wallet_snapshot(&rpc).unwrap().behind_blocks,
        Some(1_001)
    );
    assert!(started.elapsed().as_secs() < 10);
    assert!(app.wallet_snapshot(&rpc).unwrap().behind_blocks.is_none());
    assert!(
        app.background_wallet_snapshot(&rpc)
            .unwrap()
            .behind_blocks
            .is_none()
    );
}
