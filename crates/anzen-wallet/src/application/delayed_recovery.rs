//! Shared reviewed phone-recovery transfers. Callers validate the recovery source and key;
//! this module owns bounded selection, exact review binding, maturity, submission and retries.
use super::rotation::broadcast_once;
use super::*;
use crate::recovery::{self, PhoneRecoveryTransfer, SweepPath};
use anzen_common::storage::fs::StoragePath;
use anzen_common::{PHONE_RECOVERY_BLOCKS, keys::DeviceKeys};
use bitcoin::Transaction;

#[derive(Clone, Copy)]
pub(super) enum RecoveryKind {
    Contact,
    PreviousVault,
}

#[derive(Serialize, Deserialize)]
pub(super) struct RecoveryRecord {
    pub config: VaultConfig,
    pub recipient_public_key: String,
    pub destination: String,
    transfer: PhoneRecoveryTransfer,
}

#[derive(Serialize)]
pub struct RecoveryTransferReview {
    pub review_id: String,
    pub source_address: String,
    pub destination: String,
    pub amount_sats: u64,
    pub fee_sats: u64,
    pub input_count: usize,
    pub blocks_remaining: u64,
}

impl WalletApplication {
    pub(super) fn prepare_delayed_recovery(
        &self,
        backend: &dyn HotWalletBackend,
        config: VaultConfig,
        destination: &str,
        kind: RecoveryKind,
    ) -> Result<RecoveryTransferReview> {
        self.check_backend(backend)?;
        config.validate_public_policy()?;
        let all_inputs = backend.scan_vault(&config)?;
        let tip = backend.chain_tip()?.height;
        let inputs = recovery::recovery_review_inputs(&all_inputs, SweepPath::PhoneRecovery, tip);
        if inputs.is_empty() {
            bail!("no confirmed vault inputs to recover");
        }
        let destination = Address::from_str(destination)?.require_network(backend.network())?;
        let transfer = PhoneRecoveryTransfer::prepare(&config, &inputs, &destination)?;
        let amount_sats = transfer.amount_sats;
        let blocks_remaining = inputs
            .iter()
            .map(|u| {
                u.confirmation_height
                    .saturating_add(u64::from(PHONE_RECOVERY_BLOCKS))
            })
            .max()
            .context("no confirmed vault inputs to recover")?
            .saturating_sub(tip.saturating_add(1));
        let source_address = config.vault_address.clone();
        let record = RecoveryRecord {
            config,
            recipient_public_key: self.public_identity()?.vault_pubkey,
            destination: destination.to_string(),
            transfer,
        };
        let review_id = sha256::Hash::hash(&serde_json::to_vec(&record)?).to_string();
        storage::write_json(
            &self.delayed_recovery_path(kind, &review_id, "review")?,
            &record,
        )?;
        Ok(RecoveryTransferReview {
            review_id,
            source_address,
            destination: destination.to_string(),
            amount_sats,
            fee_sats: record.transfer.fee_sats,
            input_count: inputs.len(),
            blocks_remaining,
        })
    }

    pub(super) fn delayed_recovery_record(
        &self,
        kind: RecoveryKind,
        id: &str,
    ) -> Result<RecoveryRecord> {
        let record: RecoveryRecord =
            storage::read_json(&self.delayed_recovery_path(kind, id, "review")?)?;
        if sha256::Hash::hash(&serde_json::to_vec(&record)?).to_string() != id
            || record.recipient_public_key != self.public_identity()?.vault_pubkey
        {
            bail!("recovery review or recipient changed since review");
        }
        record.config.validate_public_policy()?;
        Ok(record)
    }

    pub(super) fn submit_delayed_recovery(
        &self,
        backend: &dyn HotWalletBackend,
        kind: RecoveryKind,
        id: &str,
        record: RecoveryRecord,
        load_key: impl FnOnce() -> Result<DeviceKeys>,
    ) -> Result<Txid> {
        self.check_backend(backend)?;
        let submitted = self.delayed_recovery_path(kind, id, "submitted")?;
        let transaction: Transaction = if submitted.storage_exists() {
            storage::read_json(&submitted)?
        } else {
            let live = backend.scan_vault(&record.config)?;
            let next = backend.chain_tip()?.height.saturating_add(1);
            for input in &record.transfer.inputs {
                let coin = live
                    .iter()
                    .find(|u| u.outpoint == input.outpoint)
                    .context("recovery input is no longer confirmed and unspent")?;
                let valid = coin
                    .confirmation_height
                    .saturating_add(u64::from(PHONE_RECOVERY_BLOCKS));
                if next < valid {
                    bail!("phone recovery is not mature; input requires next-block height {valid}");
                }
            }
            let signed = record.transfer.sign(&record.config, &load_key()?)?;
            storage::write_json(&submitted, &signed)?;
            signed
        };
        if transaction.compute_txid() != record.transfer.txid {
            bail!("submitted recovery transaction differs from the review");
        }
        broadcast_once(
            backend,
            &self.delayed_recovery_path(kind, id, "receipt")?,
            &transaction,
        )
    }

    pub(super) fn reject_delayed_recovery(&self, kind: RecoveryKind, id: &str) -> Result<()> {
        self.public_identity()?;
        if self
            .delayed_recovery_path(kind, id, "submitted")?
            .storage_exists()
        {
            bail!("a submitted transaction cannot be cancelled by deleting its local review");
        }
        match anzen_common::storage::fs::remove_file(
            self.delayed_recovery_path(kind, id, "review")?,
        ) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e.into()),
        }
    }

    fn delayed_recovery_path(&self, kind: RecoveryKind, id: &str, suffix: &str) -> Result<PathBuf> {
        if id.len() != 64
            || !id
                .bytes()
                .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
        {
            bail!("invalid recovery review ID");
        }
        let directory = match kind {
            RecoveryKind::Contact => "phone/contact-sweeps",
            RecoveryKind::PreviousVault => "phone/previous-vault-sweeps",
        };
        Ok(self
            .data_dir
            .join(directory)
            .join(id)
            .join(format!("{suffix}.json")))
    }
}
