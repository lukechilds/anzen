use crate::core::{
    keys::DeviceKeys,
    storage::{PHONE_DEVICE_FILE, hot_db_path, load_device_keys},
};
use anyhow::{Context, Result};
use bdk_bitcoind_rpc::{Emitter, NO_EXPECTED_MEMPOOL_TXS};
use bdk_electrum::{BdkElectrumClient, electrum_client::ElectrumApi};
use bdk_wallet::{KeychainKind, PersistedWallet, SignOptions, Wallet, rusqlite::Connection};
use bitcoin::{Address, Amount, FeeRate, OutPoint, ScriptBuf, Transaction, key::Secp256k1};
use bitcoincore_rpc::Client;
use std::{fs, path::Path};

pub struct HotWallet {
    pub wallet: PersistedWallet<Connection>,
    db: Connection,
}

impl HotWallet {
    pub fn open_or_create(data_dir: &Path) -> Result<Self> {
        let keys = load_device_keys(data_dir, PHONE_DEVICE_FILE)?;
        let path = hot_db_path(data_dir);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let db = Connection::open(&path)
            .with_context(|| format!("failed to open hot wallet database {}", path.display()))?;
        Self::open_or_create_with_keys(&keys, db)
    }

    pub fn ephemeral(keys: &DeviceKeys) -> Result<Self> {
        Self::open_or_create_with_keys(keys, Connection::open_in_memory()?)
    }

    fn open_or_create_with_keys(keys: &DeviceKeys, mut db: Connection) -> Result<Self> {
        let secp = Secp256k1::new();
        let (external, internal) = keys.hot_private_descriptors(&secp)?;
        let wallet = match Wallet::load()
            .descriptor(KeychainKind::External, Some(external.clone()))
            .descriptor(KeychainKind::Internal, Some(internal.clone()))
            .extract_keys()
            .check_network(keys.network)
            .load_wallet(&mut db)?
        {
            Some(wallet) => wallet,
            None => Wallet::create(external, internal)
                .network(keys.network)
                .create_wallet(&mut db)?,
        };
        Ok(Self { wallet, db })
    }

    pub fn next_receive_address(&mut self) -> Result<Address> {
        let address = self
            .wallet
            .reveal_next_address(KeychainKind::External)
            .address;
        self.wallet.persist(&mut self.db)?;
        Ok(address)
    }

    pub fn next_change_address(&mut self) -> Result<Address> {
        let address = self
            .wallet
            .reveal_next_address(KeychainKind::Internal)
            .address;
        self.wallet.persist(&mut self.db)?;
        Ok(address)
    }

    pub(crate) fn sync_core(&mut self, client: &Client) -> Result<()> {
        let mut emitter = Emitter::new(
            client,
            self.wallet.latest_checkpoint(),
            0,
            NO_EXPECTED_MEMPOOL_TXS,
        );
        while let Some(event) = emitter.next_block()? {
            self.wallet.apply_block_connected_to(
                &event.block,
                event.block_height(),
                event.connected_to(),
            )?;
        }
        let mempool = emitter.mempool()?;
        self.wallet.apply_unconfirmed_txs(mempool.update);
        self.wallet.apply_evicted_txs(mempool.evicted);
        self.wallet.persist(&mut self.db)?;
        Ok(())
    }

    pub(crate) fn sync_electrum<E: ElectrumApi>(
        &mut self,
        client: &BdkElectrumClient<E>,
    ) -> Result<()> {
        client.populate_tx_cache(self.wallet.tx_graph().full_txs().map(|node| node.tx));
        let request = self.wallet.start_sync_with_revealed_spks();
        let update = client.sync(request, 20, false)?;
        self.wallet.apply_update(update)?;
        self.wallet.persist(&mut self.db)?;
        Ok(())
    }

    pub fn build_soft_limit_return(
        &mut self,
        authorization_outpoint: OutPoint,
        monthly_limit_sats: u64,
        soft_limit_sats: u64,
        vault_script: ScriptBuf,
    ) -> Result<Option<Transaction>> {
        if soft_limit_sats > monthly_limit_sats {
            anyhow::bail!("soft limit cannot exceed the active monthly limit");
        }
        if self.wallet.get_utxo(authorization_outpoint).is_none() {
            anyhow::bail!(
                "hot wallet does not contain authorization output {authorization_outpoint}"
            );
        }
        if soft_limit_sats == monthly_limit_sats {
            return Ok(None);
        }

        let change_script = self.next_change_address()?.script_pubkey();
        let requested_cold_return = monthly_limit_sats - soft_limit_sats;
        let mut builder = self.wallet.build_tx();
        builder
            .add_utxo(authorization_outpoint)?
            .manually_selected_only()
            .fee_rate(FeeRate::from_sat_per_vb(1).expect("1 sat/vB is valid"))
            .drain_to(change_script);
        if soft_limit_sats == 0 {
            // With a zero soft limit there is no hot remainder from which to pay the child fee.
            // Draining the input to the vault makes the fee come out of the cold return instead.
            builder.drain_to(vault_script);
        } else {
            builder.add_recipient(vault_script, Amount::from_sat(requested_cold_return));
        }
        let mut psbt = builder.finish()?;
        let finalized = self.wallet.sign(&mut psbt, SignOptions::default())?;
        if !finalized {
            anyhow::bail!("BDK could not finalize the soft-limit return transaction");
        }
        let transaction = psbt.extract_tx()?;
        self.wallet.persist(&mut self.db)?;
        Ok(Some(transaction))
    }

    pub fn build_payment(
        &mut self,
        destination: ScriptBuf,
        amount_sats: u64,
    ) -> Result<(Transaction, u64)> {
        if amount_sats == 0 {
            anyhow::bail!("payment amount must be greater than zero");
        }
        let mut builder = self.wallet.build_tx();
        builder
            .add_recipient(destination, Amount::from_sat(amount_sats))
            .fee_rate(FeeRate::from_sat_per_vb(1).expect("1 sat/vB is valid"));
        let mut psbt = builder.finish()?;
        let finalized = self.wallet.sign(&mut psbt, SignOptions::default())?;
        if !finalized {
            anyhow::bail!("BDK could not finalize the hot-wallet payment");
        }
        let transaction = psbt.extract_tx()?;
        let fee_sats = self.wallet.calculate_fee(&transaction)?.to_sat();
        self.wallet.persist(&mut self.db)?;
        Ok((transaction, fee_sats))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::initialize;

    #[test]
    fn bdk_hot_wallet_persists_external_and_change_derivation() {
        let dir = tempfile::tempdir().unwrap();
        initialize(dir.path()).unwrap();
        let (first_receive, first_change) = {
            let mut hot = HotWallet::open_or_create(dir.path()).unwrap();
            (
                hot.next_receive_address().unwrap(),
                hot.next_change_address().unwrap(),
            )
        };
        assert_ne!(first_receive, first_change);

        let mut reopened = HotWallet::open_or_create(dir.path()).unwrap();
        assert_ne!(first_receive, reopened.next_receive_address().unwrap());
        assert_ne!(first_change, reopened.next_change_address().unwrap());
    }

    #[test]
    fn zero_value_hot_payment_is_rejected() {
        let dir = tempfile::tempdir().unwrap();
        initialize(dir.path()).unwrap();
        let mut hot = HotWallet::open_or_create(dir.path()).unwrap();
        assert!(hot.build_payment(ScriptBuf::new(), 0).is_err());
    }
}
