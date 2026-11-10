//! The phone's link to a Trezor signer (a proof of concept, like the Trezor app).
//!
//! The app's signer flows speak protocol messages to a link; here the link is Trezor's encrypted
//! channel and the signer is the Anzen app on a Trezor. Rust owns the connection: the app asks to
//! connect, pairs if the Trezor does not know this phone, sends one message at a time and waits
//! for its answer while the person reviews it on the Trezor's screen. Calls block, so the native
//! layer runs them off the main thread.
//!
//! A Trezor keeps no apps and the Anzen app stores nothing, so this side carries three things:
//! the app's image, uploaded at each connection the Trezor no longer holds it for (it is built
//! into this library, see `build.rs`); the phone's identity to Trezors it has paired with; and
//! what the signer sealed for it to keep. The last two live in a directory the native layer
//! names.
//!
//! The link stays open between requests and is reused by the next one.

use anyhow::{Context, Result};
use serde::Deserialize;
use serde_json::{Value, json};

#[derive(Deserialize)]
#[serde(tag = "method", rename_all = "snake_case")]
enum Request {
    /// Use the open link if the Trezor still answers on it; otherwise find a Trezor nearby and
    /// open a channel → `{"name": …}` once its Anzen app is loaded, or
    /// `{"name": …, "pairing": true}` if the Trezor is now showing a code to pair with.
    /// `emulator` names the port of a Trezor emulator on this machine instead, for development.
    Connect { emulator: Option<u16> },
    /// Finish pairing with the code the Trezor shows → `{"name": …}`.
    Pair { code: String },
    /// Send one protocol message → the signer's answer, or `{"declined": true}` if the person
    /// rejected it on the Trezor.
    Request { message: Value },
    /// Close the link, for when the app is done with the Trezor (not after each request).
    Disconnect,
}

#[cfg(not(target_os = "android"))]
mod link {
    use super::*;
    use anyhow::bail;
    use anzen_link::trezor::{
        self, AppImage, Bluetooth, CodeEntry, Directory, Emulator, Host, Identity, Message,
        Rejected, Transport, Trezor, TrezorSigner,
    };
    use std::{
        path::{Path, PathBuf},
        sync::{Mutex, OnceLock},
        time::Duration,
    };

    /// How long to look for a Trezor before saying none was found.
    const SEARCH: Duration = Duration::from_secs(25);
    const IDENTITY: &str = "trezor-identity.json";

    /// The Safe 7's image of the Anzen app.
    #[cfg(trezor_device_image)]
    const DEVICE_IMAGE: Option<[&str; 3]> = Some([
        include_str!(env!("ANZEN_TREZOR_DEVICE_TAPP")),
        include_str!(env!("ANZEN_TREZOR_DEVICE_PROOF")),
        include_str!(env!("ANZEN_TREZOR_DEVICE_ROOT_PACKET")),
    ]);
    #[cfg(not(trezor_device_image))]
    const DEVICE_IMAGE: Option<[&str; 3]> = None;

    /// The emulator's image, where an emulator can be reached at all.
    #[cfg(trezor_emulator_image)]
    const EMULATOR_IMAGE: Option<[&str; 3]> = Some([
        include_str!(env!("ANZEN_TREZOR_EMULATOR_TAPP")),
        include_str!(env!("ANZEN_TREZOR_EMULATOR_PROOF")),
        include_str!(env!("ANZEN_TREZOR_EMULATOR_ROOT_PACKET")),
    ]);
    #[cfg(not(trezor_emulator_image))]
    const EMULATOR_IMAGE: Option<[&str; 3]> = None;

    /// Bluetooth to a device, or the emulator's socket.
    enum Wire {
        Bluetooth(Bluetooth),
        Emulator(Emulator),
    }

    impl Transport for Wire {
        fn packet_len(&self) -> usize {
            match self {
                Self::Bluetooth(link) => link.packet_len(),
                Self::Emulator(link) => link.packet_len(),
            }
        }

        async fn send(&mut self, packet: &[u8]) -> Result<()> {
            match self {
                Self::Bluetooth(link) => link.send(packet).await,
                Self::Emulator(link) => link.send(packet).await,
            }
        }

        async fn receive(&mut self, timeout: Duration) -> Result<Option<Vec<u8>>> {
            match self {
                Self::Bluetooth(link) => link.receive(timeout).await,
                Self::Emulator(link) => link.receive(timeout).await,
            }
        }
    }

    impl Wire {
        async fn close(self) {
            if let Self::Bluetooth(link) = self {
                link.disconnect().await.ok();
            }
        }

        fn image(&self) -> Result<AppImage> {
            let [tapp, proof, root_packet] = match self {
                Self::Bluetooth(_) => DEVICE_IMAGE,
                Self::Emulator(_) => EMULATOR_IMAGE,
            }
            .context(
                "This build of Anzen has no Anzen app for a Trezor in it. Build trezor-app first.",
            )?;
            AppImage::from_parts(tapp, proof, root_packet)
        }
    }

    /// What is open now.
    enum Open {
        /// The Trezor is showing a pairing code.
        Pairing {
            name: String,
            trezor: Box<Trezor<Wire>>,
            entry: CodeEntry,
        },
        Ready {
            name: String,
            signer: Box<TrezorSigner<Wire, Directory>>,
        },
    }

    static RUNTIME: OnceLock<tokio::runtime::Runtime> = OnceLock::new();
    static OPEN: Mutex<Option<Open>> = Mutex::new(None);

    fn runtime() -> Result<&'static tokio::runtime::Runtime> {
        if RUNTIME.get().is_none() {
            let runtime = tokio::runtime::Builder::new_multi_thread()
                .worker_threads(1)
                .enable_all()
                .build()
                .context("could not start the Trezor link's runtime")?;
            let _ = RUNTIME.set(runtime);
        }
        Ok(RUNTIME.get().expect("set above"))
    }

    fn identity(root: &Path) -> Result<Identity> {
        match std::fs::read_to_string(root.join(IDENTITY)) {
            Ok(stored) => serde_json::from_str(&stored)
                .context("this phone's stored Trezor pairing is unreadable"),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Identity::generate()),
            Err(error) => Err(error).context("could not read this phone's Trezor pairing"),
        }
    }

    fn store_identity(root: &Path, identity: &Identity) -> Result<()> {
        std::fs::create_dir_all(root)?;
        let scratch = root.join(format!("{IDENTITY}.new"));
        std::fs::write(&scratch, serde_json::to_vec(identity)?)?;
        std::fs::rename(&scratch, root.join(IDENTITY))
            .context("could not store this phone's Trezor pairing")
    }

    /// Open the wallet, load the Anzen app and hand back the signer.
    async fn ready(mut trezor: Trezor<Wire>, root: &Path) -> Result<TrezorSigner<Wire, Directory>> {
        let image = trezor.transport().image()?;
        trezor.open_wallet().await?;
        TrezorSigner::open(trezor, &image, Directory(root.to_path_buf())).await
    }

    /// An iOS simulator has no Bluetooth. There the Trezor is the emulator on the same Mac, on
    /// its default port or the one `ANZEN_TREZOR_PORT` names (set it for the simulator with
    /// `SIMCTL_CHILD_ANZEN_TREZOR_PORT`).
    fn simulators_emulator() -> Option<u16> {
        if cfg!(all(target_os = "ios", target_abi = "sim")) {
            Some(
                std::env::var("ANZEN_TREZOR_PORT")
                    .ok()
                    .and_then(|port| port.parse().ok())
                    .unwrap_or(trezor::DEFAULT_PORT),
            )
        } else {
            None
        }
    }

    async fn connect(root: &Path, emulator: Option<u16>) -> Result<Open> {
        let (name, wire) = match emulator.or_else(simulators_emulator) {
            Some(port) => (
                "Trezor emulator".to_owned(),
                Wire::Emulator(Emulator::connect(port).await?),
            ),
            None => {
                let link = trezor::open(None, SEARCH).await?;
                (link.name.clone(), Wire::Bluetooth(link))
            }
        };
        let mut identity = identity(root)?;
        let host = Host::default();
        let mut trezor = Trezor::connect(wire, &identity).await?;
        if trezor.is_paired() {
            if trezor.finish(&mut identity).await? {
                store_identity(root, &identity)?;
            }
        } else if trezor.can_skip_pairing() {
            // Debug firmware (the emulator) pairs without a code, and remembers nothing.
            trezor.skip_pairing(&host).await?;
        } else {
            let entry = trezor.begin_pairing(&host).await?;
            return Ok(Open::Pairing {
                name,
                trezor: Box::new(trezor),
                entry,
            });
        }
        Ok(Open::Ready {
            name,
            signer: Box::new(ready(trezor, root).await?),
        })
    }

    async fn close(open: Open) {
        match open {
            Open::Pairing { trezor, .. } => trezor.into_transport().close().await,
            Open::Ready { signer, .. } => signer.into_trezor().into_transport().close().await,
        }
    }

    fn connected(open: &Open) -> Value {
        match open {
            Open::Pairing { name, .. } => json!({ "name": name, "pairing": true }),
            Open::Ready { name, .. } => json!({ "name": name }),
        }
    }

    pub fn handle(root: PathBuf, request: Request) -> Result<Value> {
        let runtime = runtime()?;
        let mut open = OPEN.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        match request {
            Request::Connect { emulator } => runtime.block_on(async {
                // An open link is reused if the Anzen app still answers on it.
                let reused = match open.take() {
                    Some(Open::Ready { name, mut signer }) => {
                        if signer.check().await.is_ok() {
                            Some(Open::Ready { name, signer })
                        } else {
                            close(Open::Ready { name, signer }).await;
                            None
                        }
                    }
                    Some(pairing) => {
                        close(pairing).await;
                        None
                    }
                    None => None,
                };
                *open = Some(match reused {
                    Some(reused) => reused,
                    None => connect(&root, emulator).await?,
                });
                Ok(connected(open.as_ref().expect("set above")))
            }),
            Request::Pair { code } => runtime.block_on(async {
                let Some(Open::Pairing {
                    name,
                    mut trezor,
                    entry,
                }) = open.take()
                else {
                    bail!("No Trezor is waiting for a pairing code.");
                };
                let mut identity = identity(&root)?;
                if let Err(error) = trezor.finish_pairing(entry, &code, &mut identity).await {
                    // A failed pairing ends the channel; the next connection starts another.
                    trezor.into_transport().close().await;
                    return Err(error);
                }
                store_identity(&root, &identity)?;
                *open = Some(Open::Ready {
                    name,
                    signer: Box::new(ready(*trezor, &root).await?),
                });
                Ok(connected(open.as_ref().expect("set above")))
            }),
            Request::Request { message } => {
                let message: Message =
                    serde_json::from_value(message).context("invalid protocol message")?;
                let Some(Open::Ready { signer, .. }) = open.as_mut() else {
                    bail!("No Trezor is connected.");
                };
                match runtime.block_on(signer.answer(&message)) {
                    Ok(answer) => Ok(serde_json::to_value(answer)?),
                    Err(error) if error.is::<Rejected>() => Ok(json!({ "declined": true })),
                    Err(error) => Err(error),
                }
            }
            Request::Disconnect => {
                if let Some(open) = open.take() {
                    runtime.block_on(close(open));
                }
                Ok(Value::Null)
            }
        }
    }
}

#[cfg(target_os = "android")]
mod link {
    use super::*;

    pub fn handle(_root: std::path::PathBuf, _request: Request) -> Result<Value> {
        anyhow::bail!("Trezor signers are not supported on Android yet")
    }
}

/// `root` is the directory this phone keeps its Trezor pairing and the signer's sealed state in.
pub fn trezor(root: &str, request: &str) -> Result<Value> {
    link::handle(
        std::path::PathBuf::from(root).join("trezor"),
        serde_json::from_str(request).context("invalid Trezor request")?,
    )
}
