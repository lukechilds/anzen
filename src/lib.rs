//! Reusable building blocks for the vault applications.
//!
//! The dependency direction is intentional:
//!
//! ```text
//! cold_wallet ─┐
//!              ├──> core
//! hot_wallet ──┘
//! ```
//!
//! `cold_wallet` and `hot_wallet` never depend on one another. Application front ends, such as
//! the CLI, compose their public APIs.

pub mod cold_wallet;
pub mod core;
pub mod hot_wallet;

#[cfg(test)]
pub(crate) mod test_support {
    use crate::{cold_wallet, core, hot_wallet};
    use anyhow::Result;
    use bitcoin::Network;
    use std::path::Path;

    pub struct InitializedVault {
        pub config: core::storage::VaultConfig,
        pub phone_mnemonic: String,
    }

    pub fn initialize(data_dir: &Path) -> Result<InitializedVault> {
        initialize_for_network(data_dir, Network::Regtest)
    }

    pub fn initialize_for_network(data_dir: &Path, network: Network) -> Result<InitializedVault> {
        let phone = hot_wallet::initialize(data_dir, network)?;
        cold_wallet::initialize(data_dir, network)?;
        let config = core::storage::initialize_vault_for_network(data_dir, network)?;
        cold_wallet::create_cloud_recovery_backup(data_dir, &config)?;
        Ok(InitializedVault {
            config,
            phone_mnemonic: phone.mnemonic,
        })
    }
}
