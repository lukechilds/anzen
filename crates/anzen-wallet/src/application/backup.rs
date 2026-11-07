use super::*;
use anzen_common::storage::fs::StoragePath;
use anzen_common::{
    device_channel,
    social::{
        self, CloudBackupDraft, CloudRecoveryBackup, DeviceBackupKeyRequest,
        DeviceBackupKeyResponse, RecoveryPayload,
    },
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
        self.propose_backup_for_contacts(self.backup_contact_keys()?)
    }

    pub fn propose_backup_with_contact(&self, contact: &RecoveryContact) -> Result<DeviceMessage> {
        contact.validate()?;
        let mut keys = self.backup_contact_keys()?;
        for key in &keys {
            if social::friend_fingerprint(key.as_bytes())? == contact.fingerprint {
                bail!("recovery contact is already configured");
            }
        }
        keys.push(contact.public_key_armored.clone());
        self.propose_backup_for_contacts(keys)
    }

    fn backup_contact_keys(&self) -> Result<Vec<String>> {
        self.configuration()?;
        let path = self.data_dir.join(storage::PHONE_BACKUP_FILE);
        if !path.storage_exists() {
            return Ok(Vec::new());
        }
        Ok(social::friend_public_keys(&self.export_backup()?))
    }

    fn propose_backup_for_contacts(
        &self,
        friend_public_keys: Vec<String>,
    ) -> Result<DeviceMessage> {
        let config = self.configuration()?;
        let phone = load_device_keys(&self.data_dir, PHONE_DEVICE_FILE)?;
        let hww = bitcoin::secp256k1::XOnlyPublicKey::from_str(&config.hww_vault_pubkey)?;
        // Bound untrusted OpenPGP parsing/encryption before doing any expensive work.
        let mut request = DeviceBackupKeyRequest {
            backup_id: String::new(),
            symmetric_key: [0; 32],
            friend_public_keys,
        };
        request.friend_fingerprints()?;
        let payload = zeroize::Zeroizing::new(RecoveryPayload::new(&config, &phone)?);
        let (draft, key) = social::prepare_backup(&payload, &request.friend_public_keys)?;
        let backup_id = sha256::Hash::hash(&serde_json::to_vec(&draft)?).to_string();
        request.backup_id = backup_id.clone();
        request.symmetric_key = *key;
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
        let backup = social::complete_backup(pending.draft, response.wrapped_key)?;
        storage::write_json(&self.data_dir.join(storage::PHONE_BACKUP_FILE), &backup)
    }

    pub fn export_backup(&self) -> Result<CloudRecoveryBackup> {
        self.configuration()?;
        storage::read_json(&self.data_dir.join(storage::PHONE_BACKUP_FILE))
    }
}
