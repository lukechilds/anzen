//! A stateless public-data proxy for browser-local wallets. It has no device directories,
//! private keys, signing operations or application commands.

use super::*;
use anzen_wallet::{
    HotWallet,
    api::{Blockchain, VaultConfig, VaultUtxo},
    chain_transport::PublicWalletState,
};
use bitcoin::Transaction;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Request {
    network: Network,
    request: Operation,
}

#[derive(Deserialize)]
#[serde(tag = "method", rename_all = "snake_case", deny_unknown_fields)]
enum Operation {
    Tip {},
    MedianTimePast { height: u64 },
    ScanVault { config: VaultConfig },
    ScanConnectors { config: VaultConfig },
    PreviousTransaction { utxo: VaultUtxo },
    Broadcast { transaction: Transaction },
    // Cheap change detection for regtest wallets that poll instead of subscribing.
    Activity {},
    // Deserialize the BDK state separately with serde_json. Serde's internally tagged
    // enum buffer cannot decode JSON object keys into BDK's numeric checkpoint keys.
    Sync { state: Value },
}

pub async fn control(
    State(host): State<Shared>,
    headers: HeaderMap,
    Json(request): Json<Request>,
) -> ApiResult {
    require_admin(&host, &headers).map_err(failure)?;
    // Browser wallets reach Mutinynet and mainnet over Electrum directly; this relay is regtest.
    if request.network != Network::Regtest {
        return Err(failure("the playground host relays regtest only"));
    }
    let result = tokio::task::spawn_blocking(move || -> Result<Value> {
        if let Operation::Activity {} = request.request {
            return activity(&host, request.network);
        }
        let backend = BitcoinCoreBackend::connect(&host.rpc, request.network)?;
        match request.request {
            Operation::Tip {} => Ok(serde_json::to_value(backend.chain_tip()?)?),
            Operation::MedianTimePast { height } => {
                Ok(serde_json::to_value(backend.median_time_past(height)?)?)
            }
            Operation::ScanVault { config } => {
                config.validate_public_policy()?;
                let _guard = host
                    .node_gate
                    .lock()
                    .map_err(|_| anyhow::anyhow!("node scan interrupted"))?;
                Ok(serde_json::to_value(backend.scan_vault(&config)?)?)
            }
            Operation::ScanConnectors { config } => {
                config.validate_public_policy()?;
                let _guard = host
                    .node_gate
                    .lock()
                    .map_err(|_| anyhow::anyhow!("node scan interrupted"))?;
                Ok(serde_json::to_value(backend.scan_connectors(&config)?)?)
            }
            Operation::PreviousTransaction { utxo } => {
                Ok(serde_json::to_value(backend.previous_transaction(&utxo)?)?)
            }
            Operation::Broadcast { transaction } => {
                Ok(serde_json::to_value(backend.broadcast(&transaction)?)?)
            }
            Operation::Activity {} => unreachable!("answered before connecting a wallet backend"),
            Operation::Sync { state } => {
                let state: PublicWalletState = serde_json::from_value(state)?;
                if state.network()? != request.network {
                    bail!("public wallet network mismatch");
                }
                let previous_checkpoints =
                    state.changeset.local_chain.blocks.keys().copied().collect();
                let mut wallet = HotWallet::from_public_state(state)?;
                backend.sync_hot_wallet(&mut wallet)?;
                Ok(serde_json::to_value(
                    wallet.public_chain_update(&previous_checkpoints),
                )?)
            }
        }
    })
    .await
    .map_err(failure)?
    .map_err(failure)?;
    Ok(Json(result))
}

/// The best block and Bitcoin Core's mempool sequence, which changes on every mempool
/// addition or removal. Deliberately not behind the node gate: it must stay responsive
/// while another node operation holds it.
fn activity(host: &Host, network: Network) -> Result<Value> {
    use bitcoincore_rpc::RpcApi;
    if network != Network::Regtest {
        bail!("activity polling is available only on regtest");
    }
    let backend = BitcoinCoreBackend::connect(&host.rpc, network)?;
    let tip = backend.client.get_best_block_hash()?;
    let mempool: Value = backend
        .client
        .call("getrawmempool", &[json!(false), json!(true)])?;
    Ok(json!({
        "tip": tip,
        "mempool_sequence": mempool["mempool_sequence"],
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use bdk_wallet::ChangeSet;
    use bitcoin::blockdata::constants::genesis_block;

    #[test]
    fn sync_envelope_preserves_numeric_bdk_checkpoint_keys() {
        let mut changeset = ChangeSet {
            network: Some(Network::Regtest),
            ..Default::default()
        };
        changeset
            .local_chain
            .blocks
            .insert(0, Some(genesis_block(Network::Regtest).block_hash()));
        let value = json!({"network":"regtest", "request":{"method":"sync", "state":PublicWalletState { changeset, discovered:false, fresh:false }}});
        let request: Request = serde_json::from_str(&value.to_string()).unwrap();
        let Operation::Sync { state } = request.request else {
            panic!("wrong method");
        };
        let state: PublicWalletState = serde_json::from_value(state).unwrap();
        assert_eq!(
            state.changeset.local_chain.blocks[&0],
            Some(genesis_block(Network::Regtest).block_hash())
        );
    }
}
