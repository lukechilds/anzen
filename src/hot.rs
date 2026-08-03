use crate::{
    keys::DeviceKeys,
    state::{PHONE_DEVICE_FILE, hot_db_path, load_device},
};
use anyhow::{Context, Result};
use bdk_bitcoind_rpc::{Emitter, NO_EXPECTED_MEMPOOL_TXS};
use bdk_wallet::{KeychainKind, PersistedWallet, Wallet, rusqlite::Connection};
use bitcoin::{Address, Network, key::Secp256k1};
use bitcoincore_rpc::Client;
use std::{fs, path::Path};

pub struct HotWallet {
    pub wallet: PersistedWallet<Connection>,
    db: Connection,
}

impl HotWallet {
    pub fn open_or_create(data_dir: &Path) -> Result<Self> {
        let device = load_device(data_dir, PHONE_DEVICE_FILE)?;
        let secp = Secp256k1::new();
        let keys = DeviceKeys::parse(&secp, &device.mnemonic)?;
        let (external, internal) = keys.hot_private_descriptors(&secp)?;
        let path = hot_db_path(data_dir);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let mut db = Connection::open(&path)
            .with_context(|| format!("failed to open hot wallet database {}", path.display()))?;
        let wallet = match Wallet::load()
            .descriptor(KeychainKind::External, Some(external.clone()))
            .descriptor(KeychainKind::Internal, Some(internal.clone()))
            .extract_keys()
            .check_network(Network::Regtest)
            .load_wallet(&mut db)?
        {
            Some(wallet) => wallet,
            None => Wallet::create(external, internal)
                .network(Network::Regtest)
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

    pub fn sync(&mut self, client: &Client) -> Result<()> {
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::initialize;

    #[test]
    fn bdk_hot_wallet_persists_external_and_change_derivation() {
        let dir = tempfile::tempdir().unwrap();
        initialize(dir.path(), 10_000_000).unwrap();
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
}
