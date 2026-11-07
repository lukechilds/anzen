//! Device-separated social recovery. Only the replacement phone sees the cloud payload.
use super::*;
use anzen_common::storage::fs::StoragePath;
use anzen_common::{cloud_restore::RestoreKeyResponse, device_channel, social};
use bitcoin::secp256k1::XOnlyPublicKey;
use zeroize::Zeroizing;

#[derive(Serialize, Deserialize)]
struct PendingRestore {
    request: ContactRestoreRequest,
    backup: CloudRecoveryBackup,
}

#[derive(Serialize, Deserialize)]
struct ContactReviewRecord {
    request_id: String,
    contact_public_key: String,
}

#[derive(Serialize)]
pub struct ContactRestoreReview {
    pub review_id: String,
    pub recipient_public_key: String,
    pub contact_fingerprint: String,
    pub backup_id: String,
}

#[derive(Serialize, Deserialize)]
pub struct ContactRecoveryState {
    pub request_id: String,
    pub config: VaultConfig,
    pub recipient_public_key: String,
    pub spending_policy_available: bool,
}

#[derive(Serialize)]
pub struct ContactRecoveryStatus {
    pub recovery: ContactRecoveryState,
    pub balance_sats: u64,
    pub mature_balance_sats: u64,
    pub next_block_height: Option<u64>,
    pub blocks_remaining: Option<u64>,
}

impl WalletApplication {
    pub fn propose_contact_restore(
        &self,
        backup: &CloudRecoveryBackup,
        contact: &RecoveryContact,
    ) -> Result<ContactRestoreRequest> {
        let recipient = self.public_identity()?;
        if storage::config_exists(&self.data_dir) {
            bail!("use a fresh replacement phone without an existing vault for contact recovery");
        }
        contact.validate()?;
        if backup.version != 1 || backup.kind != "vault-cloud-recovery" {
            bail!("unsupported cloud recovery backup");
        }
        let friend = backup
            .friends
            .iter()
            .find(|friend| friend.fingerprint == contact.fingerprint)
            .context("this contact is not configured in the cloud backup")?
            .clone();
        let request = ContactRestoreRequest {
            version: 1,
            backup_id: sha256::Hash::hash(&serde_json::to_vec(backup)?).to_string(),
            recipient,
            contact_public_key: contact.transport_public_key.clone(),
            friend,
        };
        request.validate()?;
        storage::write_json(
            &self.data_dir.join("phone/pending-contact-restore.json"),
            &PendingRestore {
                request: request.clone(),
                backup: backup.clone(),
            },
        )?;
        Ok(request)
    }

    pub fn review_contact_restore(
        &self,
        request: &ContactRestoreRequest,
    ) -> Result<ContactRestoreReview> {
        request.validate()?;
        let identity = self.public_identity()?;
        if request.contact_public_key != identity.vault_pubkey {
            bail!("recovery request belongs to another contact device");
        }
        // Prove this device can open the wrapper, without receiving the encrypted phone seed.
        self.unwrap_contact_key(&request.friend)?;
        let record = ContactReviewRecord {
            request_id: request.id()?,
            contact_public_key: identity.vault_pubkey,
        };
        let review_id = sha256::Hash::hash(&serde_json::to_vec(&record)?).to_string();
        storage::write_json(&self.contact_review_path(&review_id)?, &record)?;
        Ok(ContactRestoreReview {
            review_id,
            recipient_public_key: request.recipient.vault_pubkey.clone(),
            contact_fingerprint: request.friend.fingerprint.clone(),
            backup_id: request.backup_id.clone(),
        })
    }

    pub fn approve_contact_restore(
        &self,
        review_id: &str,
        request: &ContactRestoreRequest,
    ) -> Result<DeviceMessage> {
        request.validate()?;
        let record: ContactReviewRecord =
            storage::read_json(&self.contact_review_path(review_id)?)?;
        let identity = self.public_identity()?;
        if sha256::Hash::hash(&serde_json::to_vec(&record)?).to_string() != review_id
            || record.request_id != request.id()?
            || record.contact_public_key != identity.vault_pubkey
            || request.contact_public_key != identity.vault_pubkey
        {
            bail!("contact recovery changed since review");
        }
        let key = self.unwrap_contact_key(&request.friend)?;
        let own = load_device_keys(&self.data_dir, PHONE_DEVICE_FILE)?;
        device_channel::seal(
            &own,
            request.recipient.parsed_vault_pubkey()?,
            &request.response_purpose()?,
            &RestoreKeyResponse {
                request_id: request.id()?,
                symmetric_key: *key,
            },
        )
    }

    pub fn reject_contact_restore(&self, review_id: &str) -> Result<()> {
        self.public_identity()?;
        match anzen_common::storage::fs::remove_file(self.contact_review_path(review_id)?) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e.into()),
        }
    }

    pub fn store_contact_restored_key(
        &self,
        message: &DeviceMessage,
    ) -> Result<ContactRecoveryState> {
        if storage::config_exists(&self.data_dir) {
            bail!("contact recovery cannot replace an existing vault");
        }
        let pending: PendingRestore =
            storage::read_json(&self.data_dir.join("phone/pending-contact-restore.json"))?;
        pending.request.validate()?;
        let own = load_device_keys(&self.data_dir, PHONE_DEVICE_FILE)?;
        if pending.request.recipient.vault_pubkey != own.vault_pubkey.to_string()
            || pending.request.backup_id
                != sha256::Hash::hash(&serde_json::to_vec(&pending.backup)?).to_string()
        {
            bail!("contact recovery recipient or backup changed");
        }
        let response: RestoreKeyResponse = device_channel::open(
            &own,
            XOnlyPublicKey::from_str(&pending.request.contact_public_key)?,
            &pending.request.response_purpose()?,
            message,
        )?;
        if response.request_id != pending.request.id()? {
            bail!("contact response belongs to a different restore request");
        }
        let payload = Zeroizing::new(social::decrypt_with_key(
            &pending.backup,
            &response.symmetric_key,
        )?);
        let mut config = payload.recovery_config()?;
        if config.bitcoin_network()? != self.public_identity()?.bitcoin_network()?
            || config.phone_vault_pubkey == own.vault_pubkey.to_string()
            || config.hww_vault_pubkey == own.vault_pubkey.to_string()
        {
            bail!("recover into a fresh independent phone on the backup's network");
        }
        // Keep a separately saved signed policy if available; do not invent history from a script.
        let bundle_path = self.data_dir.join("phone/recovery-bundle.json");
        let spending_policy_available = if bundle_path.storage_exists() {
            let bundle: RecoveryVaultBundle = storage::read_json(&bundle_path)?;
            bundle.validate()?;
            payload.validate_against(&bundle.config)?;
            config = bundle.config;
            true
        } else {
            false
        };
        self.store_recovered_payload(&config, &payload, &pending.backup.friends)?;
        self.import_recovery_vault(&config)?;
        let state = ContactRecoveryState {
            request_id: pending.request.id()?,
            config,
            recipient_public_key: own.vault_pubkey.to_string(),
            spending_policy_available,
        };
        storage::write_json(
            &self.data_dir.join("phone/contact-recovery-state.json"),
            &state,
        )?;
        Ok(state)
    }

    pub fn contact_recovery_state(&self) -> Result<ContactRecoveryState> {
        let state: ContactRecoveryState =
            storage::read_json(&self.data_dir.join("phone/contact-recovery-state.json"))?;
        if state.recipient_public_key != self.public_identity()?.vault_pubkey
            || serde_json::to_vec(&state.config)? != serde_json::to_vec(&self.recovery_source()?)?
        {
            bail!("contact recovery source or phone changed");
        }
        self.restored_keys(&state.config)?;
        Ok(state)
    }

    pub fn contact_recovery_status(
        &self,
        backend: &dyn HotWalletBackend,
    ) -> Result<ContactRecoveryStatus> {
        self.check_backend(backend)?;
        let recovery = self.contact_recovery_state()?;
        let inputs = backend.scan_vault(&recovery.config)?;
        let timing = crate::recovery::recovery_timing(
            &inputs,
            crate::recovery::SweepPath::PhoneRecovery,
            backend.chain_tip()?.height,
        )?;
        Ok(ContactRecoveryStatus {
            mature_balance_sats: timing.mature_sats,
            recovery,
            balance_sats: timing.total_sats,
            next_block_height: timing.earliest_next_block_height,
            blocks_remaining: timing.blocks_remaining,
        })
    }

    fn contact_review_path(&self, id: &str) -> Result<PathBuf> {
        if id.len() != 64
            || !id
                .bytes()
                .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
        {
            bail!("invalid contact recovery review ID");
        }
        Ok(self
            .data_dir
            .join("phone/contact-reviews")
            .join(format!("{id}.json")))
    }
}
