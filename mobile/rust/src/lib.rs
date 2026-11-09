//! Native transport only: role routing, OS-key scope, backend selection and C ABI ownership.
//! Wallet behavior is implemented by the role facades, never by this adapter or JavaScript.

use anyhow::{Context, Result, bail};
use anzen_signer_app::{
    api as cold,
    application::{SignerApplication, SignerRequest},
};
use anzen_wallet::{
    HotWalletBackend, api as hot,
    application::{WalletApplication, WalletRequest},
    watch::{ChainWatcher, ElectrumWatcher, native::Watch},
};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{
    ffi::{CStr, CString, c_char},
    path::{Path, PathBuf},
    sync::Mutex,
};
use zeroize::Zeroizing;

static GATE: Mutex<()> = Mutex::new(());
const MAX_REQUEST_BYTES: usize = 8 * 1024 * 1024;

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum Backend {
    Electrum {
        #[serde(default)]
        servers: Vec<String>,
    },
    Rpc {
        url: String,
        user: String,
        password: String,
    },
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeSettings {
    #[serde(default)]
    pub dangerously_enable_mainnet: bool,
    pub backend: Backend,
    /// The network the app is set to. Each network keeps its own devices; without one, a role's
    /// device is the one directly under the root, as before networks were a setting.
    #[serde(default)]
    pub network: Option<hot::Network>,
}

/// Where a role's device lives: `root/<network>/<role>`. A phone set up before networks were a
/// setting kept one device per role at `root/<role>`; that one stays where it is and serves the
/// network it was made for, so switching networks never strands it.
fn device_directory(
    root: &Path,
    role: &str,
    network: Option<hot::Network>,
    legacy_network: impl FnOnce(&Path) -> Result<hot::Network>,
) -> PathBuf {
    let legacy = root.join(role);
    let Some(network) = network else {
        return legacy;
    };
    let scoped = root.join(network.to_string()).join(role);
    if !scoped.exists() && legacy.exists() && legacy_network(&legacy).ok() == Some(network) {
        return legacy;
    }
    scoped
}

/// Electrum over WebSocket (the same shared client as the browser), or Bitcoin Core on regtest.
fn connect(settings: &RuntimeSettings, network: hot::Network) -> Result<Box<dyn HotWalletBackend>> {
    Ok(match &settings.backend {
        Backend::Electrum { servers } if servers.is_empty() => {
            Box::new(hot::ElectrumBackend::new(network, hot::WebSocketConnector)?)
        }
        Backend::Electrum { servers } => Box::new(hot::ElectrumBackend::with_servers(
            network,
            servers.clone(),
            hot::WebSocketConnector,
        )?),
        Backend::Rpc {
            url,
            user,
            password,
        } => Box::new(hot::BitcoinCoreBackend::connect(
            &hot::RpcConfig {
                url: url.clone(),
                user: user.clone(),
                password: password.clone(),
            },
            network,
        )?),
    })
}

/// Native code supplies the private root/key/settings. The UI supplies only a role command.
/// The signer arm has no backend construction or hot-wallet calls.
pub fn execute(
    root: &Path,
    role: &str,
    key: [u8; 32],
    settings: &RuntimeSettings,
    command: &str,
) -> Result<Value> {
    if command.len() > MAX_REQUEST_BYTES {
        bail!("device request exceeds 8 MiB");
    }
    let _gate = GATE
        .lock()
        .map_err(|_| anyhow::anyhow!("native runtime interrupted; restart the app"))?;
    match role {
        "wallet" => {
            let directory = device_directory(root, role, settings.network, |legacy| {
                hot::with_protected_storage(legacy, key, || {
                    WalletApplication::new(legacy.to_path_buf(), true)
                        .public_identity()?
                        .bitcoin_network()
                })
            });
            hot::with_protected_storage(&directory, key, || {
                let app =
                    WalletApplication::new(directory.clone(), settings.dangerously_enable_mainnet);
                let request: WalletRequest = serde_json::from_str(command)?;
                let backend = if request.requires_chain() {
                    Some(connect(
                        settings,
                        app.public_identity()?.bitcoin_network()?,
                    )?)
                } else {
                    None
                };
                app.execute(request, backend.as_deref())
            })
        }
        "signer" => {
            let directory = device_directory(root, role, settings.network, |legacy| {
                cold::with_protected_storage(legacy, key, || {
                    SignerApplication::new(legacy.to_path_buf(), true)
                        .public_identity()?
                        .bitcoin_network()
                })
            });
            cold::with_protected_storage(&directory, key, || {
                let app =
                    SignerApplication::new(directory.clone(), settings.dangerously_enable_mainnet);
                let request: SignerRequest = serde_json::from_str(command)?;
                app.execute(request)
            })
        }
        _ => bail!("invalid native device role"),
    }
}

static WATCH: Mutex<Option<Watch>> = Mutex::new(None);

#[derive(Deserialize)]
#[serde(tag = "method", rename_all = "snake_case", deny_unknown_fields)]
enum WatchRequest {
    Start { network: hot::Network },
    Targets { scripthashes: Vec<String> },
    Changes,
    Stop,
}

/// Live updates. The shared listener runs on its own thread with its own connection, never
/// behind the command gate, so collecting changes cannot wait for a running command.
pub fn watch(settings: &RuntimeSettings, request: &str) -> Result<Value> {
    let mut current = WATCH
        .lock()
        .map_err(|_| anyhow::anyhow!("native watcher interrupted; restart the app"))?;
    Ok(match serde_json::from_str(request)? {
        WatchRequest::Start { network } => {
            if network == hot::Network::Bitcoin && !settings.dangerously_enable_mainnet {
                bail!("mainnet requires explicit dangerously-enable-mainnet opt-in");
            }
            let (watcher, rpc) = match &settings.backend {
                Backend::Electrum { servers } if servers.is_empty() => {
                    (ChainWatcher::new(network)?, None)
                }
                Backend::Electrum { servers } => (
                    ChainWatcher::Electrum(ElectrumWatcher::with_servers(servers.clone())),
                    None,
                ),
                Backend::Rpc {
                    url,
                    user,
                    password,
                } => {
                    if network != hot::Network::Regtest {
                        bail!("Bitcoin Core live updates are for regtest");
                    }
                    let rpc = hot::RpcConfig {
                        url: url.clone(),
                        user: user.clone(),
                        password: password.clone(),
                    };
                    (ChainWatcher::new(network)?, Some(rpc))
                }
            };
            *current = Some(Watch::spawn(watcher, rpc)?);
            json!({ "started": true })
        }
        WatchRequest::Targets { scripthashes } => {
            if let Some(watch) = current.as_ref() {
                watch.set_targets(scripthashes);
            }
            json!({ "watching": current.is_some() })
        }
        WatchRequest::Changes => {
            json!({ "changes": current.as_ref().map(Watch::changes).unwrap_or_default() })
        }
        WatchRequest::Stop => {
            *current = None;
            json!({ "stopped": true })
        }
    })
}

/// Live-update requests (`start`, `targets`, `changes`, `stop`); returns a JSON envelope the
/// caller owns until `anzen_mobile_free`.
///
/// # Safety
/// Both pointers must reference live NUL-terminated UTF-8 strings for this call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn anzen_mobile_watch(
    settings: *const c_char,
    request: *const c_char,
) -> *mut c_char {
    let outcome = std::panic::catch_unwind(|| -> Result<Value> {
        if settings.is_null() || request.is_null() {
            bail!("null native argument");
        }
        // SAFETY: checked for null; remaining validity/lifetime is the documented C ABI contract.
        let (settings, request) = unsafe {
            (
                CStr::from_ptr(settings).to_str()?,
                CStr::from_ptr(request).to_str()?,
            )
        };
        watch(
            &serde_json::from_str(settings).context("invalid native connection settings")?,
            request,
        )
    });
    envelope(outcome)
}

#[derive(Deserialize, Clone, Copy)]
#[serde(rename_all = "snake_case")]
enum BenchmarkEngine {
    Gpu,
    Cpu,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct BenchmarkRequest {
    engine: BenchmarkEngine,
    seconds: f64,
}

/// Benchmarks hold the GPU or every CPU core flat out, so only one runs at a time.
static BENCHMARK: Mutex<()> = Mutex::new(());

/// The vanity search's throughput on fixed public inputs, on the GPU through wgpu or on every CPU
/// thread with the library code. It touches no keys or wallet state, so it runs outside the
/// command gate.
pub fn benchmark(request: &str) -> Result<Value> {
    let request: BenchmarkRequest = serde_json::from_str(request)?;
    let seconds = request.seconds.clamp(1.0, 300.0);
    let _running = BENCHMARK
        .try_lock()
        .map_err(|_| anyhow::anyhow!("another benchmark is running"))?;
    Ok(match request.engine {
        BenchmarkEngine::Gpu => {
            let report = anzen_vanity::bench::run_wgpu_blocking(seconds)?;
            let measurement = report.measurement;
            json!({
                "engine": "gpu",
                "device": report.adapter,
                "setup_seconds": report.setup_seconds,
                "batch": measurement.batch,
                "attempts": measurement.attempts,
                "seconds": measurement.seconds,
                "rate": measurement.rate,
            })
        }
        BenchmarkEngine::Cpu => {
            let epoch = std::time::Instant::now();
            let threads = anzen_wallet::available_worker_count();
            let result = anzen_wallet::cpu_vanity_benchmark(seconds, threads, || {
                epoch.elapsed().as_secs_f64()
            })?;
            json!({
                "engine": "cpu",
                "device": format!("{threads} CPU threads"),
                "attempts": result.attempts,
                "seconds": result.seconds,
                "rate": result.rate,
            })
        }
    })
}

/// Run a vanity-search benchmark (`{"engine": "gpu"|"cpu", "seconds": n}`); returns a JSON
/// envelope the caller owns until `anzen_mobile_free`. It blocks for the whole benchmark, so call
/// it off the main thread.
///
/// # Safety
/// `request` must reference a live NUL-terminated UTF-8 string for this call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn anzen_mobile_benchmark(request: *const c_char) -> *mut c_char {
    let outcome = std::panic::catch_unwind(|| -> Result<Value> {
        if request.is_null() {
            bail!("null native argument");
        }
        // SAFETY: checked for null; remaining validity/lifetime is the documented C ABI contract.
        benchmark(unsafe { CStr::from_ptr(request).to_str()? })
    });
    envelope(outcome)
}

fn envelope(outcome: std::thread::Result<Result<Value>>) -> *mut c_char {
    let response = match outcome {
        Ok(Ok(value)) => json!({ "ok": value }),
        Ok(Err(error)) => json!({ "error": format!("{error:#}") }),
        Err(_) => json!({ "error": "native operation interrupted; restart before retrying" }),
    };
    // JSON escapes embedded NULs; serialization of Value cannot fail.
    CString::new(response.to_string())
        .expect("JSON is NUL-free")
        .into_raw()
}

/// Return a UTF-8 JSON envelope; the caller owns it until `anzen_mobile_free`.
///
/// # Safety
/// Each string pointer must reference a live NUL-terminated UTF-8 string for this call.
/// `key` must reference exactly 32 readable bytes. The adapter must not expose it to JS.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn anzen_mobile_execute(
    root: *const c_char,
    role: *const c_char,
    key: *const u8,
    settings: *const c_char,
    command: *const c_char,
) -> *mut c_char {
    let outcome = std::panic::catch_unwind(|| -> Result<Value> {
        if [root, role, settings, command].iter().any(|p| p.is_null()) || key.is_null() {
            bail!("null native argument");
        }
        // SAFETY: checked for null; remaining validity/lifetime is the documented C ABI contract.
        let (root, role, settings, command, key) = unsafe {
            (
                CStr::from_ptr(root).to_str()?,
                CStr::from_ptr(role).to_str()?,
                CStr::from_ptr(settings).to_str()?,
                CStr::from_ptr(command).to_str()?,
                Zeroizing::new(
                    std::slice::from_raw_parts(key, 32)
                        .try_into()
                        .expect("fixed key length"),
                ),
            )
        };
        execute(
            Path::new(root),
            role,
            *key,
            &serde_json::from_str(settings).context("invalid native connection settings")?,
            command,
        )
    });
    envelope(outcome)
}

/// # Safety
/// `value` must be null or a still-owned pointer returned by `anzen_mobile_execute`.
/// It must be freed exactly once and never used afterward.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn anzen_mobile_free(value: *mut c_char) {
    if !value.is_null() {
        // SAFETY: ownership is transferred back by the documented ABI contract.
        drop(unsafe { CString::from_raw(value) });
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn benchmarks_run_on_the_cpu_and_reject_unknown_engines() {
        let result = super::benchmark(r#"{"engine":"cpu","seconds":1}"#).unwrap();
        assert_eq!(result["engine"], "cpu");
        assert!(result["rate"].as_f64().unwrap() > 0.0);
        assert!(result["device"].as_str().unwrap().ends_with("CPU threads"));
        assert!(super::benchmark(r#"{"engine":"metal","seconds":1}"#).is_err());
    }

    use super::*;

    fn settings() -> RuntimeSettings {
        RuntimeSettings {
            dangerously_enable_mainnet: false,
            backend: Backend::Electrum { servers: vec![] },
            network: None,
        }
    }

    #[test]
    fn each_network_keeps_its_own_devices_and_an_earlier_device_keeps_its_network() {
        let root = tempfile::tempdir().unwrap();
        let on = |network: &str| RuntimeSettings {
            network: Some(serde_json::from_value(json!(network)).unwrap()),
            ..settings()
        };
        let identity = |settings: &RuntimeSettings| {
            execute(
                root.path(),
                "signer",
                [4; 32],
                settings,
                r#"{"method":"identity"}"#,
            )
        };
        // A signer from before networks were a setting, directly under the root.
        let earlier = execute(
            root.path(),
            "signer",
            [4; 32],
            &settings(),
            r#"{"method":"initialize","network":"regtest"}"#,
        )
        .unwrap();
        // Regtest still finds it where it is; Mutinynet starts empty and makes its own.
        assert_eq!(
            identity(&on("regtest")).unwrap()["vault_pubkey"],
            earlier["vault_pubkey"]
        );
        assert!(identity(&on("signet")).is_err());
        let signet = execute(
            root.path(),
            "signer",
            [4; 32],
            &on("signet"),
            r#"{"method":"initialize","network":"signet"}"#,
        )
        .unwrap();
        assert_ne!(signet["vault_pubkey"], earlier["vault_pubkey"]);
        assert!(root.path().join("signet/signer").exists());
        assert_eq!(
            identity(&on("regtest")).unwrap()["vault_pubkey"],
            earlier["vault_pubkey"]
        );
        assert_eq!(
            identity(&on("signet")).unwrap()["vault_pubkey"],
            signet["vault_pubkey"]
        );
    }

    #[test]
    fn native_outbox_is_encrypted_offline_and_readable_after_restart() {
        let directory = tempfile::tempdir().unwrap();
        let identity = execute(
            directory.path(),
            "signer",
            [4; 32],
            &settings(),
            r#"{"method":"initialize","network":"regtest"}"#,
        )
        .unwrap();
        let list = execute(
            directory.path(),
            "signer",
            [4; 32],
            &settings(),
            r#"{"method":"outbox"}"#,
        )
        .unwrap();
        let items = list["messages"].as_array().unwrap();
        assert_eq!(items.len(), 1);
        let id = items[0]["id"].as_str().unwrap();
        let file = directory
            .path()
            .join("signer/hww/outbox")
            .join(format!("{id}.json"));
        let bytes = std::fs::read_to_string(file).unwrap();
        assert!(bytes.contains("anzen_native_storage"));
        assert!(!bytes.contains(identity["vault_pubkey"].as_str().unwrap()));
        assert!(!bytes.contains("signer-identity"));
        let command = serde_json::json!({"method":"read_outbox", "id":id}).to_string();
        let message = execute(directory.path(), "signer", [4; 32], &settings(), &command).unwrap();
        assert_eq!(message["kind"], "signer-identity");
        assert_eq!(message["body"], identity);
        // Re-exporting exactly the same public identity does not duplicate the record.
        execute(
            directory.path(),
            "signer",
            [4; 32],
            &settings(),
            r#"{"method":"identity"}"#,
        )
        .unwrap();
        assert_eq!(
            execute(
                directory.path(),
                "signer",
                [4; 32],
                &settings(),
                r#"{"method":"outbox"}"#
            )
            .unwrap()["messages"]
                .as_array()
                .unwrap()
                .len(),
            1
        );
        assert!(execute(directory.path(), "signer", [5; 32], &settings(), &command).is_err());
        execute(
            directory.path(),
            "wallet",
            [6; 32],
            &settings(),
            r#"{"method":"initialize","network":"regtest"}"#,
        )
        .unwrap();
        assert!(execute(directory.path(), "wallet", [6; 32], &settings(), &command).is_err());
        assert!(
            execute(
                directory.path(),
                "wallet",
                [6; 32],
                &settings(),
                r#"{"method":"outbox"}"#
            )
            .unwrap()["messages"]
                .as_array()
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn native_device_secrets_are_protected_and_survive_reopening() {
        let directory = tempfile::tempdir().unwrap();
        let phone = execute(
            directory.path(),
            "wallet",
            [1; 32],
            &settings(),
            r#"{"method":"initialize","network":"regtest"}"#,
        )
        .unwrap();
        let bytes =
            std::fs::read_to_string(directory.path().join("wallet/phone/device.json")).unwrap();
        assert!(!bytes.contains("mnemonic"));
        assert!(bytes.contains("anzen_native_storage"));
        assert_eq!(
            phone,
            execute(
                directory.path(),
                "wallet",
                [1; 32],
                &settings(),
                r#"{"method":"identity"}"#
            )
            .unwrap()
        );
        assert!(
            execute(
                directory.path(),
                "wallet",
                [2; 32],
                &settings(),
                r#"{"method":"identity"}"#
            )
            .is_err()
        );
        let address1 = execute(
            directory.path(),
            "wallet",
            [1; 32],
            &settings(),
            r#"{"method":"receive_address"}"#,
        )
        .unwrap();
        let address2 = execute(
            directory.path(),
            "wallet",
            [1; 32],
            &settings(),
            r#"{"method":"receive_address"}"#,
        )
        .unwrap();
        assert_ne!(address1, address2);
        // BDK's persistent changeset must not contain descriptor private keys.
        let db = std::fs::read(directory.path().join("wallet/phone/hot-wallet.sqlite")).unwrap();
        assert!(
            !db.windows(4)
                .any(|chunk| chunk == b"tprv" || chunk == b"xprv")
        );
    }

    #[test]
    fn native_role_pairing_is_offline_and_does_not_copy_private_keys() {
        let phone = tempfile::tempdir().unwrap();
        let signer = tempfile::tempdir().unwrap();
        execute(
            phone.path(),
            "wallet",
            [1; 32],
            &settings(),
            r#"{"method":"initialize","network":"regtest"}"#,
        )
        .unwrap();
        let identity = execute(
            signer.path(),
            "signer",
            [2; 32],
            &settings(),
            r#"{"method":"initialize","network":"regtest"}"#,
        )
        .unwrap();
        let config = execute(
            phone.path(),
            "wallet",
            [1; 32],
            &settings(),
            &json!({"method":"pair_signer", "signer":identity}).to_string(),
        )
        .unwrap();
        execute(
            signer.path(),
            "signer",
            [2; 32],
            &settings(),
            &json!({"method":"accept_vault", "config":config}).to_string(),
        )
        .unwrap();
        assert!(!phone.path().join("signer").exists());
        assert!(!signer.path().join("wallet").exists());
        assert!(
            execute(
                signer.path(),
                "signer",
                [2; 32],
                &settings(),
                r#"{"method":"send","destination":"invalid","amount_sats":1}"#
            )
            .is_err()
        );
        assert!(
            execute(
                phone.path(),
                "wallet",
                [1; 32],
                &settings(),
                r#"{"method":"approve_policy","review_id":"invalid"}"#
            )
            .is_err()
        );
    }

    #[test]
    fn native_mainnet_opt_in_cannot_be_bypassed() {
        let dir = tempfile::tempdir().unwrap();
        assert!(
            execute(
                dir.path(),
                "wallet",
                [1; 32],
                &settings(),
                r#"{"method":"initialize","network":"bitcoin"}"#
            )
            .is_err()
        );
    }

    #[test]
    fn setup_tap_pairs_and_protects_the_backup_in_one_exchange() {
        let phone = tempfile::tempdir().unwrap();
        let signer = tempfile::tempdir().unwrap();
        let other = tempfile::tempdir().unwrap();
        let call = |root: &Path, role: &str, key: u8, command: Value| {
            execute(root, role, [key; 32], &settings(), &command.to_string())
        };
        let initialize = json!({"method":"initialize", "network":"regtest"});
        call(phone.path(), "wallet", 1, initialize.clone()).unwrap();
        let hww = call(signer.path(), "signer", 2, initialize.clone()).unwrap();
        let stranger = call(other.path(), "signer", 3, initialize).unwrap();
        let setup = call(
            phone.path(),
            "wallet",
            1,
            json!({"method":"setup_vault", "signer":hww}),
        )
        .unwrap();
        // An interrupted tap can be repeated with the same signer, never switched to another.
        let setup = call(
            phone.path(),
            "wallet",
            1,
            json!({"method":"setup_vault", "signer":hww}),
        )
        .map(|again| {
            assert_eq!(again["config"], setup["config"]);
            again
        })
        .unwrap();
        assert!(
            call(
                phone.path(),
                "wallet",
                1,
                json!({"method":"setup_vault", "signer":stranger}),
            )
            .is_err()
        );
        let accepted = call(
            signer.path(),
            "signer",
            2,
            json!({"method":"accept_setup", "config":setup["config"], "backup":setup["backup"]}),
        )
        .unwrap();
        assert_eq!(accepted["vault_address"], setup["config"]["vault_address"]);
        call(
            phone.path(),
            "wallet",
            1,
            json!({"method":"store_backup", "message":accepted["backup"]}),
        )
        .unwrap();
        let session = call(phone.path(), "wallet", 1, json!({"method":"session"})).unwrap();
        assert_eq!(session["paired"], true);
        assert_eq!(session["backup_stored"], true);
        let signer_session = call(signer.path(), "signer", 2, json!({"method":"session"})).unwrap();
        assert_eq!(signer_session["config"], setup["config"]);
        assert!(
            call(
                phone.path(),
                "wallet",
                1,
                json!({"method":"setup_vault", "signer":hww}),
            )
            .is_err()
        );
    }

    #[test]
    fn cloud_restore_uses_only_encrypted_native_storage_and_public_messages() {
        let owner = tempfile::tempdir().unwrap();
        let signer = tempfile::tempdir().unwrap();
        let replacement = tempfile::tempdir().unwrap();
        let call = |root: &Path, role: &str, key: u8, command: Value| {
            execute(root, role, [key; 32], &settings(), &command.to_string()).unwrap()
        };
        let initialize = json!({"method":"initialize", "network":"regtest"});
        let old_phone = call(owner.path(), "wallet", 1, initialize.clone());
        let hww = call(signer.path(), "signer", 2, initialize.clone());
        let setup = call(
            owner.path(),
            "wallet",
            1,
            json!({"method":"setup_vault", "signer":hww}),
        );
        let accepted = call(
            signer.path(),
            "signer",
            2,
            json!({"method":"accept_setup", "config":setup["config"], "backup":setup["backup"]}),
        );
        call(
            owner.path(),
            "wallet",
            1,
            json!({"method":"store_backup", "message":accepted["backup"]}),
        );
        let config = setup["config"].clone();
        let backup = call(owner.path(), "wallet", 1, json!({"method":"export_backup"}));
        // The phone is lost. Only the cloud backup and the signer remain.
        drop(owner);
        let recipient = call(replacement.path(), "wallet", 4, initialize);
        let original_key =
            std::fs::read(replacement.path().join("wallet/phone/device.json")).unwrap();
        call(
            replacement.path(),
            "wallet",
            4,
            json!({"method":"import_recovery_vault", "config":config}),
        );
        let request = call(
            replacement.path(),
            "wallet",
            4,
            json!({"method":"propose_restore", "backup":backup}),
        );
        assert!(request.get("encrypted_payload").is_none());
        let review = call(
            signer.path(),
            "signer",
            2,
            json!({"method":"review_restore", "request":request}),
        );
        assert_eq!(review["recipient_public_key"], recipient["vault_pubkey"]);
        let approved = call(
            signer.path(),
            "signer",
            2,
            json!({"method":"approve_restore", "review_id":review["review_id"]}),
        );
        assert!(approved.get("symmetric_key").is_none());
        let restored = call(
            replacement.path(),
            "wallet",
            4,
            json!({"method":"store_restored_key", "message":approved}),
        );
        assert_eq!(restored["old_phone_public_key"], old_phone["vault_pubkey"]);
        assert_eq!(restored["vault_address"], config["vault_address"]);
        assert_eq!(
            original_key,
            std::fs::read(replacement.path().join("wallet/phone/device.json")).unwrap()
        );
        let protected = replacement.path().join("wallet/phone");
        let stored = std::fs::read_to_string(protected.join("pending-restore.json")).unwrap();
        assert!(stored.contains("anzen_native_storage"));
        assert!(!stored.contains(config["vault_address"].as_str().unwrap()));
        for key in std::fs::read_dir(protected.join("restored-keys")).unwrap() {
            let stored = std::fs::read_to_string(key.unwrap().path()).unwrap();
            assert!(stored.contains("anzen_native_storage"));
            assert!(!stored.contains("mnemonic"));
        }
        assert!(!replacement.path().join("signer").exists());
        assert!(
            execute(
                replacement.path(),
                "wallet",
                [5; 32],
                &settings(),
                r#"{"method":"restored_phone"}"#
            )
            .is_err()
        );
    }

    #[test]
    fn tampered_encrypted_state_is_rejected_without_replacing_keys() {
        let dir = tempfile::tempdir().unwrap();
        execute(
            dir.path(),
            "wallet",
            [1; 32],
            &settings(),
            r#"{"method":"initialize","network":"regtest"}"#,
        )
        .unwrap();
        let path = dir.path().join("wallet/phone/device.json");
        let mut envelope: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        let ciphertext = envelope["ciphertext"].as_str().unwrap();
        let mut bytes = ciphertext.as_bytes().to_vec();
        bytes[0] = if bytes[0] == b'A' { b'B' } else { b'A' };
        envelope["ciphertext"] = String::from_utf8(bytes).unwrap().into();
        // Simulates a corrupt on-disk ciphertext, not an application write path.
        std::fs::write(&path, serde_json::to_vec(&envelope).unwrap()).unwrap();
        assert!(
            execute(
                dir.path(),
                "wallet",
                [1; 32],
                &settings(),
                r#"{"method":"identity"}"#
            )
            .is_err()
        );
        assert!(
            execute(
                dir.path(),
                "wallet",
                [1; 32],
                &settings(),
                r#"{"method":"initialize","network":"regtest"}"#
            )
            .is_err()
        );
    }

    #[test]
    fn c_abi_allocates_and_frees_owned_json() {
        let root = CString::new(tempfile::tempdir().unwrap().path().to_str().unwrap()).unwrap();
        let role = c"signer";
        let settings = c"{\"backend\":{\"type\":\"electrum\"}}";
        let command = c"{\"method\":\"identity\"}";
        // SAFETY: all pointers remain valid for the call; returned allocation is freed once.
        unsafe {
            let result = anzen_mobile_execute(
                root.as_ptr(),
                role.as_ptr(),
                [1u8; 32].as_ptr(),
                settings.as_ptr(),
                command.as_ptr(),
            );
            let response: Value =
                serde_json::from_slice(CStr::from_ptr(result).to_bytes()).unwrap();
            assert!(response.get("error").is_some());
            anzen_mobile_free(result);
            anzen_mobile_free(std::ptr::null_mut());
        }
    }
}
