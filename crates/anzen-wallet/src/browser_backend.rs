//! Browser network adapters. Only public chain queries and fully signed broadcasts leave
//! this worker. The offline cold signer does not construct or receive them.

use super::{
    HotWallet, HotWalletBackend,
    api::{Blockchain, ChainTip, VaultConfig, VaultUtxo},
};
use anyhow::{Result, ensure};
use anzen_common::storage::browser;
use bitcoin::{Network, Transaction, Txid};
use serde::{Serialize, de::DeserializeOwned};
use serde_json::json;

/// Public chain access for a browser wallet: Electrum over WebSocket on Mutinynet and mainnet,
/// and the local playground's Bitcoin Core on regtest.
pub fn connect(network: Network) -> Result<Box<dyn HotWalletBackend>> {
    Ok(match network {
        Network::Regtest => Box::new(BrowserBackend { network }),
        #[cfg(feature = "electrum")]
        _ => Box::new(super::electrum::ElectrumBackend::new(
            network,
            super::electrum::browser::BrowserConnector,
        )?),
        #[cfg(not(feature = "electrum"))]
        _ => anyhow::bail!("this browser build has no Electrum transport"),
    })
}

/// Regtest only: the local playground host relays public chain queries to Bitcoin Core.
pub struct BrowserBackend {
    network: Network,
}
impl BrowserBackend {
    fn call<T: DeserializeOwned>(&self, request: impl Serialize) -> Result<T> {
        browser::call(&json!({"type":"chain", "network":self.network, "request":request}))
    }
}
impl Blockchain for BrowserBackend {
    fn network(&self) -> Network {
        self.network
    }
    fn backend_description(&self) -> String {
        "Browser chain transport".into()
    }
    fn chain_tip(&self) -> Result<ChainTip> {
        let tip: ChainTip = self.call(json!({"method":"tip"}))?;
        ensure!(
            tip.network == self.network,
            "chain transport returned the wrong network"
        );
        Ok(tip)
    }
    fn median_time_past(&self, height: u64) -> Result<u64> {
        self.call(json!({"method":"median_time_past", "height":height}))
    }
    fn scan_vault(&self, config: &VaultConfig) -> Result<Vec<VaultUtxo>> {
        self.call(json!({"method":"scan_vault", "config":config}))
    }
    fn scan_connectors(&self, config: &VaultConfig) -> Result<Vec<VaultUtxo>> {
        self.call(json!({"method":"scan_connectors", "config":config}))
    }
    fn previous_transaction(&self, utxo: &VaultUtxo) -> Result<Transaction> {
        let tx: Transaction = self.call(json!({"method":"previous_transaction", "utxo":utxo}))?;
        ensure!(
            tx.compute_txid() == utxo.outpoint.txid,
            "chain transport returned a different transaction"
        );
        Ok(tx)
    }
    fn broadcast(&self, transaction: &Transaction) -> Result<Txid> {
        let txid = self.call(json!({"method":"broadcast", "transaction":transaction}))?;
        ensure!(
            txid == transaction.compute_txid(),
            "chain transport returned a different broadcast txid"
        );
        Ok(txid)
    }
}
impl HotWalletBackend for BrowserBackend {
    fn sync_hot_wallet(&self, wallet: &mut HotWallet) -> Result<()> {
        let update = self.call(json!({"method":"sync", "state":wallet.public_state()}))?;
        wallet.apply_chain_update(update)
    }
}
