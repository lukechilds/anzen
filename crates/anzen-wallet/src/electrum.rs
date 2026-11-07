//! Electrum over WebSocket: the only public chain transport for Mutinynet and mainnet, on every
//! platform. Everything above the socket lives here: server choice and failover, JSON-RPC
//! framing, chain identity checks, wallet sync, vault scans and broadcasts. A platform supplies
//! only a [`Connector`] that exchanges whole WebSocket text frames with a server URL: JavaScript
//! in the browser, `tungstenite` natively. Regtest uses Bitcoin Core RPC instead.

use super::{
    HotWallet, HotWalletBackend,
    api::{Blockchain, ChainTip, VaultConfig, VaultUtxo},
    chain_transport::{MUTINYNET_BLOCK_ONE, check_config, verify_previous},
};
use anyhow::{Context, Result, bail, ensure};
use anzen_common::policy::ControllerPolicy;
use bdk_electrum::{
    BdkElectrumClient,
    electrum_client::{ElectrumApi, Error, Param, raw_client::RawClient},
};
use bitcoin::{
    Address, BlockHash, Network, OutPoint, Transaction, Txid, blockdata::constants::genesis_block,
};
use serde_json::Value;
use std::{
    io::{self, Read, Write},
    str::FromStr,
    sync::{
        Mutex,
        atomic::{AtomicUsize, Ordering},
    },
};

#[cfg(target_arch = "wasm32")]
pub mod browser;
#[cfg(feature = "native")]
pub mod native;
#[cfg(test)]
mod tests;

/// Two independent Mutinynet servers. A failed operation restarts on the other one.
pub const MUTINYNET_SERVERS: &[&str] = &[
    "wss://mutinynet.com/electrum-websocket",
    "wss://electrum.mutinynet.arkade.sh",
];
/// The only public mainnet Electrum server found that accepts WebSocket connections.
pub const MAINNET_SERVERS: &[&str] = &["wss://electrum.lu.ke"];
/// Frames larger than this are refused in either direction.
pub const MAX_FRAME_BYTES: usize = 8 * 1024 * 1024;

pub fn default_servers(network: Network) -> Result<&'static [&'static str]> {
    match network {
        Network::Signet => Ok(MUTINYNET_SERVERS),
        Network::Bitcoin => Ok(MAINNET_SERVERS),
        _ => bail!("Electrum serves Mutinynet and mainnet; regtest uses Bitcoin Core RPC"),
    }
}

/// One WebSocket connection, exchanging whole text frames.
pub trait FrameTransport: Send {
    fn send(&mut self, frame: &str) -> Result<()>;
    /// The next frame. Fails when the server disconnects or does not answer in time.
    fn receive(&mut self) -> Result<String>;
}

/// Opens WebSocket connections for this platform.
pub trait Connector: Send + Sync {
    fn connect(&self, url: &str) -> Result<Box<dyn FrameTransport>>;
}

/// A server explicitly refused a transaction. That is an answer, not an outage, so it is never
/// retried on another server.
#[derive(Debug)]
pub struct BroadcastRejected(Error);
impl std::fmt::Display for BroadcastRejected {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Electrum rejected the transaction: {}", self.0)
    }
}
impl std::error::Error for BroadcastRejected {}

pub fn is_broadcast_rejection(error: &anyhow::Error) -> bool {
    error.downcast_ref::<BroadcastRejected>().is_some()
}

/// Encodes one flushed write from the Electrum client as a WebSocket frame. The client writes
/// one JSON request per line; several lines are a batch, sent as a JSON-RPC array. The trailing
/// newline also suits servers that bridge WebSocket frames to Electrum's TCP line protocol.
pub fn encode_frame(written: &[u8]) -> Result<String> {
    let text = std::str::from_utf8(written).context("Electrum request is not UTF-8")?;
    let lines: Vec<&str> = text
        .lines()
        .filter(|line| !line.trim().is_empty())
        .collect();
    ensure!(!lines.is_empty(), "empty Electrum request");
    let frame = if lines.len() == 1 {
        format!("{}\n", lines[0])
    } else {
        format!("[{}]\n", lines.join(","))
    };
    ensure!(
        frame.len() <= MAX_FRAME_BYTES,
        "Electrum request exceeds 8 MiB"
    );
    Ok(frame)
}

/// Turns received frames back into the client's one-message-per-line stream. Servers send a
/// whole JSON value per frame, newline-delimited messages, or one message split over frames.
#[derive(Default)]
pub struct FrameDecoder {
    buffer: String,
}

impl FrameDecoder {
    pub fn push(&mut self, frame: &str) -> Result<Vec<Value>> {
        ensure!(
            self.buffer.len() + frame.len() <= MAX_FRAME_BYTES,
            "Electrum response exceeds 8 MiB"
        );
        self.buffer.push_str(frame);
        let mut messages = Vec::new();
        if let Ok(value) = serde_json::from_str::<Value>(&self.buffer) {
            self.buffer.clear();
            flatten(value, &mut messages)?;
            return Ok(messages);
        }
        while let Some(end) = self.buffer.find('\n') {
            let line: String = self.buffer.drain(..=end).collect();
            if !line.trim().is_empty() {
                flatten(
                    serde_json::from_str(&line).context("invalid Electrum message")?,
                    &mut messages,
                )?;
            }
        }
        Ok(messages)
    }
}

fn flatten(value: Value, messages: &mut Vec<Value>) -> Result<()> {
    match value {
        Value::Array(values) => values.into_iter().try_for_each(|v| flatten(v, messages)),
        Value::Object(mut object) => {
            // Electrum permits `"error": null` on success; the client expects it omitted.
            if object.get("error").is_some_and(Value::is_null) {
                object.remove("error");
            }
            messages.push(Value::Object(object));
            Ok(())
        }
        _ => bail!("invalid Electrum message"),
    }
}

/// Presents a frame transport as the byte stream the Electrum client reads and writes.
pub struct FramedStream {
    transport: Box<dyn FrameTransport>,
    decoder: FrameDecoder,
    outgoing: Vec<u8>,
    incoming: io::Cursor<Vec<u8>>,
}

impl FramedStream {
    pub fn new(transport: Box<dyn FrameTransport>) -> Self {
        Self {
            transport,
            decoder: FrameDecoder::default(),
            outgoing: Vec::new(),
            incoming: io::Cursor::new(Vec::new()),
        }
    }
}

impl Read for FramedStream {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        while self.incoming.position() as usize == self.incoming.get_ref().len() {
            let frame = self.transport.receive().map_err(io::Error::other)?;
            let messages = self.decoder.push(&frame).map_err(io::Error::other)?;
            let mut bytes = Vec::new();
            for message in messages {
                serde_json::to_writer(&mut bytes, &message).map_err(io::Error::other)?;
                bytes.push(b'\n');
            }
            self.incoming = io::Cursor::new(bytes);
        }
        self.incoming.read(buf)
    }
}

impl Write for FramedStream {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        if self.outgoing.len() + buf.len() > MAX_FRAME_BYTES {
            return Err(io::Error::other("Electrum request exceeds 8 MiB"));
        }
        self.outgoing.extend_from_slice(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        if self.outgoing.is_empty() {
            return Ok(());
        }
        // A failed send must not leave a broadcast queued for the next write.
        let outgoing = std::mem::take(&mut self.outgoing);
        let frame = encode_frame(&outgoing).map_err(io::Error::other)?;
        self.transport.send(&frame).map_err(io::Error::other)
    }
}

type Client = RawClient<FramedStream>;

// The server that last worked on each network stays first until the process (or browser
// device) restarts. Mutinynet's initial order is random, spreading devices across servers.
static MUTINYNET_PREFERRED: AtomicUsize = AtomicUsize::new(usize::MAX);
static MAINNET_PREFERRED: AtomicUsize = AtomicUsize::new(0);

/// Public chain access for Mutinynet and mainnet over the platform's WebSocket connector.
/// A connection is opened on first use and reused for the backend's lifetime (one command).
pub struct ElectrumBackend<C: Connector> {
    network: Network,
    servers: Vec<String>,
    preferred: Option<&'static AtomicUsize>,
    connector: C,
    session: Mutex<Option<(usize, Client)>>,
}

impl<C: Connector> ElectrumBackend<C> {
    pub fn new(network: Network, connector: C) -> Result<Self> {
        let servers = default_servers(network)?.iter().map(|s| (*s).to_owned());
        let preferred = if network == Network::Signet {
            &MUTINYNET_PREFERRED
        } else {
            &MAINNET_PREFERRED
        };
        let _ = preferred.compare_exchange(
            usize::MAX,
            usize::from(rand::random::<bool>()),
            Ordering::Relaxed,
            Ordering::Relaxed,
        );
        Ok(Self {
            network,
            servers: servers.collect(),
            preferred: Some(preferred),
            connector,
            session: Mutex::new(None),
        })
    }

    /// Explicit servers in the given order, such as a user's own Electrum server.
    pub fn with_servers(network: Network, servers: Vec<String>, connector: C) -> Result<Self> {
        ensure!(!servers.is_empty(), "no Electrum servers are configured");
        for server in &servers {
            ensure!(
                server.starts_with("wss://") || server.starts_with("ws://"),
                "Electrum server {server} must be a WebSocket (wss://) URL"
            );
        }
        Ok(Self {
            network,
            servers,
            preferred: None,
            connector,
            session: Mutex::new(None),
        })
    }

    fn order(&self) -> Vec<usize> {
        let count = self.servers.len();
        let first = self
            .preferred
            .map_or(0, |preferred| preferred.load(Ordering::Relaxed) % count);
        (0..count).map(|i| (first + i) % count).collect()
    }

    fn label(&self) -> String {
        let name = match self.network {
            Network::Signet => "Mutinynet",
            Network::Bitcoin => "mainnet",
            _ => "configured",
        };
        if self.servers.len() == 1 {
            format!("The {name} Electrum server failed")
        } else {
            format!("All {} {name} Electrum servers failed", self.servers.len())
        }
    }

    fn open(&self, index: usize) -> Result<Client> {
        let transport = self.connector.connect(&self.servers[index])?;
        let client = RawClient::from(FramedStream::new(transport));
        client.raw_call(
            "server.version",
            [Param::String("anzen".into()), Param::String("1.4".into())],
        )?;
        ensure!(
            client.block_header(0)?.block_hash() == genesis_block(self.network).block_hash(),
            "Electrum genesis does not match selected network"
        );
        if self.network == Network::Signet {
            ensure!(
                client.block_header(1)?.block_hash() == BlockHash::from_str(MUTINYNET_BLOCK_ONE)?,
                "Electrum is not on Mutinynet (ordinary signet is not supported)"
            );
        }
        Ok(client)
    }

    /// Runs one operation, trying each server once from the preferred one. Every attempt starts
    /// over; a wallet sync applies BDK's update only after the whole scan succeeds.
    fn run<T>(
        &self,
        broadcast: bool,
        mut operation: impl FnMut(&Client) -> Result<T>,
    ) -> Result<T> {
        let mut session = self
            .session
            .lock()
            .map_err(|_| anyhow::anyhow!("Electrum session interrupted"))?;
        let mut errors = Vec::new();
        // The server already connected in this command goes first.
        let mut order = self.order();
        if let Some((open, _)) = session.as_ref() {
            order.retain(|index| index != open);
            order.insert(0, *open);
        }
        for index in order {
            let attempt = (|| -> Result<T> {
                if session.as_ref().map(|(open, _)| *open) != Some(index) {
                    *session = None;
                    *session = Some((index, self.open(index)?));
                }
                operation(&session.as_ref().expect("session was just opened").1)
            })();
            match attempt {
                Ok(value) => {
                    if let Some(preferred) = self.preferred {
                        preferred.store(index, Ordering::Relaxed);
                    }
                    return Ok(value);
                }
                Err(error) if is_broadcast_rejection(&error) => return Err(error),
                Err(error) => {
                    *session = None;
                    errors.push(format!("{}: {error:#}", self.servers[index]));
                }
            }
        }
        let outcome = if broadcast {
            " The broadcast may already have succeeded; retrying uses the same transaction."
        } else {
            ""
        };
        bail!("{}: {}{outcome}", self.label(), errors.join("; "))
    }
}

impl<C: Connector> Blockchain for ElectrumBackend<C> {
    fn network(&self) -> Network {
        self.network
    }

    fn backend_description(&self) -> String {
        format!("Electrum ({})", self.servers[self.order()[0]])
    }

    fn chain_tip(&self) -> Result<ChainTip> {
        let network = self.network;
        self.run(false, |client| {
            let tip = client.block_headers_subscribe()?;
            let height = u32::try_from(tip.height)?;
            let start = height.saturating_sub(10);
            let headers = client.batch_block_header(start..=height)?;
            ensure!(
                headers.len() == (height - start + 1) as usize,
                "missing Electrum tip headers"
            );
            ensure!(
                headers.last() == Some(&tip.header),
                "chain tip changed during query; retry"
            );
            for pair in headers.windows(2) {
                ensure!(
                    pair[1].prev_blockhash == pair[0].block_hash(),
                    "Electrum tip headers are not linked"
                );
            }
            // Median time past from linked headers, not a server-reported timestamp. Consensus
            // still checks every timelocked broadcast.
            let mut times: Vec<_> = headers.iter().map(|h| u64::from(h.time)).collect();
            times.sort_unstable();
            Ok(ChainTip {
                network,
                height: u64::from(height),
                median_time: times[times.len() / 2],
                best_block_hash: tip.header.block_hash(),
            })
        })
    }

    fn scan_vault(&self, config: &VaultConfig) -> Result<Vec<VaultUtxo>> {
        check_config(config, self.network)?;
        let address = Address::from_str(&config.vault_address)?.require_network(self.network)?;
        self.run(false, |client| scan(client, &address))
    }

    fn scan_connectors(&self, config: &VaultConfig) -> Result<Vec<VaultUtxo>> {
        check_config(config, self.network)?;
        let controller = ControllerPolicy::new_for_network(
            config.phone_vault_pubkey.parse()?,
            config.hww_vault_pubkey.parse()?,
            self.network,
        )?;
        self.run(false, |client| scan(client, &controller.address))
    }

    fn previous_transaction(&self, utxo: &VaultUtxo) -> Result<Transaction> {
        self.run(false, |client| {
            let tx = client.transaction_get(&utxo.outpoint.txid)?;
            verify_previous(&tx, utxo)?;
            Ok(tx)
        })
    }

    fn broadcast(&self, transaction: &Transaction) -> Result<Txid> {
        let txid = transaction.compute_txid();
        let known = |client: &Client| {
            client
                .transaction_get(&txid)
                .ok()
                .is_some_and(|tx| tx.compute_txid() == txid)
        };
        self.run(true, |client| {
            // Check first on each server: a previous response may have been lost.
            if known(client) {
                return Ok(txid);
            }
            match client.transaction_broadcast(transaction) {
                Ok(returned) => ensure!(
                    returned == txid,
                    "Electrum returned a different broadcast txid"
                ),
                Err(_) if known(client) => {}
                Err(error @ Error::Protocol(_)) => return Err(BroadcastRejected(error).into()),
                Err(error) => return Err(error.into()),
            }
            Ok(txid)
        })
    }
}

impl<C: Connector> HotWalletBackend for ElectrumBackend<C> {
    fn sync_hot_wallet(&self, wallet: &mut HotWallet) -> Result<()> {
        self.run(false, |client| {
            wallet.sync_electrum(&BdkElectrumClient::new(client))
        })
    }
}

fn scan(client: &Client, address: &Address) -> Result<Vec<VaultUtxo>> {
    let mut result = Vec::new();
    for item in client.script_list_unspent(&address.script_pubkey())? {
        if item.height == 0 {
            continue;
        }
        let tx = client.transaction_get(&item.tx_hash)?;
        let utxo = VaultUtxo {
            outpoint: OutPoint::new(item.tx_hash, u32::try_from(item.tx_pos)?),
            txout: bitcoin::TxOut {
                value: bitcoin::Amount::from_sat(item.value),
                script_pubkey: address.script_pubkey(),
            },
            confirmation_height: u64::try_from(item.height)?,
        };
        verify_previous(&tx, &utxo)?;
        // Bind the claimed confirmation to this server's header, as BDK does for wallet
        // history. This is still an indexer, not full-chain/SPV validation.
        let header = client.block_header(item.height)?;
        let proof = client.transaction_get_merkle(&item.tx_hash, item.height)?;
        ensure!(
            proof.block_height == item.height
                && bdk_electrum::electrum_client::utils::validate_merkle_proof(
                    &item.tx_hash,
                    &header.merkle_root,
                    &proof
                ),
            "Electrum UTXO confirmation proof is invalid"
        );
        result.push(utxo);
    }
    result.sort_by_key(|utxo| (utxo.confirmation_height, utxo.outpoint));
    Ok(result)
}
