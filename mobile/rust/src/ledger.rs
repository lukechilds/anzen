//! The phone's Bluetooth link to a Ledger signer (a proof of concept, like the Ledger app).
//!
//! The app's signer flows speak protocol messages to a link; here the link is Bluetooth and the
//! signer is the Anzen app on a Ledger. Rust owns the connection: the app asks to connect, sends
//! one message at a time and waits for its answer while the person reviews it on the Ledger's
//! screen. Calls block, so the native layer runs them off the main thread.
//!
//! The link stays open between requests, as it does in Ledger's own apps. The first connection
//! pairs the phone with the Ledger, which shows a code to confirm; dropping the link while that
//! screen is up leaves the Ledger showing it, so the app never hangs up just because one request
//! finished. An idle link is reused by the next request and closed when the app asks.

use anyhow::{Context, Result};
use serde::Deserialize;
use serde_json::{Value, json};

#[derive(Deserialize)]
#[serde(tag = "method", rename_all = "snake_case")]
enum Request {
    /// Use the open link if the Ledger still answers on it; otherwise find a Ledger nearby,
    /// connect and check its Anzen app is open → `{"name": …}`.
    Connect,
    /// Send one protocol message → the signer's answer, or `{"declined": true}` if the person
    /// rejected it on the Ledger.
    Request { message: Value },
    /// Close the link, for when the app is done with the Ledger (not after each request).
    Disconnect,
}

#[cfg(not(target_os = "android"))]
mod link {
    use super::*;
    use anzen_link::ledger::{self, HardwareSigner, Ledger, Message, Rejected};
    use std::{
        sync::{Mutex, OnceLock},
        time::Duration,
    };

    /// How long to look for a Ledger before saying none was found.
    const SEARCH: Duration = Duration::from_secs(25);

    static RUNTIME: OnceLock<tokio::runtime::Runtime> = OnceLock::new();
    static SIGNER: Mutex<Option<(String, HardwareSigner<Ledger>)>> = Mutex::new(None);

    fn runtime() -> Result<&'static tokio::runtime::Runtime> {
        if RUNTIME.get().is_none() {
            let runtime = tokio::runtime::Builder::new_multi_thread()
                .worker_threads(1)
                .enable_all()
                .build()
                .context("could not start the Bluetooth runtime")?;
            let _ = RUNTIME.set(runtime);
        }
        Ok(RUNTIME.get().expect("set above"))
    }

    pub fn handle(request: Request) -> Result<Value> {
        let runtime = runtime()?;
        let mut signer = SIGNER
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        match request {
            Request::Connect => runtime.block_on(async {
                // An open link is reused if the Anzen app still answers on it.
                if let Some((name, previous)) = signer.take() {
                    let mut link = previous.into_link();
                    if link.is_connected().await && HardwareSigner::check(&mut link).await.is_ok() {
                        *signer = Some((name.clone(), HardwareSigner::new(link)));
                        return Ok(json!({ "name": name }));
                    }
                    link.disconnect().await.ok();
                }
                let found = ledger::find(None, SEARCH).await?;
                let name = found.name.clone();
                let mut link = found.connect().await?;
                // A link to the dashboard or another app is closed, so the next try finds the
                // Ledger advertising again.
                if let Err(error) = HardwareSigner::check(&mut link).await {
                    link.disconnect().await.ok();
                    return Err(error);
                }
                *signer = Some((name.clone(), HardwareSigner::new(link)));
                Ok(json!({ "name": name }))
            }),
            Request::Request { message } => {
                let message: Message =
                    serde_json::from_value(message).context("invalid protocol message")?;
                let (_, signer) = signer.as_mut().context("no Ledger is connected")?;
                match runtime.block_on(signer.answer(&message)) {
                    Ok(answer) => Ok(serde_json::to_value(answer)?),
                    Err(error) if error.is::<Rejected>() => Ok(json!({ "declined": true })),
                    Err(error) => Err(error),
                }
            }
            Request::Disconnect => {
                if let Some((_, signer)) = signer.take() {
                    runtime.block_on(signer.into_link().disconnect()).ok();
                }
                Ok(Value::Null)
            }
        }
    }
}

#[cfg(target_os = "android")]
mod link {
    use super::*;

    pub fn handle(_request: Request) -> Result<Value> {
        anyhow::bail!("Ledger signers are not supported on Android yet")
    }
}

pub fn ledger(request: &str) -> Result<Value> {
    link::handle(serde_json::from_str(request).context("invalid Ledger request")?)
}
