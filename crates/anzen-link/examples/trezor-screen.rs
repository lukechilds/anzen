//! Look at the Trezor emulator's screen, or press its buttons, through its debug link.
//!
//! ```sh
//! cargo run -p anzen-link --example trezor-screen -- [--port 31324]            # what it shows
//! cargo run -p anzen-link --example trezor-screen -- [--port 31324] layout    # its components
//! cargo run -p anzen-link --example trezor-screen -- [--port 31324] approve   # or: reject
//! ```
//!
//! Emulator and debug firmware only: a real Trezor has no debug link.

use anzen_link::trezor::{DEFAULT_PORT, debug::DebugLink};

#[tokio::main(flavor = "current_thread")]
async fn main() -> anyhow::Result<()> {
    let mut arguments: Vec<String> = std::env::args().skip(1).collect();
    let mut port = DEFAULT_PORT;
    if let Some(at) = arguments.iter().position(|argument| argument == "--port") {
        port = arguments
            .get(at + 1)
            .and_then(|port| port.parse().ok())
            .ok_or_else(|| anyhow::anyhow!("--port takes a number"))?;
        arguments.drain(at..at + 2);
    }
    let mut debug = DebugLink::connect(port).await?;
    match arguments.first().map(String::as_str) {
        None => println!("{}", debug.screen().await?),
        Some("layout") => println!("{}", debug.layout().await?),
        Some("approve") => debug.press(true).await?,
        Some("reject") => debug.press(false).await?,
        Some(other) => anyhow::bail!("unknown command {other}: use layout, approve or reject"),
    }
    Ok(())
}
