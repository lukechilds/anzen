//! Try a Trezor as an Anzen signer from this computer, with no phone: find it over Bluetooth (or
//! reach the emulator), pair, load the Anzen app, and optionally run a whole ceremony on test
//! keys, checking every signature it returns. Nothing here touches funds.
//!
//! ```sh
//! cargo run -p anzen-link --example trezor                      # connect, show the signer's key
//! cargo run -p anzen-link --example trezor -- policy            # set a vault up, approve a policy
//! cargo run -p anzen-link --example trezor -- --emulator 31324 policy
//! ```
//!
//! The first run against a device pairs: the Trezor shows a code, typed here. The pairing and
//! what the signer seals are kept in `.anzen-data/trezor-example`, so later runs connect directly.
//! The app image is the one `trezor-app/tools/build.sh` leaves, or the `.tapp` that `--app` names.

#[path = "../tests/phone_side/mod.rs"]
#[allow(dead_code)]
mod phone_side;

use anyhow::{Context, Result, bail};
use anzen_link::trezor::{
    self, AppImage, Bluetooth, Directory, Emulator, Host, Identity, Rejected, Transport, Trezor,
    TrezorSigner,
};
use anzen_protocol::{
    address::Network,
    policy::Limits,
    script::{VaultKeys, XOnlyKey},
};
use anzen_signer::hardware::Pair;
use bitcoin::key::{Keypair, Secp256k1};
use std::{
    io::Write,
    path::{Path, PathBuf},
    time::Duration,
};

const NETWORK: Network = Network::Regtest;

/// The newest build of the app for `architecture` under `trezor-app/target`.
fn built_image(architecture: &str) -> Result<PathBuf> {
    let builds = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../trezor-app/target/artifacts/serialized/anzen.app");
    std::fs::read_dir(&builds)
        .with_context(|| {
            format!(
                "build the Trezor app first: {} is missing",
                builds.display()
            )
        })?
        .filter_map(|version| std::fs::read_dir(version.ok()?.path()).ok())
        .flatten()
        .filter_map(|file| Some(file.ok()?.path()))
        .filter(|path| {
            path.extension()
                .is_some_and(|extension| extension == "tapp")
                && path.to_string_lossy().contains(architecture)
        })
        .max()
        .with_context(|| {
            format!("no {architecture} build of the Trezor app: run trezor-app/tools/build.sh")
        })
}

fn identity(state: &Path) -> Result<Identity> {
    match std::fs::read_to_string(state.join("identity.json")) {
        Ok(stored) => Ok(serde_json::from_str(&stored)?),
        Err(_) => Ok(Identity::generate()),
    }
}

fn store(state: &Path, identity: &Identity) -> Result<()> {
    std::fs::create_dir_all(state)?;
    Ok(std::fs::write(
        state.join("identity.json"),
        serde_json::to_vec(identity)?,
    )?)
}

/// Open the channel, pairing if the Trezor does not know this computer.
async fn open<T: Transport>(transport: T, state: &Path) -> Result<Trezor<T>> {
    let mut identity = identity(state)?;
    let host = Host {
        host_name: "This computer".to_owned(),
        app_name: "Anzen".to_owned(),
    };
    eprintln!("Opening a channel. If your Trezor is locked, unlock it now.");
    let mut trezor = Trezor::connect(transport, &identity).await?;
    eprintln!("Connected to a {}.", trezor.model());
    if trezor.is_paired() {
        eprintln!("Already paired. Confirm on your Trezor if it asks.");
        if trezor.finish(&mut identity).await? {
            store(state, &identity)?;
        }
    } else if trezor.can_skip_pairing() {
        eprintln!("Debug firmware: pairing without a code. Confirm on the Trezor.");
        trezor.skip_pairing(&host).await?;
    } else {
        eprintln!("Pairing. Confirm on your Trezor, then type the code it shows.");
        let entry = trezor.begin_pairing(&host).await?;
        eprint!("Code: ");
        std::io::stderr().flush()?;
        let mut code = String::new();
        std::io::stdin().read_line(&mut code)?;
        trezor.finish_pairing(entry, &code, &mut identity).await?;
        store(state, &identity)?;
        eprintln!("Paired.");
    }
    trezor.open_wallet().await?;
    Ok(trezor)
}

async fn run<T: Transport>(transport: T, state: &Path, image: &Path, ceremony: bool) -> Result<()> {
    let trezor = open(transport, state).await?;
    eprintln!("Loading the Anzen app ({}).", image.display());
    let image = AppImage::from_build(image)?;
    let mut signer = TrezorSigner::open(trezor, &image, Directory(state.to_path_buf())).await?;
    let key = signer.identity(NETWORK).await?;
    println!("Signer's vault key on regtest: {}", key.to_hex());
    if !ceremony {
        return Ok(());
    }

    // A fixed test phone key: this vault is never funded.
    let phone = Keypair::from_seckey_slice(&Secp256k1::new(), &[0x42; 32])?;
    let keys = VaultKeys {
        phone: XOnlyKey(phone.x_only_public_key().0.serialize()),
        hww: key,
        recovery_locktime: 1_234_567_890,
    };
    eprintln!("Setting a test vault up. Confirm its address on your Trezor if it asks.");
    signer
        .pair(&Pair {
            network: NETWORK,
            phone: keys.phone,
            recovery_locktime: keys.recovery_locktime,
        })
        .await?;
    // A policy that follows from nothing: forget the last run's, so this is the first again.
    if signer
        .state(NETWORK)?
        .is_some_and(|state| state.anchor.is_some())
    {
        eprintln!("Forgetting the last run's policy. Confirm on your Trezor.");
        signer.forget_policy(NETWORK).await?;
    }
    let inputs = std::env::var("ANZEN_TEST_INPUTS")
        .ok()
        .and_then(|count| count.parse().ok())
        .unwrap_or(2_u8);
    let vault = (1..=inputs)
        .map(|byte| phone_side::input(byte, 110_000_000))
        .collect();
    let (request, secrets, messages) = phone_side::policy(
        &phone,
        keys,
        NETWORK,
        vault,
        Limits {
            monthly_limit_sats: 10_000_000,
            emergency_access_limit_sats: 50_000_000,
        },
    );
    eprintln!(
        "Asking for {} signatures over a {inputs}-deposit policy. Review and approve on your Trezor.",
        messages.len()
    );
    let started = std::time::Instant::now();
    let halves = match signer.approve_policy(&request).await {
        Ok(halves) => halves,
        Err(error) if error.is::<Rejected>() => bail!("Rejected on the Trezor."),
        Err(error) => return Err(error),
    };
    let took = started.elapsed();
    // Complete each joint signature with the phone's half and verify it as Bitcoin would.
    phone_side::complete_all(
        &phone,
        &keys,
        &messages,
        &secrets,
        &request.phone_nonces,
        &halves,
    );
    println!(
        "{} signatures, all valid, in {:.1}s including the review.",
        halves.len(),
        took.as_secs_f32()
    );
    Ok(())
}

#[tokio::main(flavor = "multi_thread", worker_threads = 2)]
async fn main() -> Result<()> {
    let mut arguments: Vec<String> = std::env::args().skip(1).collect();
    let mut option = |name: &str| -> Result<Option<String>> {
        let Some(at) = arguments.iter().position(|argument| argument == name) else {
            return Ok(None);
        };
        let value = arguments
            .get(at + 1)
            .cloned()
            .with_context(|| format!("{name} takes a value"))?;
        arguments.drain(at..at + 2);
        Ok(Some(value))
    };
    let emulator = option("--emulator")?
        .map(|port| port.parse::<u16>().context("--emulator takes a port"))
        .transpose()?;
    let app = option("--app")?.map(PathBuf::from);
    let state = option("--state")?.map_or_else(
        || PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../.anzen-data/trezor-example"),
        PathBuf::from,
    );
    let ceremony = match arguments.as_slice() {
        [] => false,
        [command] if command == "policy" => true,
        _ => bail!("usage: trezor [--emulator PORT] [--app FILE.tapp] [--state DIR] [policy]"),
    };

    match emulator {
        Some(port) => {
            let host = if cfg!(target_os = "macos") {
                "macos-aarch64"
            } else {
                "linux-x86_64"
            };
            let image = app.map_or_else(|| built_image(host), Ok)?;
            // A debug Trezor remembers no pairing, so the emulator gets a state of its own.
            let state = state.join("emulator");
            run(Emulator::connect(port).await?, &state, &image, ceremony).await
        }
        None => {
            let image = app.map_or_else(|| built_image("armv8m"), Ok)?;
            eprintln!("Looking for a Trezor over Bluetooth…");
            let found = trezor::find(None, Duration::from_secs(25)).await?;
            eprintln!("Found {}. Connecting…", found.name);
            let link: Bluetooth = found.connect().await?;
            run(link, &state, &image, ceremony).await
        }
    }
}
