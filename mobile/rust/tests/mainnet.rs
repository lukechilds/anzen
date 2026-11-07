//! Read-only network smoke tests over the native WebSocket client. They create no wallet and
//! never sign or broadcast anything.
use anzen_wallet::{
    api::{Blockchain, ElectrumBackend, Network, WebSocketConnector},
    watch::{ChainWatcher, native::Watch},
};
use std::{thread, time::Duration};

#[test]
#[ignore = "requires public Electrum connectivity; read-only, no wallet keys"]
fn mainnet_electrum_backend_checks_the_real_network_and_reads_tip() {
    let backend = ElectrumBackend::new(Network::Bitcoin, WebSocketConnector).unwrap();
    let tip = backend.chain_tip().unwrap();
    assert_eq!(tip.network, Network::Bitcoin);
    assert!(tip.height > 800_000);
}

#[test]
#[ignore = "requires public Electrum connectivity; read-only, no wallet keys"]
fn mainnet_listener_subscribes_without_reporting_spurious_changes() {
    let watch = Watch::spawn(ChainWatcher::new(Network::Bitcoin).unwrap(), None).unwrap();
    // A well-known script hash (the genesis coinbase output), subscribed like a wallet address.
    watch.set_targets(vec![
        "740485f380ff6379d11ef6fe7d7cdd68aea7f8bd0d953d9fdf3531fb7d531833".into(),
    ]);
    thread::sleep(Duration::from_secs(8));
    // Subscribing reports one refresh; a new block could also arrive meanwhile.
    use anzen_wallet::watch::Change;
    let changes = watch.changes();
    assert!(changes.contains(&Change::Subscribed));
    assert!(
        changes
            .iter()
            .all(|change| matches!(change, Change::Subscribed | Change::Block))
    );
}
