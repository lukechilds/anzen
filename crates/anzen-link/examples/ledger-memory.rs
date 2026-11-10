//! Print a development build's stack and heap marks (`--features measure` in `ledger-app`).
//! `cargo run -p anzen-link --example ledger-memory -- 127.0.0.1:9998 [reset]`

use anzen_link::ledger::{HardwareSigner, Speculos};

#[tokio::main(flavor = "current_thread")]
async fn main() -> anyhow::Result<()> {
    let mut arguments = std::env::args().skip(1);
    let address = arguments
        .next()
        .unwrap_or_else(|| "127.0.0.1:9999".to_owned());
    let reset = arguments.next().is_some();
    let mut signer = HardwareSigner::open(Speculos::connect(&address).await?).await?;
    let [size, deepest, now, free] = signer.memory(reset).await?;
    println!(
        "stack: {deepest} of {size} bytes at its deepest, {now} now; heap: largest free block {free} bytes"
    );
    Ok(())
}
