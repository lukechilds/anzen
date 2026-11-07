//! The live-update listener as a state machine with no I/O, so every platform shares it.

use crate::electrum::{FrameDecoder, default_servers};
use anyhow::Result;
use bitcoin::Network;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::BTreeSet;

/// How long a connection may take to open.
pub const CONNECT_TIMEOUT_MS: u64 = 10_000;
/// Retry a lost subscription connection after this long, rotating servers.
pub const RETRY_MS: u64 = 30_000;
/// While disconnected, suggest a refresh this often so changes still appear.
pub const POLL_MS: u64 = 30_000;
/// Servers drop idle sessions; a ping keeps the subscription alive.
pub const PING_MS: u64 = 60_000;
/// Scripts subscribed per request frame.
const BATCH: usize = 100;

/// Why a refresh is suggested.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Change {
    /// A new block.
    Block,
    /// A watched script's history changed, in the mempool or a block.
    Address,
    /// A subscription was established. Anything before it (since the app's last sync, or
    /// while disconnected) may have been missed.
    Subscribed,
    /// No subscription: a timed refresh stands in for notifications.
    Poll,
    /// Regtest: the node's tip or mempool changed, or polling started.
    Activity,
}

/// What the platform should do next.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum Action {
    /// Open a WebSocket to `url`, replacing any current connection, and report `opened` or
    /// `closed`.
    Connect { url: String },
    /// Send one text frame on the open connection.
    Send { frame: String },
    /// Close the current connection. No `closed` report is expected.
    Close,
    /// Refresh the wallet.
    Changed { reason: Change },
}

#[derive(Debug, PartialEq, Eq)]
enum State {
    Waiting,
    Connecting { deadline: u64 },
    Live,
}

/// Electrum subscriptions to new block headers and to each watched script hash.
pub struct ElectrumWatcher {
    servers: Vec<String>,
    server: usize,
    state: State,
    targets: BTreeSet<String>,
    subscribed: BTreeSet<String>,
    decoder: FrameDecoder,
    next_id: u64,
    retry_at: u64,
    poll_at: u64,
    ping_at: u64,
}

impl ElectrumWatcher {
    pub fn new(network: Network) -> Result<Self> {
        let servers: Vec<String> = default_servers(network)?
            .iter()
            .map(|s| (*s).to_owned())
            .collect();
        Ok(Self::with_servers(servers))
    }

    pub fn with_servers(servers: Vec<String>) -> Self {
        // Spread devices across servers, as the sync backend does.
        let server = rand::random::<u8>() as usize % servers.len().max(1);
        Self {
            servers,
            server,
            state: State::Waiting,
            targets: BTreeSet::new(),
            subscribed: BTreeSet::new(),
            decoder: FrameDecoder::default(),
            next_id: 0,
            retry_at: 0,
            poll_at: u64::MAX,
            ping_at: u64::MAX,
        }
    }

    /// Adds scripts to watch; only new ones are subscribed. Unsubscribing is unnecessary for a
    /// hint channel.
    pub fn watch(&mut self, scripthashes: &[String]) -> Vec<Action> {
        for hash in scripthashes {
            if hash.len() == 64 && hash.bytes().all(|b| b.is_ascii_hexdigit()) {
                self.targets.insert(hash.to_ascii_lowercase());
            }
        }
        if self.state == State::Live {
            self.subscribe()
        } else {
            Vec::new()
        }
    }

    pub fn opened(&mut self, now: u64) -> Vec<Action> {
        if !matches!(self.state, State::Connecting { .. }) {
            return Vec::new();
        }
        self.state = State::Live;
        self.subscribed.clear();
        self.decoder = FrameDecoder::default();
        self.ping_at = now + PING_MS;
        self.poll_at = u64::MAX;
        let mut actions = vec![self.request(vec![
            ("server.version", json!(["anzen-watch", "1.4"])),
            ("blockchain.headers.subscribe", json!([])),
        ])];
        actions.extend(self.subscribe());
        // Notifications only cover what happens from now on; refresh once to cover the gap.
        actions.push(Action::Changed {
            reason: Change::Subscribed,
        });
        actions
    }

    pub fn received(&mut self, frame: &str, now: u64) -> Vec<Action> {
        if self.state != State::Live {
            return Vec::new();
        }
        let messages = match self.decoder.push(frame) {
            Ok(messages) => messages,
            Err(_) => {
                let mut actions = vec![Action::Close];
                actions.extend(self.closed(now));
                return actions;
            }
        };
        let mut changes = BTreeSet::new();
        for message in messages {
            match message["method"].as_str() {
                Some("blockchain.headers.subscribe") => changes.insert(Change::Block),
                Some("blockchain.scripthash.subscribe") => changes.insert(Change::Address),
                _ => false,
            };
        }
        changes
            .into_iter()
            .map(|reason| Action::Changed { reason })
            .collect()
    }

    pub fn closed(&mut self, now: u64) -> Vec<Action> {
        if self.state == State::Waiting {
            return Vec::new();
        }
        let was_live = self.state == State::Live;
        self.state = State::Waiting;
        self.server = (self.server + 1) % self.servers.len().max(1);
        self.retry_at = now + RETRY_MS;
        self.poll_at = now + POLL_MS;
        self.ping_at = u64::MAX;
        // A live subscription just stopped; refresh now in case the loss hid a change.
        if was_live {
            vec![Action::Changed {
                reason: Change::Poll,
            }]
        } else {
            Vec::new()
        }
    }

    pub fn tick(&mut self, now: u64) -> Vec<Action> {
        let mut actions = Vec::new();
        match self.state {
            State::Waiting if now >= self.retry_at && !self.servers.is_empty() => {
                self.state = State::Connecting {
                    deadline: now + CONNECT_TIMEOUT_MS,
                };
                actions.push(Action::Connect {
                    url: self.servers[self.server].clone(),
                });
            }
            State::Connecting { deadline } if now >= deadline => {
                actions.push(Action::Close);
                actions.extend(self.closed(now));
            }
            State::Live if now >= self.ping_at => {
                self.ping_at = now + PING_MS;
                actions.push(self.request(vec![("server.ping", json!([]))]));
            }
            _ => {}
        }
        if self.state != State::Live && now >= self.poll_at {
            self.poll_at = now + POLL_MS;
            actions.push(Action::Changed {
                reason: Change::Poll,
            });
        }
        actions
    }

    fn subscribe(&mut self) -> Vec<Action> {
        let pending: Vec<String> = self.targets.difference(&self.subscribed).cloned().collect();
        let actions = pending
            .chunks(BATCH)
            .map(|batch| {
                self.request(
                    batch
                        .iter()
                        .map(|hash| ("blockchain.scripthash.subscribe", json!([hash])))
                        .collect(),
                )
            })
            .collect();
        self.subscribed.extend(pending);
        actions
    }

    fn request(&mut self, calls: Vec<(&str, Value)>) -> Action {
        let mut frames: Vec<Value> = calls
            .into_iter()
            .map(|(method, params)| {
                self.next_id += 1;
                json!({"jsonrpc": "2.0", "id": self.next_id, "method": method, "params": params})
            })
            .collect();
        let frame = if frames.len() == 1 {
            frames.remove(0)
        } else {
            Value::Array(frames)
        };
        Action::Send {
            frame: format!("{frame}\n"),
        }
    }
}

/// One regtest observation from the local node.
#[derive(Debug, Clone, Deserialize)]
pub struct RegtestActivity {
    pub tip: String,
    pub mempool_sequence: u64,
    /// The playground is mining a long job; refresh once it finishes.
    #[serde(default)]
    pub mining: bool,
}

/// Regtest has no subscriptions: the platform polls the node's tip and mempool sequence.
#[derive(Default)]
pub struct ActivityTracker {
    last: Option<(String, u64)>,
    deferred: bool,
}

impl ActivityTracker {
    pub fn observe(&mut self, activity: &RegtestActivity) -> Vec<Action> {
        let key = (activity.tip.clone(), activity.mempool_sequence);
        // The first observation is a baseline taken after the app's initial sync. A change in
        // between would otherwise be missed, so it refreshes once too.
        if self.last.as_ref() != Some(&key) {
            self.deferred = true;
        }
        self.last = Some(key);
        if self.deferred && !activity.mining {
            self.deferred = false;
            return vec![Action::Changed {
                reason: Change::Activity,
            }];
        }
        Vec::new()
    }
}

/// The listener for a network: Electrum subscriptions, or regtest activity polling.
pub enum ChainWatcher {
    Electrum(ElectrumWatcher),
    Regtest(ActivityTracker),
}

impl ChainWatcher {
    pub fn new(network: Network) -> Result<Self> {
        Ok(match network {
            Network::Regtest => Self::Regtest(ActivityTracker::default()),
            _ => Self::Electrum(ElectrumWatcher::new(network)?),
        })
    }
    /// Whether the platform should report regtest activity every second.
    pub fn polls_activity(&self) -> bool {
        matches!(self, Self::Regtest(_))
    }
    pub fn watch(&mut self, scripthashes: &[String]) -> Vec<Action> {
        match self {
            Self::Electrum(watcher) => watcher.watch(scripthashes),
            Self::Regtest(_) => Vec::new(),
        }
    }
    pub fn opened(&mut self, now: u64) -> Vec<Action> {
        match self {
            Self::Electrum(watcher) => watcher.opened(now),
            Self::Regtest(_) => Vec::new(),
        }
    }
    pub fn received(&mut self, frame: &str, now: u64) -> Vec<Action> {
        match self {
            Self::Electrum(watcher) => watcher.received(frame, now),
            Self::Regtest(_) => Vec::new(),
        }
    }
    pub fn closed(&mut self, now: u64) -> Vec<Action> {
        match self {
            Self::Electrum(watcher) => watcher.closed(now),
            Self::Regtest(_) => Vec::new(),
        }
    }
    pub fn tick(&mut self, now: u64) -> Vec<Action> {
        match self {
            Self::Electrum(watcher) => watcher.tick(now),
            Self::Regtest(_) => Vec::new(),
        }
    }
    pub fn activity(&mut self, activity: &RegtestActivity) -> Vec<Action> {
        match self {
            Self::Regtest(tracker) => tracker.observe(activity),
            Self::Electrum(_) => Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hash(n: usize) -> String {
        format!("{n:064x}")
    }
    fn watcher() -> ElectrumWatcher {
        let mut watcher = ElectrumWatcher::with_servers(vec!["wss://a".into(), "wss://b".into()]);
        watcher.server = 0;
        watcher
    }
    fn sent(actions: &[Action]) -> Vec<Value> {
        actions
            .iter()
            .filter_map(|action| match action {
                Action::Send { frame } => Some(serde_json::from_str(frame).unwrap()),
                _ => None,
            })
            .collect()
    }
    fn methods(frames: &[Value]) -> Vec<String> {
        frames
            .iter()
            .flat_map(|frame| match frame {
                Value::Array(calls) => calls.clone(),
                call => vec![call.clone()],
            })
            .map(|call| call["method"].as_str().unwrap().to_owned())
            .collect()
    }

    #[test]
    fn subscribes_to_headers_and_new_scripts_in_batches_and_reports_notifications() {
        let mut watcher = watcher();
        assert!(watcher.watch(&[hash(0)]).is_empty()); // Not connected yet.
        assert_eq!(
            watcher.tick(0),
            vec![Action::Connect {
                url: "wss://a".into()
            }]
        );
        let mut targets: Vec<String> = (0..150).map(hash).collect();
        targets.push("not a script hash".into());
        watcher.watch(&targets);
        let opened_actions = watcher.opened(1);
        // Once subscribed, one refresh covers anything between the app's sync and now.
        assert!(opened_actions.contains(&Action::Changed {
            reason: Change::Subscribed
        }));
        let opened = sent(&opened_actions);
        let all = methods(&opened);
        assert_eq!(all[..2], ["server.version", "blockchain.headers.subscribe"]);
        assert_eq!(
            all.iter()
                .filter(|m| *m == "blockchain.scripthash.subscribe")
                .count(),
            150
        );
        assert_eq!(opened.len(), 3); // Handshake, then two batches of at most 100.
        // A duplicate open report while already live is ignored.
        assert!(watcher.opened(1).is_empty());
        // Only new scripts are subscribed later.
        let later = sent(&watcher.watch(&[hash(0), hash(150)]));
        assert_eq!(later.len(), 1);
        assert_eq!(later[0]["params"], json!([hash(150)]));
        // A frame with a block and two address notifications is one refresh per reason.
        let notifications = format!(
            "{}\n{}\n{}\n",
            json!({"method":"blockchain.headers.subscribe","params":[{}]}),
            json!({"method":"blockchain.scripthash.subscribe","params":[hash(3),"s"]}),
            json!({"method":"blockchain.scripthash.subscribe","params":[hash(4),"s"]}),
        );
        assert_eq!(
            watcher.received(&notifications, 2),
            vec![
                Action::Changed {
                    reason: Change::Block
                },
                Action::Changed {
                    reason: Change::Address
                }
            ]
        );
        // Replies to our own requests are not changes.
        assert!(watcher.received(r#"{"id":1,"result":null}"#, 2).is_empty());
    }

    #[test]
    fn a_lost_subscription_refreshes_on_a_timer_rotates_servers_and_resubscribes() {
        let mut watcher = watcher();
        watcher.watch(&[hash(1)]);
        watcher.tick(0);
        watcher.opened(0);
        assert_eq!(
            watcher.closed(5),
            vec![Action::Changed {
                reason: Change::Poll
            }]
        );
        assert!(watcher.tick(5 + RETRY_MS - 1).is_empty());
        // Unreachable server: timed refreshes continue while retrying the other server.
        let retry = watcher.tick(5 + RETRY_MS);
        assert!(retry.contains(&Action::Connect {
            url: "wss://b".into()
        }));
        assert!(retry.contains(&Action::Changed {
            reason: Change::Poll
        }));
        let timeout = watcher.tick(5 + RETRY_MS + CONNECT_TIMEOUT_MS);
        assert_eq!(timeout, vec![Action::Close]);
        let reconnect = watcher.tick(5 + 2 * RETRY_MS + CONNECT_TIMEOUT_MS);
        assert!(reconnect.contains(&Action::Connect {
            url: "wss://a".into()
        }));
        let opened = watcher.opened(6 + 2 * RETRY_MS + CONNECT_TIMEOUT_MS);
        assert!(opened.contains(&Action::Changed {
            reason: Change::Subscribed
        }));
        assert!(methods(&sent(&opened)).contains(&"blockchain.scripthash.subscribe".to_owned()));
    }

    #[test]
    fn pings_idle_sessions_and_drops_a_corrupt_connection() {
        let mut watcher = watcher();
        watcher.tick(0);
        watcher.opened(0);
        assert!(watcher.tick(PING_MS - 1).is_empty());
        assert_eq!(methods(&sent(&watcher.tick(PING_MS))), ["server.ping"]);
        let corrupt = watcher.received("not json\n", PING_MS);
        assert_eq!(corrupt[0], Action::Close);
        assert!(corrupt.contains(&Action::Changed {
            reason: Change::Poll
        }));
    }

    #[test]
    fn regtest_refreshes_on_new_activity_but_waits_for_mining_jobs() {
        let at = |tip: &str, sequence, mining| RegtestActivity {
            tip: tip.into(),
            mempool_sequence: sequence,
            mining,
        };
        let mut tracker = ActivityTracker::default();
        // The baseline refreshes once; after that only real changes do.
        assert_eq!(tracker.observe(&at("a", 1, false)).len(), 1);
        assert!(tracker.observe(&at("a", 1, false)).is_empty());
        assert_eq!(tracker.observe(&at("a", 2, false)).len(), 1);
        assert_eq!(tracker.observe(&at("b", 2, false)).len(), 1);
        assert!(tracker.observe(&at("c", 2, true)).is_empty());
        assert!(tracker.observe(&at("d", 2, true)).is_empty());
        assert_eq!(
            tracker.observe(&at("d", 2, false)),
            vec![Action::Changed {
                reason: Change::Activity
            }]
        );
    }
}
