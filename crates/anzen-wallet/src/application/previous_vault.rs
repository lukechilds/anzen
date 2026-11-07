//! Keep retired signer vaults discoverable on the surviving phone. Sending to an old address
//! does not reset older coins' clocks; each eligible cohort can move to the current vault.
use super::delayed_recovery::RecoveryKind;
use super::*;
use crate::recovery::{self, SweepPath};
use anzen_common::storage::fs::StoragePath;

#[derive(Debug, Serialize)]
pub struct PreviousVaultSnapshot {
    pub vault_id: String,
    pub vault_address: String,
    pub phone: RecoveryTiming,
    pub hww: RecoveryTiming,
    pub can_recover: bool,
}

fn vault_id(config: &VaultConfig) -> String {
    sha256::Hash::hash(config.vault_descriptor.as_bytes()).to_string()
}

impl WalletApplication {
    pub(super) fn remember_previous_vault(&self, config: &VaultConfig) -> Result<()> {
        config.validate_public_policy()?;
        let phone = self.public_identity()?;
        if config.network != phone.network || config.phone_vault_pubkey != phone.vault_pubkey {
            bail!("retired vault does not belong to the surviving phone");
        }
        storage::write_json(&self.previous_vault_path(&vault_id(config))?, config)
    }

    fn previous_vault(&self, id: &str) -> Result<VaultConfig> {
        let config: VaultConfig = storage::read_json(&self.previous_vault_path(id)?)?;
        config.validate_public_policy()?;
        if vault_id(&config) != id || config.network != self.public_identity()?.network {
            bail!("retired vault record does not match its identity or network");
        }
        Ok(config)
    }

    pub fn previous_vaults(
        &self,
        backend: &dyn HotWalletBackend,
    ) -> Result<Vec<PreviousVaultSnapshot>> {
        self.check_backend(backend)?;
        let current = self.configuration()?;
        let phone = self.public_identity()?;
        let directory = self.data_dir.join("phone/previous-vaults");
        if !directory.storage_exists() {
            return Ok(vec![]);
        }
        let mut ids = Vec::new();
        for entry in anzen_common::storage::fs::read_dir(directory)? {
            let path = entry?.path();
            if path.extension().and_then(|s| s.to_str()) == Some("json") {
                ids.push(
                    path.file_stem()
                        .and_then(|s| s.to_str())
                        .context("invalid retired vault filename")?
                        .to_owned(),
                );
            }
        }
        ids.sort();
        let tip = backend.chain_tip()?.height;
        let mut snapshots = Vec::new();
        for id in ids {
            let config = self.previous_vault(&id)?;
            if config.vault_descriptor == current.vault_descriptor {
                continue;
            }
            let inputs = backend.scan_vault(&config)?;
            snapshots.push(PreviousVaultSnapshot {
                vault_id: id,
                vault_address: config.vault_address,
                phone: recovery::recovery_timing(&inputs, SweepPath::PhoneRecovery, tip)?,
                hww: recovery::recovery_timing(&inputs, SweepPath::HwwRecovery, tip)?,
                can_recover: config.phone_vault_pubkey == phone.vault_pubkey,
            });
        }
        Ok(snapshots)
    }

    pub fn review_previous_vault_recovery(
        &self,
        backend: &dyn HotWalletBackend,
        id: &str,
    ) -> Result<RecoveryTransferReview> {
        let old = self.previous_vault(id)?;
        let current = self.configuration()?;
        if old.phone_vault_pubkey != self.public_identity()?.vault_pubkey {
            bail!("the previous phone key is required to recover this retired vault");
        }
        if old.vault_descriptor == current.vault_descriptor {
            bail!("this is the current vault, not a retired signer vault");
        }
        self.prepare_delayed_recovery(
            backend,
            old,
            &current.vault_address,
            RecoveryKind::PreviousVault,
        )
    }

    pub fn confirm_previous_vault_recovery(
        &self,
        backend: &dyn HotWalletBackend,
        review_id: &str,
    ) -> Result<Txid> {
        let record = self.delayed_recovery_record(RecoveryKind::PreviousVault, review_id)?;
        let old = self.previous_vault(&vault_id(&record.config))?;
        if serde_json::to_vec(&old)? != serde_json::to_vec(&record.config)?
            || record.destination != self.configuration()?.vault_address
            || old.phone_vault_pubkey != self.public_identity()?.vault_pubkey
        {
            bail!("retired vault, current destination or phone changed since review");
        }
        self.submit_delayed_recovery(
            backend,
            RecoveryKind::PreviousVault,
            review_id,
            record,
            || load_device_keys(&self.data_dir, PHONE_DEVICE_FILE),
        )
    }

    pub fn reject_previous_vault_recovery(&self, review_id: &str) -> Result<()> {
        self.reject_delayed_recovery(RecoveryKind::PreviousVault, review_id)
    }

    fn previous_vault_path(&self, id: &str) -> Result<PathBuf> {
        if id.len() != 64
            || !id
                .bytes()
                .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
        {
            bail!("invalid retired vault ID");
        }
        Ok(self
            .data_dir
            .join("phone/previous-vaults")
            .join(format!("{id}.json")))
    }
}
