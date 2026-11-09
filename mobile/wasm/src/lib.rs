//! Browser bindings only. All wallet/signing behavior stays in the shared role facades.

use anzen_signer_app::application::{SignerApplication, SignerRequest};
use anzen_wallet::application::{WalletApplication, WalletRequest};
use serde_json::{Value, json};
use std::path::PathBuf;
use wasm_bindgen::prelude::*;

/// The live-update listener, run by the device's I/O worker. It holds no keys or wallet state:
/// JavaScript reports socket events and time, and carries out the returned actions (JSON).
#[wasm_bindgen]
pub struct ChainWatcher(anzen_wallet::watch::ChainWatcher);

fn actions(actions: Vec<anzen_wallet::watch::Action>) -> String {
    serde_json::to_string(&actions).unwrap_or_else(|_| "[]".into())
}

#[wasm_bindgen]
impl ChainWatcher {
    #[wasm_bindgen(constructor)]
    pub fn new(network: &str) -> Result<ChainWatcher, JsError> {
        let network = serde_json::from_value(Value::String(network.into()))
            .map_err(|error| JsError::new(&error.to_string()))?;
        anzen_wallet::watch::ChainWatcher::new(network)
            .map(ChainWatcher)
            .map_err(|error| JsError::new(&format!("{error:#}")))
    }
    pub fn polls_activity(&self) -> bool {
        self.0.polls_activity()
    }
    /// `scripthashes`: a JSON array from the wallet's `watch_targets`.
    pub fn watch(&mut self, scripthashes: &str) -> String {
        let targets: Vec<String> = serde_json::from_str(scripthashes).unwrap_or_default();
        actions(self.0.watch(&targets))
    }
    pub fn opened(&mut self, now: f64) -> String {
        actions(self.0.opened(now as u64))
    }
    pub fn received(&mut self, frame: &str, now: f64) -> String {
        actions(self.0.received(frame, now as u64))
    }
    pub fn closed(&mut self, now: f64) -> String {
        actions(self.0.closed(now as u64))
    }
    pub fn tick(&mut self, now: f64) -> String {
        actions(self.0.tick(now as u64))
    }
    /// `activity`: the playground host's regtest activity report (JSON).
    pub fn activity(&mut self, activity: &str) -> String {
        match serde_json::from_str(activity) {
            Ok(activity) => actions(self.0.activity(&activity)),
            Err(_) => "[]".into(),
        }
    }
}

#[wasm_bindgen]
pub fn execute(role: &str, allow_mainnet: bool, command: &str) -> String {
    let result = (|| -> anyhow::Result<Value> {
        anyhow::ensure!(
            command.len() <= 8 * 1024 * 1024,
            "device request exceeds 8 MiB"
        );
        let root = PathBuf::from("/device");
        match role {
            "wallet" => {
                let app = WalletApplication::new(root, allow_mainnet);
                let request: WalletRequest = serde_json::from_str(command)?;
                let backend = if request.requires_chain() {
                    Some(anzen_wallet::browser_backend::connect(
                        app.public_identity()?.bitcoin_network()?,
                    )?)
                } else {
                    None
                };
                app.execute(request, backend.as_deref())
            }
            "signer" => SignerApplication::new(root, allow_mainnet)
                .execute(serde_json::from_str::<SignerRequest>(command)?),
            _ => anyhow::bail!("invalid browser device role"),
        }
    })();
    match result {
        Ok(value) => json!({"ok":value}),
        Err(error) => json!({"error":format!("{error:#}")}),
    }
    .to_string()
}

#[wasm_bindgen]
extern "C" {
    /// Browsers have no `std::time::Instant`; workers and pages both have `performance`.
    #[wasm_bindgen(js_namespace = performance, js_name = now)]
    fn performance_now() -> f64;
    /// Set by the benchmark worker: a panic inside a WebGPU callback ends the worker without
    /// a result, so the panic's message is reported through this instead.
    #[wasm_bindgen(js_name = anzenBenchmarkPanic)]
    fn benchmark_panic(message: &str);
}

/// The GPU vanity-search benchmark on fixed public inputs, through WebGPU. `request` is
/// `{"seconds":n}`; resolves to the same JSON envelope as `execute`.
/// It touches no device storage, so it runs in its own worker.
#[wasm_bindgen]
pub async fn benchmark(request: String) -> String {
    #[derive(serde::Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Request {
        seconds: f64,
    }
    std::panic::set_hook(Box::new(|info| benchmark_panic(&info.to_string())));
    let result = async {
        let request: Request = serde_json::from_str(&request)?;
        let report = anzen_vanity::bench::run_wgpu(request.seconds.clamp(1.0, 300.0), || {
            performance_now() / 1000.0
        })
        .await?;
        let measurement = report.measurement;
        anyhow::Ok(json!({
            "adapter": report.adapter,
            "setup_seconds": report.setup_seconds,
            "batch": measurement.batch,
            "attempts": measurement.attempts,
            "seconds": measurement.seconds,
            "rate": measurement.rate,
        }))
    }
    .await;
    match result {
        Ok(value) => json!({"ok":value}),
        Err(error) => json!({"error":format!("{error:#}")}),
    }
    .to_string()
}
