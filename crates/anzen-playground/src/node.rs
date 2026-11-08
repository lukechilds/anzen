//! Regtest test controls, outside both device roles. No caller can select a mainnet endpoint.
use super::*;
use anzen_wallet::api::Blockchain;
use bitcoin::{Address, Amount, key::Secp256k1};
use bitcoincore_rpc::RpcApi;
use std::str::FromStr;

#[derive(Deserialize)]
#[serde(tag = "method", rename_all = "snake_case", deny_unknown_fields)]
pub enum NodeRequest {
    Status,
    Mine { blocks: u64 },
    AdvanceTime { seconds: u64 },
    Fund { address: String, amount_sats: u64 },
}

pub async fn control(
    State(host): State<Shared>,
    headers: HeaderMap,
    Json(request): Json<NodeRequest>,
) -> ApiResult {
    require_admin(&host, &headers).map_err(failure)?;
    let result = tokio::task::spawn_blocking(move || -> Result<Value> {
        let _guard = host
            .node_gate
            .lock()
            .map_err(|_| anyhow::anyhow!("node operation interrupted"))?;
        let backend = BitcoinCoreBackend::connect(&host.rpc, Network::Regtest)?;
        match request {
            NodeRequest::Status => {}
            NodeRequest::Mine { blocks } => {
                if !(1..=5000).contains(&blocks) {
                    bail!("mine between 1 and 5000 blocks per request");
                }
                backend.mine(blocks, &mining_address())?;
            }
            NodeRequest::AdvanceTime { seconds } => {
                // Up to 400 days: far enough to open the signer's ~388-day time lock.
                if seconds == 0 || seconds > 400 * 24 * 60 * 60 {
                    bail!("advance between one second and 400 days");
                }
                let block = backend
                    .client
                    .get_block_header_info(&backend.client.get_best_block_hash()?)?;
                let now = u64::try_from(chrono::Utc::now().timestamp())?;
                let next = u64::try_from(block.time)?
                    .max(now)
                    .checked_add(seconds)
                    .context("mock time overflow")?;
                backend.set_mock_time(next)?;
                // Eleven new blocks move median-time-past too; changing wall time alone is insufficient.
                backend.mine(11, &mining_address())?;
            }
            NodeRequest::Fund {
                address,
                amount_sats,
            } => {
                let destination = Address::from_str(&address)?.require_network(Network::Regtest)?;
                if amount_sats == 0 || amount_sats > 1_000_000_000 {
                    bail!("fund between 1 sat and 10 test BTC per request");
                }
                let loaded = backend.client.list_wallets()?;
                if !loaded.iter().any(|name| name == "anzen-playground-faucet")
                    && backend
                        .client
                        .load_wallet("anzen-playground-faucet")
                        .is_err()
                {
                    backend.client.create_wallet(
                        "anzen-playground-faucet",
                        None,
                        None,
                        None,
                        None,
                    )?;
                }
                // Use the same timeout/network checks as the node client. The default
                // wallet RPC client times out after 15 seconds during initial mining.
                let rpc = BitcoinCoreBackend::connect(
                    &RpcConfig {
                        url: format!(
                            "{}/wallet/anzen-playground-faucet",
                            host.rpc.url.trim_end_matches('/')
                        ),
                        ..host.rpc.clone()
                    },
                    Network::Regtest,
                )?
                .client;
                let miner = rpc
                    .get_new_address(None, None)?
                    .require_network(Network::Regtest)?;
                // Mine to a separate faucet, then pay an ordinary output to the chosen device.
                // The wallet never needs to receive enormous or immature coinbase outputs.
                // Reuse mature faucet funds. Re-mining 110 blocks for every payment is slow
                // and eventually cannot replenish anything after regtest's rapid halvings.
                if rpc.get_balance(None, None)?.to_sat() < amount_sats.saturating_add(100_000) {
                    rpc.generate_to_address(110, &miner)
                        .context("initial faucet mining failed")?;
                }
                let txid = rpc
                    .send_to_address(
                        &destination,
                        Amount::from_sat(amount_sats),
                        None,
                        None,
                        None,
                        None,
                        None,
                        None,
                    )
                    .context("faucet payment failed; inspect the destination before retrying")?;
                rpc.generate_to_address(1, &miner).with_context(|| {
                    format!("faucet payment {txid} was sent but confirmation mining failed")
                })?;
                return Ok(json!({ "txid": txid, "amount_sats": amount_sats }));
            }
        }
        let tip = backend.chain_tip()?;
        Ok(json!({ "height": tip.height, "median_time": tip.median_time, "network": "regtest" }))
    })
    .await
    .map_err(failure)?
    .map_err(failure)?;
    Ok(Json(result))
}

pub(super) fn mining_address() -> Address {
    let secp = Secp256k1::new();
    let key = bitcoin::secp256k1::SecretKey::from_slice(&[1; 32]).expect("constant valid key");
    let pair = bitcoin::secp256k1::Keypair::from_secret_key(&secp, &key);
    Address::p2tr(&secp, pair.x_only_public_key().0, None, Network::Regtest)
}
