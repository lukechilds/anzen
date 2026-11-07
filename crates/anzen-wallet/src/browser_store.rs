//! BDK's portable persistence contract, backed by device-local browser storage. BDK still
//! controls address discovery, coin selection and signing; no wallet logic moves into JS.

use anyhow::Result;
use anzen_common::storage::{fs::StoragePath, read_json, write_json};
use bdk_wallet::{ChangeSet, WalletPersister, chain::Merge};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Default, Serialize, Deserialize)]
pub struct State {
    pub changeset: ChangeSet,
    pub discovered: bool,
    /// The seed was generated on this device (see `HotWallet::starts_at_tip`).
    #[serde(default)]
    pub fresh: bool,
}

pub struct BrowserStore {
    path: Option<PathBuf>,
    pub state: State,
}

impl BrowserStore {
    pub fn open(path: &Path) -> Result<Self> {
        let state = if path.storage_exists() {
            read_json(path)?
        } else {
            State::default()
        };
        Ok(Self {
            path: Some(path.to_path_buf()),
            state,
        })
    }
    pub fn open_in_memory() -> Result<Self> {
        Ok(Self {
            path: None,
            state: State::default(),
        })
    }
    pub fn set_discovered(&mut self, discovered: bool) -> Result<()> {
        let previous = self.state.discovered;
        self.state.discovered = discovered;
        if let Err(error) = self.save() {
            self.state.discovered = previous;
            return Err(error);
        }
        Ok(())
    }
    pub fn set_fresh(&mut self, fresh: bool) -> Result<()> {
        let previous = self.state.fresh;
        self.state.fresh = fresh;
        if let Err(error) = self.save() {
            self.state.fresh = previous;
            return Err(error);
        }
        Ok(())
    }
    fn save(&self) -> Result<()> {
        if let Some(path) = &self.path {
            write_json(path, &self.state)?;
        }
        Ok(())
    }
}

impl WalletPersister for BrowserStore {
    type Error = anyhow::Error;
    fn initialize(persister: &mut Self) -> Result<ChangeSet> {
        Ok(persister.state.changeset.clone())
    }
    fn persist(persister: &mut Self, changeset: &ChangeSet) -> Result<()> {
        let old = persister.state.changeset.clone();
        persister.state.changeset.merge(changeset.clone());
        if let Err(error) = persister.save() {
            persister.state.changeset = old;
            return Err(error);
        }
        Ok(())
    }
}
