use super::*;
use anzen_common::{
    cloud_backup::{
        self, CloudBackupDraft, CloudRecoveryBackup, DeviceBackupKeyRequest,
        DeviceBackupKeyResponse, RecoveryPayload,
    },
    device_channel,
};

#[derive(Serialize, Deserialize)]
struct PendingBackup {
    vault_address: String,
    backup_id: String,
    draft: CloudBackupDraft,
}

impl WalletApplication {
    /// The signer receives only a random data-encryption key, never the seed or seed ciphertext.
    pub fn propose_backup(&self) -> Result<DeviceMessage> {
        let config = self.configuration()?;
        let phone = load_device_keys(&self.data_dir, PHONE_DEVICE_FILE)?;
        let hww = bitcoin::secp256k1::XOnlyPublicKey::from_str(&config.hww_vault_pubkey)?;
        let payload = zeroize::Zeroizing::new(RecoveryPayload::new(&config, &phone)?);
        let (draft, key) = cloud_backup::prepare_backup(&payload)?;
        let backup_id = sha256::Hash::hash(&serde_json::to_vec(&draft)?).to_string();
        let request = DeviceBackupKeyRequest {
            backup_id: backup_id.clone(),
            symmetric_key: *key,
        };
        storage::write_json(
            &self.data_dir.join("phone/pending-backup.json"),
            &PendingBackup {
                vault_address: config.vault_address.clone(),
                backup_id,
                draft,
            },
        )?;
        device_channel::seal(
            &phone,
            hww,
            &format!("cloud-backup-request/{}", config.vault_address),
            &request,
        )
    }

    pub fn store_backup(&self, message: &DeviceMessage) -> Result<()> {
        let config = self.configuration()?;
        let phone = load_device_keys(&self.data_dir, PHONE_DEVICE_FILE)?;
        let hww = bitcoin::secp256k1::XOnlyPublicKey::from_str(&config.hww_vault_pubkey)?;
        let response: DeviceBackupKeyResponse = device_channel::open(
            &phone,
            hww,
            &format!("cloud-backup-response/{}", config.vault_address),
            message,
        )?;
        let pending: PendingBackup =
            storage::read_json(&self.data_dir.join("phone/pending-backup.json"))?;
        if pending.vault_address != config.vault_address
            || pending.backup_id != response.backup_id
            || sha256::Hash::hash(&serde_json::to_vec(&pending.draft)?).to_string()
                != pending.backup_id
        {
            bail!("backup approval does not match the current phone backup request");
        }
        let backup = cloud_backup::complete_backup(pending.draft, response.wrapped_key)?;
        storage::write_json(&self.data_dir.join(storage::PHONE_BACKUP_FILE), &backup)
    }

    pub fn export_backup(&self) -> Result<CloudRecoveryBackup> {
        self.configuration()?;
        storage::read_json(&self.data_dir.join(storage::PHONE_BACKUP_FILE))
    }
}
