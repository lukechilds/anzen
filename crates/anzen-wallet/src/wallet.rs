#[cfg(target_arch = "wasm32")]
use super::browser_store::BrowserStore as Connection;
use anyhow::{Context, Result};
use anzen_common::storage::fs;
use anzen_common::{
    keys::DeviceKeys,
    storage::{PHONE_DEVICE_FILE, load_device_keys},
};
#[cfg(feature = "native")]
use bdk_bitcoind_rpc::{Emitter, NO_EXPECTED_MEMPOOL_TXS};
#[cfg(feature = "electrum")]
use bdk_electrum::{BdkElectrumClient, electrum_client::ElectrumApi};
#[cfg(feature = "native")]
use bdk_wallet::rusqlite::Connection;
use bdk_wallet::{KeychainKind, PersistedWallet, SignOptions, Wallet};
use bitcoin::{Address, Amount, FeeRate, OutPoint, ScriptBuf, Transaction, key::Secp256k1};
#[cfg(feature = "native")]
use bitcoincore_rpc::Client;
use std::path::Path;

#[cfg(feature = "native")]
const FRESH_TABLE: &str =
    "CREATE TABLE IF NOT EXISTS anzen_fresh_wallet (id INTEGER PRIMARY KEY CHECK (id = 0))";

// Policy proposals reserve multiple receive addresses at once. Allow more than the usual
// twenty-address gap when recovering a phone that has lost its address-index database.
#[cfg(feature = "electrum")]
const RECOVERY_STOP_GAP: usize = 100;

pub struct HotWallet {
    pub wallet: PersistedWallet<Connection>,
    db: Connection,
}

impl HotWallet {
    /// A temporary watch-only wallet for the network adapter. BDK ChangeSet descriptors use
    /// DescriptorPublicKey, so this API cannot deserialize an extended private key.
    #[cfg(feature = "native")]
    pub fn from_public_state(state: super::chain_transport::PublicWalletState) -> Result<Self> {
        use bdk_wallet::WalletPersister;
        let mut db = Connection::open_in_memory()?;
        <Connection as WalletPersister>::initialize(&mut db)?;
        <Connection as WalletPersister>::persist(&mut db, &state.changeset)?;
        let wallet = Wallet::load()
            .load_wallet(&mut db)?
            .context("public wallet is not initialized")?;
        db.execute(
            "CREATE TABLE anzen_sync_state (id INTEGER PRIMARY KEY CHECK (id = 0))",
            [],
        )?;
        if state.discovered {
            db.execute("INSERT INTO anzen_sync_state (id) VALUES (0)", [])?;
        }
        db.execute(FRESH_TABLE, [])?;
        if state.fresh {
            db.execute("INSERT INTO anzen_fresh_wallet (id) VALUES (0)", [])?;
        }
        Ok(Self { wallet, db })
    }

    #[cfg(feature = "native")]
    pub fn public_chain_update(
        &self,
        previous_checkpoints: &std::collections::BTreeSet<u32>,
    ) -> super::chain_transport::WalletChainUpdate {
        super::chain_transport::WalletChainUpdate::from_wallet(&self.wallet, previous_checkpoints)
    }

    #[cfg(target_arch = "wasm32")]
    pub fn public_state(&self) -> super::chain_transport::PublicWalletState {
        super::chain_transport::PublicWalletState {
            changeset: self.db.state.changeset.clone(),
            discovered: self.db.state.discovered,
            fresh: self.db.state.fresh,
        }
    }

    #[cfg(target_arch = "wasm32")]
    pub fn apply_chain_update(
        &mut self,
        update: super::chain_transport::WalletChainUpdate,
    ) -> Result<()> {
        let network = self.wallet.network();
        self.wallet.apply_update(update.into_update(network)?)?;
        self.wallet.persist(&mut self.db)?;
        self.db.set_discovered(true)
    }

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
        #[cfg(feature = "native")]
        db.execute(
            "CREATE TABLE IF NOT EXISTS anzen_sync_state (id INTEGER PRIMARY KEY CHECK (id = 0))",
            [],
        )?;
        #[cfg(feature = "native")]
        db.execute(FRESH_TABLE, [])?;
        Ok(Self { wallet, db })
    }

    /// Mark a wallet whose seed was just generated on this device. Never set for a restored or
    /// imported seed.
    pub(crate) fn mark_fresh(&mut self) -> Result<()> {
        #[cfg(feature = "native")]
        self.db.execute(
            "INSERT OR IGNORE INTO anzen_fresh_wallet (id) VALUES (0)",
            [],
        )?;
        #[cfg(target_arch = "wasm32")]
        self.db.set_fresh(true)?;
        Ok(())
    }

    fn is_fresh(&self) -> Result<bool> {
        #[cfg(feature = "native")]
        return Ok(self.db.query_row(
            "SELECT EXISTS(SELECT 1 FROM anzen_fresh_wallet WHERE id = 0)",
            [],
            |row| row.get(0),
        )?);
        #[cfg(target_arch = "wasm32")]
        return Ok(self.db.state.fresh);
    }

    /// Whether a full-block sync may start at the chain tip instead of genesis. Only when this
    /// device generated the seed and has never revealed any of its addresses: then nobody can
    /// have paid it, so no history is skipped. Revealing an address first (even before any
    /// sync) makes the first sync scan the whole chain as usual.
    pub(crate) fn starts_at_tip(&self) -> Result<bool> {
        Ok(self.wallet.latest_checkpoint().height() == 0
            && self
                .wallet
                .derivation_index(KeychainKind::External)
                .is_none()
            && self
                .wallet
                .derivation_index(KeychainKind::Internal)
                .is_none()
            && self.is_fresh()?)
    }

    pub fn next_receive_address(&mut self) -> Result<Address> {
        let address = self
            .wallet
            .reveal_next_address(KeychainKind::External)
            .address;
        self.wallet.persist(&mut self.db)?;
        Ok(address)
    }

    /// Whether this wallet derived `script`, including addresses revealed by policy proposals.
    pub fn is_mine(&self, script: bitcoin::ScriptBuf) -> bool {
        self.wallet.is_mine(script)
    }

    pub fn next_change_address(&mut self) -> Result<Address> {
        let address = self
            .wallet
            .reveal_next_address(KeychainKind::Internal)
            .address;
        self.wallet.persist(&mut self.db)?;
        Ok(address)
    }

    #[cfg(feature = "native")]
    pub(crate) fn sync_core(&mut self, client: &Client) -> Result<()> {
        use bitcoincore_rpc::RpcApi;
        // Stop at the tip seen when the sync began. Blocks mined meanwhile (a recovery-mining
        // job adds tens of thousands) wait for the next sync instead of holding this one open.
        let tip = u32::try_from(client.get_block_count()?)?;
        let start_height = if self.starts_at_tip()? { tip } else { 0 };
        let mut emitter = Emitter::new(
            client,
            self.wallet.latest_checkpoint(),
            start_height,
            NO_EXPECTED_MEMPOOL_TXS,
        );
        while let Some(event) = emitter.next_block()? {
            self.wallet.apply_block_connected_to(
                &event.block,
                event.block_height(),
                event.connected_to(),
            )?;
            if event.block_height() >= tip {
                break;
            }
        }
        let mempool = emitter.mempool()?;
        self.wallet.apply_unconfirmed_txs(mempool.update);
        self.wallet.apply_evicted_txs(mempool.evicted);
        self.wallet.persist(&mut self.db)?;
        Ok(())
    }

    /// Electrum sync on every platform. The first sync discovers addresses with a wide gap;
    /// later syncs check the revealed addresses only.
    #[cfg(feature = "electrum")]
    pub(crate) fn sync_electrum<E: ElectrumApi>(
        &mut self,
        client: &BdkElectrumClient<E>,
    ) -> Result<()> {
        // Browsers have no `SystemTime`; BDK takes the scan time explicitly.
        let now = u64::try_from(chrono::Utc::now().timestamp())?;
        client.populate_tx_cache(self.wallet.tx_graph().full_txs().map(|node| node.tx));
        let discovered = self.discovered()?;
        if !discovered {
            let request = self.wallet.start_full_scan_at(now);
            let update = client.full_scan(request, RECOVERY_STOP_GAP, 20, false)?;
            self.wallet.apply_update(update)?;
            self.wallet.persist(&mut self.db)?;
        }
        // Still sync all known addresses: an existing database can contain revealed indices
        // beyond the discovery gap. Ordinary subsequent syncs need only this inexpensive path.
        let request = self.wallet.start_sync_with_revealed_spks_at(now);
        let update = client.sync(request, 20, false)?;
        self.wallet.apply_update(update)?;
        self.wallet.persist(&mut self.db)?;
        if !discovered {
            self.mark_discovered()?;
        }
        Ok(())
    }

    #[cfg(feature = "electrum")]
    fn discovered(&self) -> Result<bool> {
        #[cfg(feature = "native")]
        return Ok(self.db.query_row(
            "SELECT EXISTS(SELECT 1 FROM anzen_sync_state WHERE id = 0)",
            [],
            |row| row.get(0),
        )?);
        #[cfg(target_arch = "wasm32")]
        return Ok(self.db.state.discovered);
    }

    /// Written only after a successful, persisted sync.
    #[cfg(feature = "electrum")]
    fn mark_discovered(&mut self) -> Result<()> {
        #[cfg(feature = "native")]
        self.db
            .execute("INSERT OR IGNORE INTO anzen_sync_state (id) VALUES (0)", [])?;
        #[cfg(target_arch = "wasm32")]
        self.db.set_discovered(true)?;
        Ok(())
    }

    /// Require address discovery at the next Electrum sync, including when restoring over a
    /// surviving database. The completion marker is written only after a successful persisted sync.
    pub fn request_full_scan(&mut self) -> Result<()> {
        #[cfg(feature = "native")]
        {
            self.db.execute("DELETE FROM anzen_sync_state", [])?;
            self.db.execute("DELETE FROM anzen_fresh_wallet", [])?;
        }
        #[cfg(target_arch = "wasm32")]
        {
            self.db.set_discovered(false)?;
            self.db.set_fresh(false)?;
        }
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
        let (psbt, fee_sats) = self.prepare_payment(destination, amount_sats)?;
        Ok((self.sign_payment(psbt)?, fee_sats))
    }

    /// Reserve change and construct the exact unsigned payment for review, without signing it.
    pub fn prepare_payment(
        &mut self,
        destination: ScriptBuf,
        amount_sats: u64,
    ) -> Result<(bitcoin::Psbt, u64)> {
        if amount_sats == 0 {
            anyhow::bail!("payment amount must be greater than zero");
        }
        let mut builder = self.wallet.build_tx();
        builder
            .add_recipient(destination, Amount::from_sat(amount_sats))
            .fee_rate(FeeRate::from_sat_per_vb(1).expect("1 sat/vB is valid"));
        let psbt = builder.finish()?;
        let fee_sats = psbt.fee()?.to_sat();
        self.wallet.persist(&mut self.db)?;
        Ok((psbt, fee_sats))
    }

    pub fn sign_payment(&mut self, mut psbt: bitcoin::Psbt) -> Result<Transaction> {
        let finalized = self.wallet.sign(&mut psbt, SignOptions::default())?;
        if !finalized {
            anyhow::bail!("BDK could not finalize the hot-wallet payment");
        }
        let transaction = psbt.extract_tx()?;
        self.wallet.persist(&mut self.db)?;
        Ok(transaction)
    }

    /// Move all currently spendable hot coins to the replacement phone. Immature coinbase
    /// outputs cannot be moved yet; rotation retains the old key and database for those coins.
    pub fn build_sweep(&mut self, destination: ScriptBuf) -> Result<Option<Transaction>> {
        self.prepare_sweep(destination)?
            .map(|psbt| self.sign_payment(psbt))
            .transpose()
    }

    /// Construct a reviewed drain transaction without signing it yet.
    pub fn prepare_sweep(&mut self, destination: ScriptBuf) -> Result<Option<bitcoin::Psbt>> {
        let balance = self.wallet.balance();
        if balance.total() == balance.immature {
            return Ok(None);
        }
        let mut builder = self.wallet.build_tx();
        builder
            .drain_wallet()
            .drain_to(destination)
            .fee_rate(FeeRate::from_sat_per_vb(1).expect("1 sat/vB is valid"));
        let psbt = builder.finish()?;
        self.wallet.persist(&mut self.db)?;
        Ok(Some(psbt))
    }
}

pub fn hot_db_path(data_dir: &Path) -> std::path::PathBuf {
    data_dir.join("phone/hot-wallet.sqlite")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::initialize;

    #[test]
    fn bdk_hot_wallet_persists_external_and_change_derivation() {
        let dir = anzen_common::storage::tempdir().unwrap();
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
    fn only_a_generated_seed_with_no_revealed_address_may_skip_history() {
        let dir = anzen_common::storage::tempdir().unwrap();
        initialize(dir.path()).unwrap();
        // A copy of the seed elsewhere counts as restored: it may have been paid.
        let restored = anzen_common::storage::tempdir().unwrap();
        std::fs::create_dir_all(restored.path().join("phone")).unwrap();
        std::fs::copy(
            dir.path().join(PHONE_DEVICE_FILE),
            restored.path().join(PHONE_DEVICE_FILE),
        )
        .unwrap();
        assert!(
            !HotWallet::open_or_create(restored.path())
                .unwrap()
                .starts_at_tip()
                .unwrap()
        );
        // `initialize` pairs the devices but reveals no hot-wallet address.
        let mut hot = HotWallet::open_or_create(dir.path()).unwrap();
        assert!(hot.starts_at_tip().unwrap());
        // Once an address has been shown, it may have been paid before the first sync.
        hot.next_receive_address().unwrap();
        assert!(!hot.starts_at_tip().unwrap());

        let other = anzen_common::storage::tempdir().unwrap();
        initialize(other.path()).unwrap();
        let mut hot = HotWallet::open_or_create(other.path()).unwrap();
        hot.request_full_scan().unwrap();
        assert!(!hot.starts_at_tip().unwrap());
    }

    #[test]
    fn zero_value_hot_payment_is_rejected() {
        let dir = anzen_common::storage::tempdir().unwrap();
        initialize(dir.path()).unwrap();
        let mut hot = HotWallet::open_or_create(dir.path()).unwrap();
        assert!(hot.build_payment(ScriptBuf::new(), 0).is_err());
    }
}
