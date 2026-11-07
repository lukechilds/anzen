use super::*;
use anzen_common::{cloud_restore::RestoreKeyResponse, device_channel, social};

#[derive(Serialize, Deserialize)]
struct PendingRestore {
    config: VaultConfig,
    request: RestoreKeyRequest,
}

#[derive(Serialize)]
pub struct RestoreReview {
    pub review_id: String,
    pub vault_address: String,
    pub backup_id: String,
    pub recipient_public_key: String,
}

impl SignerApplication {
    pub fn review_restore(&self, request: &RestoreKeyRequest) -> Result<RestoreReview> {
        let config = self.configuration()?;
        request.validate(&config)?;
        let hww = load_device_keys(&self.data_dir, HWW_DEVICE_FILE)?;
        social::unwrap_backup_key(&hww.seed, &request.wrapped_key)?;
        let pending = PendingRestore {
            config,
            request: request.clone(),
        };
        let review_id = sha256::Hash::hash(&serde_json::to_vec(&pending)?).to_string();
        // Wrapped key and public identity only. No phone payload, seed ciphertext or DEK.
        storage::write_json(&self.restore_review_path(&review_id)?, &pending)?;
        Ok(RestoreReview {
            review_id,
            vault_address: pending.config.vault_address,
            backup_id: request.backup_id.clone(),
            recipient_public_key: request.recipient.vault_pubkey.clone(),
        })
    }

    pub fn approve_restore(&self, review_id: &str) -> Result<DeviceMessage> {
        let config = self.configuration()?;
        let pending: PendingRestore = storage::read_json(&self.restore_review_path(review_id)?)?;
        if sha256::Hash::hash(&serde_json::to_vec(&pending)?).to_string() != review_id
            || serde_json::to_vec(&pending.config)? != serde_json::to_vec(&config)?
        {
            bail!("restore request or vault changed since review");
        }
        pending.request.validate(&config)?;
        let hww = load_device_keys(&self.data_dir, HWW_DEVICE_FILE)?;
        let key = social::unwrap_backup_key(&hww.seed, &pending.request.wrapped_key)?;
        let response = RestoreKeyResponse {
            request_id: pending.request.id()?,
            symmetric_key: *key,
        };
        device_channel::seal(
            &hww,
            pending.request.recipient.parsed_vault_pubkey()?,
            &pending.request.response_purpose()?,
            &response,
        )
    }

    pub fn reject_restore(&self, review_id: &str) -> Result<()> {
        self.configuration()?;
        match anzen_common::storage::fs::remove_file(self.restore_review_path(review_id)?) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e.into()),
        }
    }

    fn restore_review_path(&self, review_id: &str) -> Result<PathBuf> {
        self.review_path(review_id)?;
        Ok(self
            .data_dir
            .join("hww/restore-reviews")
            .join(format!("{review_id}.json")))
    }
}
