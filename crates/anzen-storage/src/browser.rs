//! Synchronous host calls from a dedicated WASM worker. The host must not acknowledge
//! storage writes until committed. No UI callback may implement wallet decisions.

use anyhow::{Result, bail};
use serde::{Serialize, de::DeserializeOwned};
use wasm_bindgen::prelude::*;

#[wasm_bindgen]
extern "C" {
    #[wasm_bindgen(catch, js_name = anzenPlatformCall)]
    fn platform_call(request: &str) -> Result<String, JsValue>;
}

pub fn call<T: DeserializeOwned>(request: &impl Serialize) -> Result<T> {
    let response = platform_call(&serde_json::to_string(request)?).map_err(|error| {
        anyhow::anyhow!(
            "browser platform unavailable: {}",
            error
                .as_string()
                .unwrap_or_else(|| "worker interrupted".into())
        )
    })?;
    let envelope: serde_json::Value = serde_json::from_str(&response)?;
    if let Some(error) = envelope.get("error") {
        bail!("browser platform: {error}");
    }
    Ok(serde_json::from_value(
        envelope
            .get("ok")
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("missing platform response"))?,
    )?)
}
