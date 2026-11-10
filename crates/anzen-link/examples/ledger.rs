//! Find a Ledger over Bluetooth and ask it which app is open.
//! `cargo run -p anzen-link --example ledger`

use anzen_link::ledger::{self, apdu};
use std::time::Duration;

#[tokio::main(flavor = "multi_thread", worker_threads = 2)]
async fn main() -> anyhow::Result<()> {
    eprintln!("Scanning…");
    let found = ledger::find(std::env::args().nth(1).as_deref(), Duration::from_secs(20)).await?;
    eprintln!("Found {} ({}). Connecting…", found.name, found.model);
    let mut ledger = found.connect().await?;
    eprintln!("Connected, {}-byte frames.", ledger.mtu());
    // The app name and version command every Ledger app and the dashboard answer.
    let answer = ledger
        .exchange(&apdu(0xb0, 0x01, 0, 0, &[])?, Duration::from_secs(10))
        .await?
        .ok()?;
    let mut fields = Vec::new();
    let mut rest = &answer[1..];
    while let [length, tail @ ..] = rest {
        let (field, tail) = tail.split_at(usize::from(*length).min(tail.len()));
        fields.push(String::from_utf8_lossy(field).into_owned());
        rest = tail;
    }
    println!(
        "Open app: {} {}",
        fields.first().map_or("?", String::as_str),
        fields.get(1).map_or("", String::as_str)
    );
    ledger.disconnect().await
}
