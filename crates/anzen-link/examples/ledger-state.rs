//! Connect to a Ledger over Bluetooth and print the vault its Anzen app holds on each network.
//! `cargo run -p anzen-link --example ledger-state`

use anzen_link::ledger::{self, HardwareSigner};
use anzen_protocol::address::Network;
use std::time::Duration;

#[tokio::main(flavor = "multi_thread", worker_threads = 2)]
async fn main() -> anyhow::Result<()> {
    eprintln!("Scanning…");
    let found = ledger::find(None, Duration::from_secs(20)).await?;
    eprintln!("Found {}. Connecting…", found.name);
    let mut signer = HardwareSigner::open(found.connect().await?).await?;
    for network in [Network::Mainnet, Network::Mutinynet, Network::Regtest] {
        let key = signer.identity(network).await?;
        match signer.state(network).await? {
            Some(state) => println!(
                "{}: signer key {}, vault with phone key {}, limits {} / {} sats, policy approved: {}",
                network.name(),
                key.to_hex(),
                state.keys.phone.to_hex(),
                state.limits.monthly_limit_sats,
                state.limits.emergency_access_limit_sats,
                state.anchor.is_some()
            ),
            None => println!("{}: signer key {}, no vault", network.name(), key.to_hex()),
        }
    }
    signer.into_link().disconnect().await
}
