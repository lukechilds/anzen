//! Loopback-only public chain data, encrypted cloud storage and regtest controls.
//! Wallet keys and application commands exist only in browser-local WASM devices.

mod chain;
mod cloud;
mod jobs;
mod node;

use anyhow::{Context, Result, bail};
use anzen_wallet::{
    HotWalletBackend,
    api::{BitcoinCoreBackend, Network, RpcConfig},
};
use axum::{
    Json, Router,
    extract::{DefaultBodyLimit, State},
    http::{HeaderMap, HeaderValue, Method, StatusCode, header},
    routing::post,
};
use clap::Parser;
use rand::RngCore;
use serde::Deserialize;
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{Arc, Mutex},
};
use tower_http::cors::CorsLayer;

#[derive(Parser)]
struct Args {
    #[arg(long, default_value = ".playground-data")]
    data_dir: PathBuf,
    #[arg(long, default_value_t = 8787)]
    port: u16,
    #[arg(long)]
    dangerously_enable_mainnet: bool,
    #[arg(long, env = "ANZEN_RPC_URL", default_value = "http://127.0.0.1:18443")]
    rpc_url: String,
    #[arg(long, env = "ANZEN_RPC_USER", default_value = "anzen")]
    rpc_user: String,
    #[arg(long, env = "ANZEN_RPC_PASSWORD", default_value = "anzen")]
    rpc_password: String,
}

struct Host {
    directory: PathBuf,
    admin_token: String,
    allow_mainnet: bool,
    rpc: RpcConfig,
    node_gate: Mutex<()>,
    jobs: Mutex<HashMap<String, Arc<jobs::Job>>>,
}

type Shared = Arc<Host>;
type ApiResult = std::result::Result<Json<Value>, (StatusCode, Json<Value>)>;

fn failure(error: impl std::fmt::Display) -> (StatusCode, Json<Value>) {
    (
        StatusCode::BAD_REQUEST,
        Json(json!({ "error": error.to_string() })),
    )
}

fn token() -> String {
    let mut bytes = [0u8; 32];
    rand::rngs::OsRng.fill_bytes(&mut bytes);
    bitcoin::hex::DisplayHex::to_lower_hex_string(&bytes[..])
}

fn bearer(headers: &HeaderMap) -> Result<&str> {
    // CORS controls reads, but this also rejects cross-origin writes before invoking a device.
    if let Some(origin) = headers.get(header::ORIGIN) {
        let origin = origin.to_str()?;
        if !["http://127.0.0.1:8081", "http://localhost:8081"].contains(&origin) {
            bail!("untrusted playground origin");
        }
    }
    headers
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .context("playground capability required")
}

fn require_admin(host: &Host, headers: &HeaderMap) -> Result<()> {
    if bearer(headers)? != host.admin_token {
        bail!("invalid playground administrator capability");
    }
    Ok(())
}

async fn session_settings(State(host): State<Shared>, headers: HeaderMap) -> ApiResult {
    require_admin(&host, &headers).map_err(failure)?;
    Ok(Json(json!({ "allow_mainnet": host.allow_mainnet })))
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();
    // Only encrypted cloud objects and regtest metadata are stored by the host.
    std::fs::create_dir_all(&args.data_dir)?;
    let session_dir = args.data_dir.join(token());
    std::fs::create_dir(&session_dir)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&session_dir, std::fs::Permissions::from_mode(0o700))?;
    }
    let host = Arc::new(Host {
        directory: session_dir,
        admin_token: token(),
        allow_mainnet: args.dangerously_enable_mainnet,
        rpc: RpcConfig {
            url: args.rpc_url,
            user: args.rpc_user,
            password: args.rpc_password,
        },
        node_gate: Mutex::new(()),
        jobs: Mutex::new(HashMap::new()),
    });
    let cors = CorsLayer::new()
        .allow_origin([
            HeaderValue::from_static("http://127.0.0.1:8081"),
            HeaderValue::from_static("http://localhost:8081"),
        ])
        .allow_methods([Method::POST])
        .allow_headers([header::AUTHORIZATION, header::CONTENT_TYPE]);
    let router = Router::new()
        .route("/session", post(session_settings))
        .route("/chain", post(chain::control))
        .route("/node", post(node::control))
        .route("/node/job", post(jobs::control))
        .route("/cloud", post(cloud::control))
        .layer(DefaultBodyLimit::max(8 * 1024 * 1024))
        .layer(cors)
        .with_state(host.clone());
    let listener =
        tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, args.port)).await?;
    println!("Anzen local playground — real Rust wallets; isolated development state");
    println!(
        "Open http://localhost:8081/#playground={}&port={}",
        host.admin_token, args.port
    );
    println!("State: {}", host.directory.display());
    println!("Keep this capability URL private. Restarting the host creates a new session.");
    axum::serve(listener, router)
        .with_graceful_shutdown(async move {
            let _ = tokio::signal::ctrl_c().await;
            jobs::cancel_all(&host);
        })
        .await?;
    Ok(())
}
