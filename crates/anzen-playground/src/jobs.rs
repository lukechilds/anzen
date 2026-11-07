//! Long regtest mining runs survive HTTP requests and report progress independently.
//! Cancelling stops at a batch boundary; blocks already mined are never rolled back.
use super::*;
use anzen_wallet::api::Blockchain;
use bitcoincore_rpc::RpcApi;
use serde::Serialize;
use std::sync::atomic::{AtomicBool, Ordering};

pub struct Job {
    snapshot: Mutex<Snapshot>,
    cancel: AtomicBool,
}

#[derive(Clone, Serialize)]
pub struct Snapshot {
    id: String,
    status: String,
    requested_blocks: u64,
    mined_blocks: u64,
    height: Option<u64>,
    error: Option<String>,
}

#[derive(Deserialize)]
#[serde(tag = "method", rename_all = "snake_case", deny_unknown_fields)]
pub enum Request {
    Start { blocks: u64 },
    Status { id: String },
    Cancel { id: String },
    List,
}

pub async fn control(
    State(host): State<Shared>,
    headers: HeaderMap,
    Json(request): Json<Request>,
) -> ApiResult {
    require_admin(&host, &headers).map_err(failure)?;
    let cancel_requested = matches!(&request, Request::Cancel { .. });
    match request {
        Request::Start { blocks } => {
            if !(1..=70_000).contains(&blocks) {
                return Err(failure("mine between 1 and 70000 blocks per job"));
            }
            let mut jobs = host.jobs.lock().map_err(failure)?;
            if jobs.len() >= 32 {
                return Err(failure(
                    "session job limit reached; start a fresh playground session",
                ));
            }
            for job in jobs.values() {
                let status = job.snapshot.lock().map_err(failure)?.status.clone();
                if status == "queued" || status == "running" {
                    return Err(failure("a mining job is already active"));
                }
            }
            let id = token();
            let initial = Snapshot {
                id: id.clone(),
                status: "queued".into(),
                requested_blocks: blocks,
                mined_blocks: 0,
                height: None,
                error: None,
            };
            let job = Arc::new(Job {
                snapshot: Mutex::new(initial.clone()),
                cancel: AtomicBool::new(false),
            });
            jobs.insert(id, job.clone());
            drop(jobs);
            tokio::task::spawn_blocking(move || {
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    mine(&host, &job, blocks)
                }))
                .unwrap_or_else(|_| Err(anyhow::anyhow!("mining worker interrupted")));
                if let Err(error) = result {
                    if let Ok(mut snapshot) = job.snapshot.lock() {
                        snapshot.status = "failed".into();
                        snapshot.error = Some(format!("{error:#}"));
                    }
                }
            });
            Ok(Json(serde_json::to_value(initial).map_err(failure)?))
        }
        Request::Status { id } | Request::Cancel { id } => {
            let job = host
                .jobs
                .lock()
                .map_err(failure)?
                .get(&id)
                .cloned()
                .ok_or_else(|| failure("unknown mining job"))?;
            if cancel_requested {
                job.cancel.store(true, Ordering::Relaxed);
            }
            let value = job.snapshot.lock().map_err(failure)?.clone();
            Ok(Json(serde_json::to_value(value).map_err(failure)?))
        }
        Request::List => {
            let jobs = host.jobs.lock().map_err(failure)?;
            let snapshots = jobs
                .values()
                .map(|job| job.snapshot.lock().map(|s| s.clone()).map_err(failure))
                .collect::<std::result::Result<Vec<_>, _>>()?;
            Ok(Json(json!({ "jobs": snapshots })))
        }
    }
}

/// Whether a mining job is queued or running. Wallets defer refreshing until it finishes.
pub fn mining_active(host: &Host) -> bool {
    host.jobs.lock().is_ok_and(|jobs| {
        jobs.values().any(|job| {
            job.snapshot
                .lock()
                .is_ok_and(|snapshot| ["queued", "running"].contains(&snapshot.status.as_str()))
        })
    })
}

pub fn cancel_all(host: &Host) {
    if let Ok(jobs) = host.jobs.lock() {
        for job in jobs.values() {
            job.cancel.store(true, Ordering::Relaxed);
        }
    }
}

fn mine(host: &Host, job: &Job, blocks: u64) -> Result<()> {
    let _guard = host
        .node_gate
        .lock()
        .map_err(|_| anyhow::anyhow!("node operation interrupted"))?;
    let backend = BitcoinCoreBackend::connect(&host.rpc, Network::Regtest)?;
    let mut mined = 0;
    let start = backend.chain_tip()?.height;
    job.snapshot
        .lock()
        .map_err(|_| anyhow::anyhow!("job interrupted"))?
        .status = "running".into();
    while mined < blocks {
        if job.cancel.load(Ordering::Relaxed) {
            job.snapshot
                .lock()
                .map_err(|_| anyhow::anyhow!("job interrupted"))?
                .status = "cancelled".into();
            return Ok(());
        }
        let count = (blocks - mined).min(250);
        let header = backend
            .client
            .get_block_header_info(&backend.client.get_best_block_hash()?)?;
        let now = u64::try_from(chrono::Utc::now().timestamp())?;
        // Keep mock time ahead of MTP during fast height mining. These are real blocks at an
        // accelerated test cadence, not a claim that 14 calendar months have elapsed.
        backend.set_mock_time(
            u64::try_from(header.time)?
                .max(now)
                .checked_add(count)
                .context("mock time overflow")?,
        )?;
        backend.mine(count, &node::mining_address())?;
        mined += count;
        let mut snapshot = job
            .snapshot
            .lock()
            .map_err(|_| anyhow::anyhow!("job interrupted"))?;
        snapshot.mined_blocks = mined;
        snapshot.height = Some(start + mined);
    }
    job.snapshot
        .lock()
        .map_err(|_| anyhow::anyhow!("job interrupted"))?
        .status = "completed".into();
    Ok(())
}
