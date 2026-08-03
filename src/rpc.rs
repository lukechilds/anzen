use crate::state::VaultConfig;
use anyhow::{Context, Result, bail};
use bitcoin::{Address, Amount, BlockHash, Network, OutPoint, TxOut};
use bitcoincore_rpc::{
    Client, RpcApi,
    json::{GetBlockchainInfoResult, ScanTxOutRequest},
};
use serde::{Deserialize, Serialize};
use std::time::Duration;

#[derive(Debug, Clone)]
pub struct RpcConfig {
    pub url: String,
    pub user: String,
    pub password: String,
}

pub struct RegtestRpc {
    pub client: Client,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VaultUtxo {
    pub outpoint: OutPoint,
    pub txout: TxOut,
    pub confirmation_height: u64,
}

impl RegtestRpc {
    pub fn connect(config: &RpcConfig) -> Result<Self> {
        // Real CSV recovery tests intentionally mine tens of thousands of blocks. Core can
        // legitimately spend more than the jsonrpc crate's 15-second default on that request.
        let transport = bitcoincore_rpc::jsonrpc::simple_http::Builder::new()
            .url(&config.url)
            .with_context(|| format!("invalid Bitcoin Core RPC URL {}", config.url))?
            .auth(config.user.clone(), Some(config.password.clone()))
            .timeout(Duration::from_secs(300))
            .build();
        let client = Client::from_jsonrpc(
            bitcoincore_rpc::jsonrpc::client::Client::with_transport(transport),
        );
        let rpc = Self { client };
        rpc.chain_info()?;
        Ok(rpc)
    }

    pub fn chain_info(&self) -> Result<GetBlockchainInfoResult> {
        let info = self
            .client
            .get_blockchain_info()
            .context("Bitcoin Core getblockchaininfo failed")?;
        if info.chain != Network::Regtest {
            bail!("refusing non-regtest Bitcoin Core network: {}", info.chain);
        }
        Ok(info)
    }

    pub fn set_mock_time(&self, timestamp: u64) -> Result<()> {
        let _: serde_json::Value = self
            .client
            .call("setmocktime", &[serde_json::json!(timestamp)])
            .context("Bitcoin Core setmocktime failed")?;
        Ok(())
    }

    pub fn mine(&self, blocks: u64, address: &Address) -> Result<Vec<BlockHash>> {
        self.client
            .generate_to_address(blocks, address)
            .context("Bitcoin Core generatetoaddress failed")
    }

    pub fn scan_vault(&self, config: &VaultConfig) -> Result<Vec<VaultUtxo>> {
        let request = ScanTxOutRequest::Single(format!("addr({})", config.vault_address));
        let result = self
            .client
            .scan_tx_out_set_blocking(&[request])
            .context("Bitcoin Core scantxoutset failed")?;
        if result.success != Some(true) {
            bail!("Bitcoin Core did not complete the vault UTXO scan");
        }
        let mut utxos = result
            .unspents
            .into_iter()
            .map(|utxo| VaultUtxo {
                outpoint: OutPoint::new(utxo.txid, utxo.vout),
                txout: TxOut {
                    value: utxo.amount,
                    script_pubkey: utxo.script_pub_key,
                },
                confirmation_height: utxo.height,
            })
            .collect::<Vec<_>>();
        utxos.sort_by_key(|utxo| (utxo.confirmation_height, utxo.outpoint));
        Ok(utxos)
    }

    pub fn vault_balance(&self, config: &VaultConfig) -> Result<Amount> {
        let sats = self
            .scan_vault(config)?
            .into_iter()
            .map(|utxo| utxo.txout.value.to_sat())
            .sum();
        Ok(Amount::from_sat(sats))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rpc_config_keeps_explicit_connection_values() {
        let config = RpcConfig {
            url: "http://bitcoind:18443".to_owned(),
            user: "vault".to_owned(),
            password: "secret".to_owned(),
        };
        assert_eq!(config.url, "http://bitcoind:18443");
        assert_eq!(config.user, "vault");
        assert_eq!(config.password, "secret");
    }
}
