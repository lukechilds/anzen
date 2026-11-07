use super::*;
use anzen_common::keys::DeviceKeys;
use bdk_electrum::electrum_client::ToElectrumScriptHash;
use bdk_wallet::KeychainKind;
use bitcoin::{
    Amount, TxOut,
    consensus::encode::{deserialize_hex, serialize_hex},
    key::Secp256k1,
};
use serde_json::json;
use std::{
    collections::VecDeque,
    sync::{Arc, Mutex},
};

type Handler = Box<dyn FnMut(&Value) -> std::result::Result<Value, String> + Send>;

/// A WebSocket that answers each Electrum call with `handler`, as an ElectrumX server would:
/// a batch is answered with one array frame. A handler error drops the connection.
struct FakeSocket {
    handler: Handler,
    replies: VecDeque<String>,
}

impl FrameTransport for FakeSocket {
    fn send(&mut self, frame: &str) -> Result<()> {
        let payload: Value = serde_json::from_str(frame)?;
        let calls = match &payload {
            Value::Array(calls) => calls.clone(),
            call => vec![call.clone()],
        };
        let mut replies = Vec::new();
        for call in &calls {
            let mut reply = (self.handler)(call).map_err(|error| anyhow::anyhow!(error))?;
            reply["id"] = call["id"].clone();
            replies.push(reply);
        }
        self.replies.push_back(if payload.is_array() {
            Value::Array(replies).to_string()
        } else {
            replies.remove(0).to_string()
        });
        Ok(())
    }
    fn receive(&mut self) -> Result<String> {
        self.replies
            .pop_front()
            .ok_or_else(|| anyhow::anyhow!("Electrum request timed out"))
    }
}

type Make = dyn Fn(usize) -> Result<Handler> + Send + Sync;

/// Opens a fake socket per connection; `make` receives the server's position in the list.
struct FakeConnector {
    servers: Vec<String>,
    make: Box<Make>,
    opened: Arc<Mutex<Vec<usize>>>,
}

impl Connector for FakeConnector {
    fn connect(&self, url: &str) -> Result<Box<dyn FrameTransport>> {
        let index = self.servers.iter().position(|s| s == url).unwrap();
        self.opened.lock().unwrap().push(index);
        Ok(Box::new(FakeSocket {
            handler: (self.make)(index)?,
            replies: VecDeque::new(),
        }))
    }
}

fn backend(
    network: Network,
    make: impl Fn(usize) -> Result<Handler> + Send + Sync + 'static,
) -> (ElectrumBackend<FakeConnector>, Arc<Mutex<Vec<usize>>>) {
    let servers: Vec<String> = default_servers(network)
        .unwrap()
        .iter()
        .map(|s| (*s).to_owned())
        .collect();
    let opened = Arc::new(Mutex::new(Vec::new()));
    let connector = FakeConnector {
        servers: servers.clone(),
        make: Box::new(make),
        opened: opened.clone(),
    };
    // Explicit servers keep a fixed order, unlike the process-wide preference.
    (
        ElectrumBackend::with_servers(network, servers, connector).unwrap(),
        opened,
    )
}

fn handler(
    f: impl FnMut(&Value) -> std::result::Result<Value, String> + Send + 'static,
) -> Handler {
    Box::new(f)
}

fn network_headers(network: &str) -> Vec<bitcoin::block::Header> {
    let fixture: Value =
        serde_json::from_str(include_str!("../../../../test-vectors/chain-headers.json")).unwrap();
    fixture[network]["headers"]
        .as_array()
        .unwrap()
        .iter()
        .rev()
        .map(|h| deserialize_hex(h.as_str().unwrap()).unwrap())
        .collect()
}

fn headers() -> Vec<bitcoin::block::Header> {
    network_headers("signet")
}

fn answer(call: &Value, headers: &[bitcoin::block::Header]) -> Value {
    let p = &call["params"];
    let result = match call["method"].as_str().unwrap() {
        "server.version" => json!(["test-electrum", "1.4"]),
        "blockchain.block.header" => {
            json!(serialize_hex(&headers[p[0].as_u64().unwrap() as usize]))
        }
        "blockchain.headers.subscribe" => {
            json!({"height":headers.len()-1, "hex":serialize_hex(headers.last().unwrap())})
        }
        "blockchain.block.headers" => {
            let hs: Vec<_> = headers
                .iter()
                .skip(p[0].as_u64().unwrap() as usize)
                .take(p[1].as_u64().unwrap() as usize)
                .collect();
            json!({"count":hs.len(), "hex":hs.iter().map(|h| serialize_hex(*h)).collect::<String>(), "max":2016})
        }
        "blockchain.scripthash.get_history" | "blockchain.scripthash.listunspent" => json!([]),
        method => panic!("unexpected method {method}"),
    };
    json!({"result":result})
}

fn wallet() -> HotWallet {
    let keys = DeviceKeys::generate_for_network(&Secp256k1::new(), Network::Signet).unwrap();
    HotWallet::ephemeral(&keys).unwrap()
}

#[test]
fn frames_carry_single_requests_and_batches_and_decode_every_server_style() {
    assert_eq!(encode_frame(b"{\"id\":1}\n").unwrap(), "{\"id\":1}\n");
    assert_eq!(
        encode_frame(b"{\"id\":1}\n{\"id\":2}\n").unwrap(),
        "[{\"id\":1},{\"id\":2}]\n"
    );
    assert!(encode_frame(b"\n").is_err());

    let mut decoder = FrameDecoder::default();
    // A batch reply as one array, with Electrum's permitted `"error": null`.
    let batch = decoder
        .push(r#"[{"id":2,"result":1,"error":null},{"id":1,"result":2}]"#)
        .unwrap();
    assert_eq!(
        batch,
        vec![json!({"id":2,"result":1}), json!({"id":1,"result":2})]
    );
    // Newline-delimited messages, including a notification.
    let lines = decoder
        .push("{\"method\":\"blockchain.headers.subscribe\",\"params\":[]}\n{\"id\":3,\"result\":[]}\n")
        .unwrap();
    assert_eq!(lines.len(), 2);
    // One message split across frames.
    assert!(decoder.push("{\"id\":4,\"res").unwrap().is_empty());
    assert_eq!(
        decoder.push("ult\":5}").unwrap(),
        vec![json!({"id":4,"result":5})]
    );
    assert!(FrameDecoder::default().push("not json\n").is_err());
    assert!(FrameDecoder::default().push("[1]").is_err());
    assert!(
        FrameDecoder::default()
            .push(&"x".repeat(MAX_FRAME_BYTES + 1))
            .is_err()
    );
}

#[test]
fn network_guards_and_failover_are_bounded_and_recompute_mtp() {
    assert!(default_servers(Network::Regtest).is_err());
    let (electrum, opened) = backend(Network::Signet, |i| {
        if i == 0 {
            bail!("offline");
        }
        Ok(handler(|c| Ok(answer(c, &headers()))))
    });
    let tip = electrum.chain_tip().unwrap();
    assert_eq!(*opened.lock().unwrap(), vec![0, 1]);
    assert_eq!(tip.median_time, u64::from(headers()[1].time));
    // The working connection is reused for the rest of the command.
    electrum.chain_tip().unwrap();
    assert_eq!(*opened.lock().unwrap(), vec![0, 1]);

    let (electrum, _) = backend(Network::Signet, |_| {
        Ok(handler(|c| {
            let mut hs = headers();
            hs[1].nonce += 1;
            Ok(answer(c, &hs))
        }))
    });
    let error = electrum.chain_tip().unwrap_err().to_string();
    assert!(error.contains("All 2 Mutinynet Electrum servers failed"));
    assert!(error.contains("not on Mutinynet"));
}

#[test]
fn mainnet_uses_its_single_server_and_checks_the_mainnet_genesis() {
    let (electrum, opened) = backend(Network::Bitcoin, |_| {
        Ok(handler(|c| Ok(answer(c, &network_headers("bitcoin")))))
    });
    let tip = electrum.chain_tip().unwrap();
    assert_eq!(*opened.lock().unwrap(), vec![0]);
    assert_eq!((tip.network, tip.height), (Network::Bitcoin, 1));
    // A Mutinynet server offered as the mainnet server is refused by its genesis block.
    let (electrum, _) = backend(Network::Bitcoin, |_| {
        Ok(handler(|c| Ok(answer(c, &headers()))))
    });
    let error = electrum.chain_tip().unwrap_err().to_string();
    assert!(error.contains("The mainnet Electrum server failed"));
    assert!(error.contains("genesis does not match"));
}

#[test]
fn partial_discovery_is_discarded_and_restarted_on_the_other_server() {
    let counts = Arc::new(Mutex::new([0, 0]));
    let shared = counts.clone();
    let (electrum, _) = backend(Network::Signet, move |attempt| {
        let counts = shared.clone();
        Ok(handler(move |call| {
            if call["method"] == "blockchain.scripthash.get_history" {
                let mut counts = counts.lock().unwrap();
                counts[attempt] += 1;
                if attempt == 0 && counts[0] == 45 {
                    return Err("connection lost".into());
                }
            }
            Ok(answer(call, &headers()))
        }))
    });
    let mut wallet = wallet();
    electrum.sync_hot_wallet(&mut wallet).unwrap();
    assert_eq!(*counts.lock().unwrap(), [45, 200]);
    assert_eq!(wallet.wallet.latest_checkpoint().height(), 1);
    assert_eq!(wallet.wallet.balance().total().to_sat(), 0);
    // Discovery is recorded: the next sync checks revealed addresses only.
    let before = counts.lock().unwrap()[1];
    electrum.sync_hot_wallet(&mut wallet).unwrap();
    assert_eq!(counts.lock().unwrap()[1], before);
}

#[test]
fn discovery_finds_funds_beyond_twenty_addresses_and_scans_verify_utxos() {
    let mut wallet = wallet();
    let address = wallet
        .wallet
        .peek_address(KeychainKind::External, 70)
        .address;
    let script = address.script_pubkey();
    let hash = serde_json::to_value(script.to_electrum_scripthash()).unwrap();
    let mut funding = genesis_block(Network::Signet).txdata[0].clone();
    funding.output = vec![TxOut {
        value: Amount::from_sat(12_345),
        script_pubkey: script,
    }];
    let txid = funding.compute_txid();
    let mut hs = headers();
    let mut next = hs[1];
    next.prev_blockhash = hs[1].block_hash();
    next.time += 30;
    next.merkle_root = txid.to_raw_hash().into();
    hs.push(next);
    let make = move || {
        let (hash, funding, hs) = (hash.clone(), funding.clone(), hs.clone());
        handler(move |call| {
            let result = match call["method"].as_str().unwrap() {
                "blockchain.scripthash.get_history" if call["params"][0] == hash => {
                    json!([{"height":2,"tx_hash":txid}])
                }
                "blockchain.transaction.get" => json!(serialize_hex(&funding)),
                "blockchain.transaction.get_merkle" => {
                    json!({"block_height":2,"pos":0,"merkle":[]})
                }
                "blockchain.scripthash.listunspent" => json!([
                    {"height":2,"tx_hash":txid,"tx_pos":0,"value":12345},
                    {"height":0,"tx_hash":txid,"tx_pos":0,"value":12345}
                ]),
                _ => return Ok(answer(call, &hs)),
            };
            Ok(json!({"result":result}))
        })
    };
    let socket = |handler| {
        RawClient::from(FramedStream::new(Box::new(FakeSocket {
            handler,
            replies: VecDeque::new(),
        })))
    };
    let (electrum, _) = backend(Network::Signet, {
        let make = make.clone();
        move |_| Ok(make())
    });
    electrum.sync_hot_wallet(&mut wallet).unwrap();
    assert_eq!(wallet.wallet.balance().immature.to_sat(), 12_345);
    assert!(wallet.wallet.tx_graph().get_tx(txid).is_some());
    assert_eq!(scan(&socket(make()), &address).unwrap().len(), 1);
    let bad = socket(handler(|call| {
        if call["method"] == "blockchain.scripthash.listunspent" {
            let tx = genesis_block(Network::Signet).txdata[0].clone();
            return Ok(
                json!({"result":[{"height":1,"tx_hash":tx.compute_txid(),"tx_pos":0,"value":1}]}),
            );
        }
        Ok(json!({"result":serialize_hex(&genesis_block(Network::Signet).txdata[0])}))
    }));
    assert!(
        scan(&bad, &address)
            .unwrap_err()
            .to_string()
            .contains("amount or script")
    );
}

#[test]
fn broadcast_lost_reply_is_reconciled_on_fallback_without_another_broadcast() {
    let tx: Transaction = genesis_block(Network::Signet).txdata[0].clone();
    let broadcasts = Arc::new(Mutex::new(0));
    let (electrum, _) = backend(Network::Signet, {
        let (broadcasts, tx) = (broadcasts.clone(), tx.clone());
        move |attempt| {
            let (broadcasts, tx) = (broadcasts.clone(), tx.clone());
            Ok(handler(move |call| {
                match call["method"].as_str().unwrap() {
                    "blockchain.transaction.get" if attempt == 1 => {
                        Ok(json!({"result":serialize_hex(&tx)}))
                    }
                    "blockchain.transaction.get" => {
                        Ok(json!({"error":{"code":-5,"message":"not found"}}))
                    }
                    "blockchain.transaction.broadcast" => {
                        *broadcasts.lock().unwrap() += 1;
                        Err("accepted but reply lost".into())
                    }
                    _ => Ok(answer(call, &headers())),
                }
            }))
        }
    });
    assert_eq!(electrum.broadcast(&tx).unwrap(), tx.compute_txid());
    assert_eq!(*broadcasts.lock().unwrap(), 1);
}

#[test]
fn explicit_broadcast_rejection_does_not_try_another_server() {
    let (electrum, opened) = backend(Network::Signet, |_| {
        Ok(handler(|call| {
            if call["method"] == "blockchain.transaction.get"
                || call["method"] == "blockchain.transaction.broadcast"
            {
                return Ok(json!({"error":{"code":-26,"message":"rejected"}}));
            }
            Ok(answer(call, &headers()))
        }))
    });
    let error = electrum
        .broadcast(&genesis_block(Network::Signet).txdata[0])
        .unwrap_err();
    assert_eq!(*opened.lock().unwrap(), vec![0]);
    assert!(is_broadcast_rejection(&error));
    assert!(error.to_string().contains("rejected"));
}

#[test]
fn handshake_error_before_broadcast_still_allows_failover() {
    let tx = genesis_block(Network::Signet).txdata[0].clone();
    let (electrum, opened) = backend(Network::Signet, {
        let tx = tx.clone();
        move |attempt| {
            let tx = tx.clone();
            Ok(handler(move |call| {
                if attempt == 0 {
                    return Ok(json!({"error":{"code":-32603,"message":"server unavailable"}}));
                }
                if call["method"] == "blockchain.transaction.get" {
                    return Ok(json!({"result":serialize_hex(&tx)}));
                }
                Ok(answer(call, &headers()))
            }))
        }
    });
    assert_eq!(electrum.broadcast(&tx).unwrap(), tx.compute_txid());
    assert_eq!(*opened.lock().unwrap(), vec![0, 1]);
}

#[test]
fn only_websocket_servers_are_accepted() {
    let connector = FakeConnector {
        servers: vec![],
        make: Box::new(|_| bail!("unused")),
        opened: Arc::default(),
    };
    assert!(
        ElectrumBackend::with_servers(Network::Bitcoin, vec!["ssl://host:50002".into()], connector)
            .is_err()
    );
}
