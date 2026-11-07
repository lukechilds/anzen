//! Public chain synchronization messages. These contain BDK public descriptors/history,
//! never private descriptors, seeds, signing requests or application state.

use anyhow::{Context, Result, ensure};
use bdk_wallet::{
    ChangeSet, KeychainKind, Update,
    chain::{BlockId, CheckPoint, ConfirmationBlockTime, TxUpdate},
};
use bitcoin::{OutPoint, Transaction, TxOut, Txid};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};

// Signets share a genesis. Pin block one as well to distinguish Mutinynet.
pub const MUTINYNET_BLOCK_ONE: &str =
    "000002855893a0a9b24eaffc5efc770558a326fee4fc10c9da22fc19cd2954f9";

#[cfg(feature = "electrum")]
pub(crate) fn check_config(
    config: &super::api::VaultConfig,
    network: bitcoin::Network,
) -> Result<()> {
    ensure!(
        config.bitcoin_network()? == network,
        "vault network mismatch"
    );
    config.validate_public_policy()
}

#[cfg(feature = "electrum")]
pub(crate) fn verify_previous(tx: &Transaction, utxo: &super::api::VaultUtxo) -> Result<()> {
    ensure!(
        tx.compute_txid() == utxo.outpoint.txid,
        "indexer returned a different transaction"
    );
    ensure!(
        tx.output.get(utxo.outpoint.vout as usize) == Some(&utxo.txout),
        "indexer UTXO amount or script disagrees with its transaction"
    );
    Ok(())
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PublicWalletState {
    pub changeset: ChangeSet,
    pub discovered: bool,
    /// The seed was generated on its device (see `HotWallet::starts_at_tip`).
    #[serde(default)]
    pub fresh: bool,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WalletChainUpdate {
    pub checkpoints: Vec<BlockId>,
    pub transactions: Vec<Arc<Transaction>>,
    pub txouts: BTreeMap<OutPoint, TxOut>,
    pub anchors: BTreeSet<(ConfirmationBlockTime, Txid)>,
    pub seen_ats: Vec<(Txid, u64)>,
    pub evicted_ats: Vec<(Txid, u64)>,
    pub last_active_indices: BTreeMap<KeychainKind, u32>,
}

impl WalletChainUpdate {
    /// Export only public graph/chain data from a watch-only synchronization wallet.
    pub fn from_wallet(wallet: &bdk_wallet::Wallet, previous: &BTreeSet<u32>) -> Self {
        let graph = wallet.tx_graph().initial_changeset();
        let anchors: BTreeSet<_> = graph
            .anchors
            .iter()
            .map(|(a, _)| a.block_id.height)
            .collect();
        let tip = wallet.latest_checkpoint();
        let mut checkpoints: Vec<_> = tip
            .iter()
            .filter(|p| {
                p.height() == 0
                    || p.height() >= tip.height().saturating_sub(10)
                    || previous.contains(&p.height())
                    || anchors.contains(&p.height())
            })
            .map(|p| p.block_id())
            .collect();
        checkpoints.reverse();
        Self {
            checkpoints,
            transactions: graph.txs.into_iter().collect(),
            txouts: graph.txouts,
            anchors: graph.anchors,
            seen_ats: graph.last_seen.into_iter().collect(),
            evicted_ats: graph.last_evicted.into_iter().collect(),
            last_active_indices: wallet.spk_index().last_revealed_indices(),
        }
    }

    pub fn into_update(self, network: bitcoin::Network) -> Result<Update> {
        ensure!(
            self.checkpoints
                .first()
                .is_some_and(|block| block.height == 0
                    && block.hash
                        == bitcoin::blockdata::constants::genesis_block(network).block_hash()),
            "chain update has the wrong genesis"
        );
        let chain = CheckPoint::from_block_ids(self.checkpoints)
            .map_err(|_| anyhow::anyhow!("invalid chain checkpoints"))?;
        let mut tx_update = TxUpdate::default();
        tx_update.txs = self.transactions;
        tx_update.txouts = self.txouts;
        tx_update.anchors = self.anchors;
        tx_update.seen_ats = self.seen_ats.into_iter().collect();
        tx_update.evicted_ats = self.evicted_ats.into_iter().collect();
        Ok(Update {
            chain: Some(chain),
            tx_update,
            last_active_indices: self.last_active_indices,
        })
    }
}

impl PublicWalletState {
    pub fn network(&self) -> Result<bitcoin::Network> {
        self.changeset
            .network
            .context("public wallet state has no network")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::HotWallet;
    use anzen_common::keys::DeviceKeys;
    use bitcoin::{Network, key::Secp256k1};

    #[test]
    fn public_state_cannot_contain_private_descriptors() {
        let keys = DeviceKeys::generate_for_network(&Secp256k1::new(), Network::Regtest).unwrap();
        let hot = HotWallet::ephemeral(&keys).unwrap();
        let changeset = ChangeSet {
            descriptor: Some(hot.wallet.public_descriptor(KeychainKind::External).clone()),
            change_descriptor: Some(hot.wallet.public_descriptor(KeychainKind::Internal).clone()),
            network: Some(Network::Regtest),
            local_chain: hot.wallet.local_chain().initial_changeset(),
            ..Default::default()
        };
        let mut public = serde_json::to_value(PublicWalletState {
            changeset,
            discovered: false,
            fresh: false,
        })
        .unwrap();
        assert!(!public.to_string().contains("tprv"));
        let watch =
            HotWallet::from_public_state(serde_json::from_value(public.clone()).unwrap()).unwrap();
        assert!(
            watch
                .wallet
                .get_signers(KeychainKind::External)
                .signers()
                .is_empty()
        );
        let (secret, _) = keys.hot_private_descriptors(&Secp256k1::new()).unwrap();
        public["changeset"]["descriptor"] = serde_json::Value::String(secret);
        assert!(serde_json::from_value::<PublicWalletState>(public).is_err());
    }

    #[test]
    fn chain_updates_require_the_devices_genesis_and_ordered_checkpoints() {
        let keys = DeviceKeys::generate_for_network(&Secp256k1::new(), Network::Regtest).unwrap();
        let hot = HotWallet::ephemeral(&keys).unwrap();
        let update = hot.public_chain_update(&BTreeSet::new());
        let json = serde_json::to_string(&update).unwrap();
        assert!(
            serde_json::from_str::<WalletChainUpdate>(&json)
                .unwrap()
                .into_update(Network::Regtest)
                .is_ok()
        );
        assert!(
            serde_json::from_str::<WalletChainUpdate>(&json)
                .unwrap()
                .into_update(Network::Bitcoin)
                .is_err()
        );
        let mut update: WalletChainUpdate = serde_json::from_str(&json).unwrap();
        update.checkpoints.push(update.checkpoints[0]);
        assert!(update.into_update(Network::Regtest).is_err());
    }
}
