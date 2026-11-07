//! Runs the shared listener on a native background thread: Electrum over the Rust WebSocket
//! client, or Bitcoin Core polling on regtest. The app collects the hints with [`Watch::changes`].

use super::{Action, ChainWatcher, Change, RegtestActivity};
use crate::electrum::native::NativeSocket;
use anyhow::Result;
use anzen_chain::{BitcoinCoreBackend, RpcConfig};
use bitcoin::Network;
use bitcoincore_rpc::RpcApi;
use std::{
    collections::BTreeSet,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

const TICK: Duration = Duration::from_secs(1);

#[derive(Default)]
struct Shared {
    changes: BTreeSet<Change>,
    targets: Option<Vec<String>>,
}

/// A running listener. Dropping it stops the thread.
pub struct Watch {
    shared: Arc<Mutex<Shared>>,
    stop: Arc<AtomicBool>,
}

impl Watch {
    /// Mutinynet and mainnet use Electrum; regtest polls the node given by `rpc`.
    pub fn start(network: Network, rpc: Option<RpcConfig>) -> Result<Self> {
        Self::spawn(ChainWatcher::new(network)?, rpc)
    }

    /// Runs a given listener, such as one with explicit servers.
    pub fn spawn(mut watcher: ChainWatcher, rpc: Option<RpcConfig>) -> Result<Self> {
        let shared = Arc::new(Mutex::new(Shared::default()));
        let stop = Arc::new(AtomicBool::new(false));
        let (thread_shared, thread_stop) = (shared.clone(), stop.clone());
        thread::Builder::new()
            .name("anzen-chain-watch".into())
            .spawn(move || run(&mut watcher, rpc, &thread_shared, &thread_stop))?;
        Ok(Self { shared, stop })
    }

    /// Replaces the scripts to watch. Only new ones are subscribed.
    pub fn set_targets(&self, scripthashes: Vec<String>) {
        if let Ok(mut shared) = self.shared.lock() {
            shared.targets = Some(scripthashes);
        }
    }

    /// Takes the reasons to refresh that arrived since the last call.
    pub fn changes(&self) -> Vec<Change> {
        self.shared
            .lock()
            .map(|mut shared| std::mem::take(&mut shared.changes).into_iter().collect())
            .unwrap_or_default()
    }
}

impl Drop for Watch {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

fn run(
    watcher: &mut ChainWatcher,
    rpc: Option<RpcConfig>,
    shared: &Mutex<Shared>,
    stop: &AtomicBool,
) {
    let started = Instant::now();
    let now = || started.elapsed().as_millis() as u64;
    let mut socket: Option<NativeSocket> = None;
    let mut node: Option<BitcoinCoreBackend> = None;
    while !stop.load(Ordering::Relaxed) {
        let mut actions = Vec::new();
        if let Some(targets) = shared.lock().ok().and_then(|mut s| s.targets.take()) {
            actions.extend(watcher.watch(&targets));
        }
        if watcher.polls_activity() {
            if let Some(activity) = rpc
                .as_ref()
                .and_then(|rpc| regtest_activity(&mut node, rpc))
            {
                actions.extend(watcher.activity(&activity));
            }
        }
        actions.extend(watcher.tick(now()));
        perform(watcher, actions, &mut socket, shared, &now);
        match socket.as_mut().map(|socket| socket.receive_within(TICK)) {
            Some(Ok(Some(frame))) => {
                let actions = watcher.received(&frame, now());
                perform(watcher, actions, &mut socket, shared, &now);
            }
            Some(Ok(None)) => {}
            Some(Err(_)) => {
                socket = None;
                let actions = watcher.closed(now());
                perform(watcher, actions, &mut socket, shared, &now);
            }
            None => thread::sleep(TICK),
        }
    }
}

fn perform(
    watcher: &mut ChainWatcher,
    actions: Vec<Action>,
    socket: &mut Option<NativeSocket>,
    shared: &Mutex<Shared>,
    now: &dyn Fn() -> u64,
) {
    let mut queue = std::collections::VecDeque::from(actions);
    while let Some(action) = queue.pop_front() {
        match action {
            Action::Connect { url } => {
                *socket = None;
                let next = match NativeSocket::connect(&url) {
                    Ok(connected) => {
                        *socket = Some(connected);
                        watcher.opened(now())
                    }
                    Err(_) => watcher.closed(now()),
                };
                queue.extend(next);
            }
            Action::Send { frame } => {
                use crate::electrum::FrameTransport;
                if let Some(connected) = socket.as_mut() {
                    if connected.send(&frame).is_err() {
                        *socket = None;
                        queue.extend(watcher.closed(now()));
                    }
                }
            }
            Action::Close => *socket = None,
            Action::Changed { reason } => {
                if let Ok(mut shared) = shared.lock() {
                    shared.changes.insert(reason);
                }
            }
        }
    }
}

fn regtest_activity(
    node: &mut Option<BitcoinCoreBackend>,
    rpc: &RpcConfig,
) -> Option<RegtestActivity> {
    if node.is_none() {
        *node = BitcoinCoreBackend::connect(rpc, Network::Regtest).ok();
    }
    let client = &node.as_ref()?.client;
    let observed = (|| -> Result<RegtestActivity> {
        let tip = client.get_best_block_hash()?.to_string();
        let mempool: serde_json::Value = client.call(
            "getrawmempool",
            &[serde_json::json!(false), serde_json::json!(true)],
        )?;
        Ok(RegtestActivity {
            tip,
            mempool_sequence: mempool["mempool_sequence"].as_u64().unwrap_or_default(),
            mining: false,
        })
    })();
    if observed.is_err() {
        *node = None; // Reconnect on the next tick.
    }
    observed.ok()
}
