use super::{storage::VaultConfig, types::VaultUtxo};
use anyhow::{Context, Result, bail};
use bdk_electrum::{
    BdkElectrumClient,
    electrum_client::{Client as ElectrumClient, ElectrumApi},
};
use bitcoin::{
    Address, Amount, BlockHash, Network, OutPoint, Transaction, TxOut,
    blockdata::constants::genesis_block,
};
use bitcoincore_rpc::{
    Client, RpcApi,
    json::{GetBlockchainInfoResult, ScanTxOutRequest},
};
use std::time::Duration;

pub const MAINNET_ELECTRUM_SERVERS: &[&str] = &[
    "ssl://bitcoin.lu.ke:50002",
    "ssl://electrum.blockstream.info:50002",
    "ssl://electrum.bullbitcoin.com:50002",
    "ssl://electrum.cakewallet.com:50002",
];

pub const TESTNET_ELECTRUM_SERVERS: &[&str] = &[
    "ssl://electrum.blockstream.info:60002",
    "ssl://testnet.aranguren.org:51002",
];

pub const REGTEST_ELECTRUM_SERVERS: &[&str] = &["tcp://127.0.0.1:50001"];

#[derive(Debug, Clone)]
pub struct RpcConfig {
    pub url: String,
    pub user: String,
    pub password: String,
}

pub struct BitcoinCoreBackend {
    pub client: Client,
    network: Network,
}

pub struct ElectrumBackend {
    pub(crate) client: BdkElectrumClient<ElectrumClient>,
    server: String,
    network: Network,
}

#[derive(Debug, Clone)]
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
    fn scan_vault(&self, config: &VaultConfig) -> Result<Vec<VaultUtxo>>;
    fn broadcast(&self, transaction: &Transaction) -> Result<bitcoin::Txid>;
    /// Estimate the fee rate in satoshis per virtual byte that is likely to confirm
    /// within `target_blocks`. Returns a conservative default when the backend cannot
    /// produce a reliable estimate (e.g., regtest).
    fn estimate_fee_rate(&self, target_blocks: u16) -> Result<u64>;
}

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

    fn scan_vault(&self, config: &VaultConfig) -> Result<Vec<VaultUtxo>> {
        BitcoinCoreBackend::scan_vault(self, config)
    }

    fn estimate_fee_rate(&self, target_blocks: u16) -> Result<u64> {
        BitcoinCoreBackend::estimate_fee_rate(self, target_blocks)
    }

    fn broadcast(&self, transaction: &Transaction) -> Result<bitcoin::Txid> {
        self.client
            .send_raw_transaction(transaction)
            .context("Bitcoin Core transaction broadcast failed")
    }
}

impl ElectrumBackend {
    pub fn estimate_fee_rate(&self, target_blocks: u16) -> Result<u64> {
        if self.network == Network::Regtest {
            return Ok(super::DEFAULT_FEE_RATE_SAT_VB);
        }
        match self.client.inner.estimate_fee(target_blocks as usize) {
            Ok(btc_per_kb) => {
                let sat_per_vbyte = (btc_per_kb * 100_000_000.0_f64 / 1_000.0_f64) as u64;
                Ok(sat_per_vbyte.max(1))
            }
            Err(_) => Ok(super::DEFAULT_FEE_RATE_SAT_VB),
        }
    }

    pub fn connect_default(network: Network) -> Result<Self> {
        let servers = match network {
            Network::Bitcoin => MAINNET_ELECTRUM_SERVERS,
            Network::Testnet => TESTNET_ELECTRUM_SERVERS,
            Network::Regtest => REGTEST_ELECTRUM_SERVERS,
            other => bail!("unsupported Electrum network: {other}"),
        };
        Self::connect(network, servers)
    }

    pub fn connect(network: Network, servers: &[&str]) -> Result<Self> {
        if servers.is_empty() {
            bail!("no Electrum servers are configured for {network}");
        }
        let expected_genesis = genesis_block(network).block_hash();
        let mut errors = Vec::new();
        for server in servers {
            let attempt = (|| -> Result<Self> {
                let raw = ElectrumClient::new(server)
                    .with_context(|| format!("invalid Electrum server URL {server}"))?;
                let genesis = raw
                    .block_header(0)
                    .with_context(|| format!("Electrum server {server} did not return genesis"))?
                    .block_hash();
                if genesis != expected_genesis {
                    bail!("Electrum server {server} is not on {network}");
                }
                Ok(Self {
                    client: BdkElectrumClient::new(raw),
                    server: (*server).to_owned(),
                    network,
                })
            })();
            match attempt {
                Ok(backend) => return Ok(backend),
                Err(error) => errors.push(format!("{server}: {error:#}")),
            }
        }
        bail!(
            "failed to connect to any Electrum server for {network}: {}",
            errors.join("; ")
        )
    }
}

impl Blockchain for ElectrumBackend {
    fn network(&self) -> Network {
        self.network
    }

    fn backend_description(&self) -> String {
        format!("Electrum ({})", self.server)
    }

    fn chain_tip(&self) -> Result<ChainTip> {
        let tip = self
            .client
            .inner
            .block_headers_subscribe()
            .context("Electrum header subscription failed")?;
        let first_height = tip.height.saturating_sub(10);
        let heights = (first_height..=tip.height)
            .map(|height| height as u32)
            .collect::<Vec<_>>();
        let mut times = self
            .client
            .inner
            .batch_block_header(heights.iter())
            .context("Electrum median-time-past header query failed")?
            .into_iter()
            .map(|header| u64::from(header.time))
            .collect::<Vec<_>>();
        times.sort_unstable();
        let median_time = times[times.len() / 2];
        Ok(ChainTip {
            network: self.network,
            height: tip.height as u64,
            median_time,
            best_block_hash: tip.header.block_hash(),
        })
    }

    fn scan_vault(&self, config: &VaultConfig) -> Result<Vec<VaultUtxo>> {
        if config.bitcoin_network()? != self.network {
            bail!(
                "refusing to scan a {} vault with {} Electrum",
                config.network,
                self.network
            );
        }
        let address = config
            .vault_address
            .parse::<Address<_>>()?
            .require_network(self.network)?;
        let script_pubkey = address.script_pubkey();
        let mut utxos = self
            .client
            .inner
            .script_list_unspent(&script_pubkey)
            .context("Electrum vault UTXO query failed")?
            .into_iter()
            .filter(|utxo| utxo.height > 0)
            .map(|utxo| VaultUtxo {
                outpoint: OutPoint::new(utxo.tx_hash, utxo.tx_pos as u32),
                txout: TxOut {
                    value: Amount::from_sat(utxo.value),
                    script_pubkey: script_pubkey.clone(),
                },
                confirmation_height: utxo.height as u64,
            })
            .collect::<Vec<_>>();
        utxos.sort_by_key(|utxo| (utxo.confirmation_height, utxo.outpoint));
        Ok(utxos)
    }

    fn estimate_fee_rate(&self, target_blocks: u16) -> Result<u64> {
        ElectrumBackend::estimate_fee_rate(self, target_blocks)
    }

    fn broadcast(&self, transaction: &Transaction) -> Result<bitcoin::Txid> {
        self.client
            .transaction_broadcast(transaction)
            .context("Electrum transaction broadcast failed")
    }
}

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

    pub fn estimate_fee_rate(&self, target_blocks: u16) -> Result<u64> {
        // On regtest the mempool is often empty; return the MVP default.
        if self.network == Network::Regtest {
            return Ok(super::DEFAULT_FEE_RATE_SAT_VB);
        }
        use bitcoincore_rpc::json::EstimateSmartFeeResult;
        let result: EstimateSmartFeeResult = self
            .client
            .call("estimatesmartfee", &[serde_json::json!(target_blocks)])
            .context("Bitcoin Core estimatesmartfee failed")?;
        let fee_btc_per_kvb = result
            .fee_rate
            .ok_or_else(|| anyhow::anyhow!("Bitcoin Core could not estimate fee rate"))?;
        // fee_rate is BTC/kvB as an Amount; convert to sat/vB.
        let sat_per_vbyte = fee_btc_per_kvb.to_sat() / 1_000;
        Ok(sat_per_vbyte.max(1))
    }
}

#[cfg(test)]
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

    #[test]
    fn electrum_defaults_cover_public_mainnet_and_local_regtest() {
        assert!(MAINNET_ELECTRUM_SERVERS.contains(&"ssl://bitcoin.lu.ke:50002"));
        assert!(
            MAINNET_ELECTRUM_SERVERS
                .iter()
                .all(|server| server.starts_with("ssl://"))
        );
        assert_eq!(REGTEST_ELECTRUM_SERVERS, &["tcp://127.0.0.1:50001"]);
    }
}
