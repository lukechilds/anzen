use super::*;
use anzen_common::storage::fs::StoragePath;
use anzen_common::{
    device_channel,
    social::{self, DeviceBackupKeyRequest, DeviceBackupKeyResponse},
};

#[derive(Serialize, Deserialize)]
struct PendingBackup {
    config: VaultConfig,
    request_hash: String,
}

#[derive(Serialize)]
pub struct SetupAccepted {
    pub vault_address: String,
    pub backup: DeviceMessage,
}

#[derive(Serialize)]
pub struct BackupReview {
    pub review_id: String,
    pub vault_address: String,
    pub phone_public_key: String,
    pub friend_fingerprints: Vec<String>,
}

impl SignerApplication {
    /// Vault setup in one exchange. Holding the devices together is the user's consent: the
    /// vault cannot move funds before a reviewed policy, and the backup request is sealed by the
    /// phone key of the vault being accepted. A backup naming recovery contacts still needs
    /// its own review, so setup refuses one.
    pub fn accept_setup(
        &self,
        config: &VaultConfig,
        backup: &DeviceMessage,
    ) -> Result<SetupAccepted> {
        config.validate_public_policy()?;
        if !self
            .backup_key(config, backup)?
            .friend_public_keys
            .is_empty()
        {
            bail!("a backup with recovery contacts must be reviewed on the signer");
        }
        self.accept_vault(config)?;
        Ok(SetupAccepted {
            vault_address: config.vault_address.clone(),
            backup: self.wrap_backup_for_config(config, backup)?,
        })
    }

    pub fn review_backup(&self, message: &DeviceMessage) -> Result<BackupReview> {
        let config = self.configuration()?;
        let request = self.backup_key(&config, message)?;
        let friend_fingerprints = request.friend_fingerprints()?;
        let pending = PendingBackup {
            config,
            request_hash: sha256::Hash::hash(&serde_json::to_vec(message)?).to_string(),
        };
        let review_id = sha256::Hash::hash(&serde_json::to_vec(&pending)?).to_string();
        storage::write_json(&self.backup_review_path(&review_id)?, &pending)?;
        Ok(BackupReview {
            review_id,
            vault_address: pending.config.vault_address,
            phone_public_key: pending.config.phone_vault_pubkey,
            friend_fingerprints,
        })
    }

    pub fn approve_backup(
        &self,
        review_id: &str,
        message: &DeviceMessage,
    ) -> Result<DeviceMessage> {
        let config = self.configuration()?;
        let pending: PendingBackup = storage::read_json(&self.backup_review_path(review_id)?)?;
        if sha256::Hash::hash(&serde_json::to_vec(&pending)?).to_string() != review_id
            || serde_json::to_vec(&config)? != serde_json::to_vec(&pending.config)?
            || sha256::Hash::hash(&serde_json::to_vec(message)?).to_string() != pending.request_hash
        {
            bail!("backup request or vault changed since review");
        }
        self.wrap_backup_for_config(&config, message)
    }

    pub(super) fn wrap_backup_for_config(
        &self,
        config: &VaultConfig,
        message: &DeviceMessage,
    ) -> Result<DeviceMessage> {
        let request = self.backup_key(config, message)?;
        let hww = load_device_keys(&self.data_dir, HWW_DEVICE_FILE)?;
        let response = DeviceBackupKeyResponse {
            backup_id: request.backup_id.clone(),
            wrapped_key: social::wrap_backup_key(&hww.seed, &request.symmetric_key)?,
        };
        let phone = bitcoin::secp256k1::XOnlyPublicKey::from_str(&config.phone_vault_pubkey)?;
        device_channel::seal(
            &hww,
            phone,
            &format!("cloud-backup-response/{}", config.vault_address),
            &response,
        )
    }

    pub fn reject_backup(&self, review_id: &str) -> Result<()> {
        self.configuration()?;
        let path = self.backup_review_path(review_id)?;
        if path.storage_exists() {
            anzen_common::storage::fs::remove_file(path)?;
        }
        Ok(())
    }

    pub(super) fn backup_key(
        &self,
        config: &VaultConfig,
        message: &DeviceMessage,
    ) -> Result<DeviceBackupKeyRequest> {
        let hww = load_device_keys(&self.data_dir, HWW_DEVICE_FILE)?;
        let phone = bitcoin::secp256k1::XOnlyPublicKey::from_str(&config.phone_vault_pubkey)?;
        let request: DeviceBackupKeyRequest = device_channel::open(
            &hww,
            phone,
            &format!("cloud-backup-request/{}", config.vault_address),
            message,
        )?;
        if request.backup_id.len() != 64
            || !request
                .backup_id
                .bytes()
                .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
        {
            bail!("invalid backup commitment");
        }
        request.friend_fingerprints()?;
        Ok(request)
    }

    fn backup_review_path(&self, review_id: &str) -> Result<PathBuf> {
        self.review_path(review_id)?;
        Ok(self
            .data_dir
            .join("hww/backup-reviews")
            .join(format!("{review_id}.json")))
    }
}
