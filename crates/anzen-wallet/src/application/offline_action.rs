//! A new phone can coordinate HWW recovery using public vault data, without the lost phone key.
use super::*;
use crate::{
    offline_action,
    recovery::{self, RecoveryClock},
};
use anzen_common::storage::fs::StoragePath;

#[derive(Serialize, Deserialize)]
struct PendingAction {
    config: VaultConfig,
    request: OfflineActionRequest,
}

#[derive(Serialize)]
pub struct RecoverySnapshot {
    pub vault_address: String,
    pub chain_height: u64,
    pub median_time: u64,
    pub utxo_count: usize,
    pub hww: RecoveryTiming,
}

impl WalletApplication {
    pub fn import_recovery_bundle(&self, bundle: &RecoveryVaultBundle) -> Result<()> {
        bundle.validate()?;
        if bundle.config.bitcoin_network()? != self.public_identity()?.bitcoin_network()? {
            bail!("recovery vault is on a different network");
        }
        storage::write_json(&self.data_dir.join("phone/recovery-bundle.json"), bundle)?;
        self.import_recovery_vault(&bundle.config)
    }

    pub fn import_recovery_vault(&self, config: &VaultConfig) -> Result<()> {
        config.validate_public_policy()?;
        if config.bitcoin_network()? != self.public_identity()?.bitcoin_network()? {
            bail!("recovery vault is on a different network");
        }
        // Watch-only source; never replace this phone's own key, vault, policy or wallet state.
        storage::write_json(&self.data_dir.join("phone/recovery-source.json"), config)
    }

    pub fn recovery_source(&self) -> Result<VaultConfig> {
        let path = self.data_dir.join("phone/recovery-source.json");
        let config: VaultConfig = if path.storage_exists() {
            storage::read_json(&path)?
        } else {
            self.configuration()?
        };
        config.validate_public_policy()?;
        if config.bitcoin_network()? != self.public_identity()?.bitcoin_network()? {
            bail!("recovery vault is on a different network");
        }
        Ok(config)
    }

    pub fn recovery_snapshot(&self, backend: &dyn HotWalletBackend) -> Result<RecoverySnapshot> {
        self.check_backend(backend)?;
        let config = self.recovery_source()?;
        let tip = backend.chain_tip()?;
        let utxos = backend.scan_vault(&config)?;
        let clock = RecoveryClock::read(backend, &tip, &utxos)?;
        Ok(RecoverySnapshot {
            vault_address: config.vault_address,
            chain_height: tip.height,
            median_time: tip.median_time,
            utxo_count: utxos.len(),
            hww: recovery::recovery_timing(&utxos, &clock)?,
        })
    }

    pub fn propose_offline_action(
        &self,
        backend: &dyn HotWalletBackend,
        action: OfflineAction,
        destination: &str,
    ) -> Result<OfflineActionRequest> {
        self.check_backend(backend)?;
        let config = self.recovery_source()?;
        let utxos = match action {
            OfflineAction::HwwRecovery => {
                let utxos = backend.scan_vault(&config)?;
                let clock = RecoveryClock::read(backend, &backend.chain_tip()?, &utxos)?;
                recovery::recovery_review_inputs(&utxos, &clock)?
            }
            OfflineAction::RevokeActions => backend.scan_connectors(&config)?,
        };
        if utxos.is_empty() {
            bail!(match action {
                OfflineAction::HwwRecovery => "no confirmed vault inputs for HWW recovery",
                OfflineAction::RevokeActions => "no live controller inputs to revoke",
            });
        }
        let request = offline_action::request(&config, action, &utxos, destination)?;
        let id = sha256::Hash::hash(&serde_json::to_vec(&request)?).to_string();
        storage::write_json(
            &self.action_proposal_path(&id),
            &PendingAction {
                config,
                request: request.clone(),
            },
        )?;
        Ok(request)
    }

    pub fn activate_offline_action(
        &self,
        backend: &dyn HotWalletBackend,
        approval: &OfflineActionApproval,
    ) -> Result<Txid> {
        self.check_backend(backend)?;
        let config = self.recovery_source()?;
        let id = sha256::Hash::hash(&serde_json::to_vec(&approval.request)?).to_string();
        let pending: PendingAction = storage::read_json(&self.action_proposal_path(&id))
            .context("this phone did not propose the approved offline action")?;
        if serde_json::to_vec(&pending.config)? != serde_json::to_vec(&config)?
            || pending.request != approval.request
        {
            bail!("recovery source or action changed after proposal");
        }
        let transaction = offline_action::finalize(&config, approval)?;
        // Persist the exact accepted txid for idempotent retries without a second transfer.
        let receipt = self.action_proposal_path(&format!("{id}-broadcast"));
        if receipt.storage_exists() {
            let previous: Txid = storage::read_json(&receipt)?;
            if previous != transaction.compute_txid() {
                bail!("offline action receipt mismatch");
            }
            return Ok(previous);
        }
        let submitted = self.action_proposal_path(&format!("{id}-submitted"));
        if submitted.storage_exists() {
            let previous: Txid = storage::read_json(&submitted)?;
            if previous != transaction.compute_txid() {
                bail!("offline action submission mismatch");
            }
        } else if approval.request.action == OfflineAction::HwwRecovery {
            let live = backend.scan_vault(&config)?;
            let mut inputs = Vec::new();
            for input in &approval.request.inputs {
                let outpoint = anzen_common::protocol::bitcoin_outpoint(input.outpoint);
                inputs.push(
                    live.iter()
                        .find(|u| u.outpoint == outpoint)
                        .context("recovery input is no longer confirmed and unspent")?
                        .clone(),
                );
            }
            let clock = RecoveryClock::read(backend, &backend.chain_tip()?, &inputs)?;
            for input in &inputs {
                let opens_at = clock.opens_at(input)?;
                if clock.median_time < opens_at {
                    bail!(
                        "HWW recovery is not mature; an input's time lock opens at median time {opens_at}"
                    );
                }
            }
        }
        // An interrupted broadcast can be retried even after its inputs disappear from scans.
        // This is an intent, not evidence of acceptance: the backend still must accept this tx.
        storage::write_json(&submitted, &transaction.compute_txid())?;
        let txid = backend.broadcast(&transaction)?;
        if txid != transaction.compute_txid() {
            bail!("backend returned an unexpected offline action transaction ID");
        }
        storage::write_json(&receipt, &txid)?;
        Ok(txid)
    }

    fn action_proposal_path(&self, id: &str) -> PathBuf {
        // IDs are computed locally, never interpolated from a caller-supplied path.
        self.data_dir
            .join("phone/action-proposals")
            .join(format!("{id}.json"))
    }
}
