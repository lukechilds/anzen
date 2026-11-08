//! Chain access for hosts. A backend reports public chain data and broadcasts signed
//! transactions; it cannot change what a device signs. Signers never link a backend.
//! Bitcoin Core RPC lives here; Electrum over WebSocket is in `anzen-wallet`.

#[cfg(feature = "native")]
use anyhow::Context;
use anyhow::{Result, bail};
#[cfg(feature = "native")]
use anzen_common::policy::ControllerPolicy;
pub use anzen_common::{storage::VaultConfig, types::VaultUtxo};
#[cfg(feature = "native")]
use bitcoin::{Address, Amount, OutPoint, TxOut};
use bitcoin::{BlockHash, Network, Transaction};
#[cfg(feature = "native")]
use bitcoincore_rpc::{
    Client, RpcApi,
    json::{GetBlockchainInfoResult, ScanTxOutRequest},
};
#[cfg(feature = "native")]
use std::str::FromStr;
#[cfg(feature = "native")]
use std::time::Duration;

#[derive(Debug, Clone)]
pub struct RpcConfig {
    pub url: String,
    pub user: String,
    pub password: String,
}

#[cfg(feature = "native")]
pub struct BitcoinCoreBackend {
    pub client: Client,
    network: Network,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ChainTip {
    pub network: Network,
    pub height: u64,
    pub median_time: u64,
    pub best_block_hash: BlockHash,
}

pub trait Blockchain {
    fn network(&self) -> Network;
    fn backend_description(&self) -> String;
    fn chain_tip(&self) -> Result<ChainTip>;
    /// Median time past of the block at `height`. A time-based relative lock on a coin
    /// confirmed at height `h` counts from the median time past at `h - 1` (BIP68).
    fn median_time_past(&self, height: u64) -> Result<u64>;
    fn scan_vault(&self, config: &VaultConfig) -> Result<Vec<VaultUtxo>>;
    fn scan_connectors(&self, config: &VaultConfig) -> Result<Vec<VaultUtxo>>;
    /// Full previous transaction for an offline signer to authenticate input amounts/scripts.
    fn previous_transaction(&self, _utxo: &VaultUtxo) -> Result<Transaction> {
        bail!("this backend does not provide previous transactions for offline review")
    }
    fn broadcast(&self, transaction: &Transaction) -> Result<bitcoin::Txid>;
}

#[cfg(feature = "native")]
impl Blockchain for BitcoinCoreBackend {
    fn network(&self) -> Network {
        self.network
    }

    fn backend_description(&self) -> String {
        "Bitcoin Core RPC".to_owned()
    }

    fn chain_tip(&self) -> Result<ChainTip> {
        let info = self.chain_info()?;
        Ok(ChainTip {
            network: info.chain,
            height: info.blocks,
            median_time: info.median_time,
            best_block_hash: info.best_block_hash,
        })
    }

    fn median_time_past(&self, height: u64) -> Result<u64> {
        let hash = self.client.get_block_hash(height)?;
        let header = self
            .client
            .get_block_header_info(&hash)
            .context("Bitcoin Core getblockheader failed")?;
        Ok(header
            .median_time
            .context("Bitcoin Core omitted the block's median time")? as u64)
    }

    fn scan_vault(&self, config: &VaultConfig) -> Result<Vec<VaultUtxo>> {
        BitcoinCoreBackend::scan_vault(self, config)
    }

    fn scan_connectors(&self, config: &VaultConfig) -> Result<Vec<VaultUtxo>> {
        BitcoinCoreBackend::scan_connectors(self, config)
    }

    fn previous_transaction(&self, utxo: &VaultUtxo) -> Result<Transaction> {
        // The scan supplies a confirmed height: specifying its block avoids requiring txindex.
        let block = self.client.get_block_hash(utxo.confirmation_height)?;
        self.client
            .get_raw_transaction(&utxo.outpoint.txid, Some(&block))
            .context("Bitcoin Core could not provide the previous transaction")
    }

    fn broadcast(&self, transaction: &Transaction) -> Result<bitcoin::Txid> {
        match self.client.send_raw_transaction(transaction) {
            // A retry after a successful broadcast/failed local write may already be confirmed.
            Err(bitcoincore_rpc::Error::JsonRpc(bitcoincore_rpc::jsonrpc::error::Error::Rpc(
                error,
            ))) if error.code == -27 => Ok(transaction.compute_txid()),
            result => result.context("Bitcoin Core transaction broadcast failed"),
        }
    }
}

#[cfg(feature = "native")]
impl BitcoinCoreBackend {
    pub fn connect(config: &RpcConfig, expected_network: Network) -> Result<Self> {
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
        let rpc = Self {
            client,
            network: expected_network,
        };
        rpc.chain_info()?;
        Ok(rpc)
    }

    pub fn chain_info(&self) -> Result<GetBlockchainInfoResult> {
        let info = self
            .client
            .get_blockchain_info()
            .context("Bitcoin Core getblockchaininfo failed")?;
        if info.chain != self.network {
            bail!(
                "Bitcoin Core network {} does not match requested network {}",
                info.chain,
                self.network
            );
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
        if config.bitcoin_network()? != self.network {
            bail!(
                "refusing to scan a {} vault with {} Bitcoin Core RPC",
                config.network,
                self.network
            );
        }
        self.scan_address(&config.vault_address, "vault")
    }

    pub fn scan_connectors(&self, config: &VaultConfig) -> Result<Vec<VaultUtxo>> {
        if config.bitcoin_network()? != self.network {
            bail!(
                "refusing to scan a {} vault with {} Bitcoin Core RPC",
                config.network,
                self.network
            );
        }
        let controller = controller_policy(config)?;
        self.scan_address(&controller.address.to_string(), "controller")
    }

    fn scan_address(&self, address: &str, label: &str) -> Result<Vec<VaultUtxo>> {
        let request = ScanTxOutRequest::Single(format!("addr({address})"));
        let result = self
            .client
            .scan_tx_out_set_blocking(&[request])
            .with_context(|| format!("Bitcoin Core {label} scantxoutset failed"))?;
        if result.success != Some(true) {
            bail!("Bitcoin Core did not complete the {label} UTXO scan");
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

#[cfg(feature = "native")]
fn controller_policy(config: &VaultConfig) -> Result<ControllerPolicy> {
    let phone = bitcoin::secp256k1::XOnlyPublicKey::from_str(&config.phone_vault_pubkey)
        .context("invalid configured phone vault key")?;
    let hww = bitcoin::secp256k1::XOnlyPublicKey::from_str(&config.hww_vault_pubkey)
        .context("invalid configured HWW vault key")?;
    ControllerPolicy::new_for_network(phone, hww, config.bitcoin_network()?)
}

#[cfg(all(test, feature = "native"))]
mod tests {
    use super::*;

    #[test]
    fn rpc_config_keeps_explicit_connection_values() {
        let config = RpcConfig {
            url: "http://bitcoind:18443".to_owned(),
            user: "anzen".to_owned(),
            password: "secret".to_owned(),
        };
        assert_eq!(config.url, "http://bitcoind:18443");
        assert_eq!(config.user, "anzen");
        assert_eq!(config.password, "secret");
    }
}
