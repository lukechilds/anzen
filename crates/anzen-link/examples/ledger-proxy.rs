//! A TCP door to a Ledger over Bluetooth, speaking the proxy protocol Ledger's own tools use
//! (`LEDGER_PROXY_ADDRESS` / `LEDGER_PROXY_PORT`): for loading an app without a cable.
//! `cargo run -p anzen-link --example ledger-proxy [port]`

use anzen_link::ledger::{self, Exchange};
use std::time::Duration;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
};

#[tokio::main(flavor = "multi_thread", worker_threads = 2)]
async fn main() -> anyhow::Result<()> {
    let port: u16 = std::env::args()
        .nth(1)
        .map_or(Ok(9998), |port| port.parse())?;
    eprintln!("Scanning…");
    let found = ledger::find(None, Duration::from_secs(30)).await?;
    eprintln!("Found {}. Connecting…", found.name);
    let mut ledger = found.connect().await?;
    let listener = TcpListener::bind(("0.0.0.0", port)).await?;
    eprintln!("Ready on port {port}.");
    loop {
        let (mut stream, _) = listener.accept().await?;
        eprintln!("Client connected.");
        loop {
            let mut length = [0_u8; 4];
            if stream.read_exact(&mut length).await.is_err() {
                break;
            }
            let mut apdu = vec![0_u8; u32::from_be_bytes(length) as usize];
            stream.read_exact(&mut apdu).await?;
            // Installing waits for approvals on the device.
            let answer = ledger.exchange(&apdu, Duration::from_secs(300)).await?;
            eprintln!(
                "{} … → {:#06x} ({} bytes)",
                apdu.iter()
                    .take(5)
                    .map(|byte| format!("{byte:02x}"))
                    .collect::<String>(),
                answer.status,
                answer.data.len()
            );
            stream
                .write_all(&(answer.data.len() as u32).to_be_bytes())
                .await?;
            stream.write_all(&answer.data).await?;
            stream.write_all(&answer.status.to_be_bytes()).await?;
        }
        eprintln!("Client disconnected.");
    }
}
