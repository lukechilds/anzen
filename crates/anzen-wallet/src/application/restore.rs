use super::*;
use anzen_common::{
    cloud_backup::{self, RecoveryPayload},
    cloud_restore::RestoreKeyResponse,
    crypto, device_channel,
    keys::DeviceKeys,
};
use bitcoin::{key::Secp256k1, secp256k1::XOnlyPublicKey};
use zeroize::Zeroizing;

#[derive(Serialize, Deserialize)]
struct PendingRestore {
    config: VaultConfig,
    request: RestoreKeyRequest,
    backup: CloudRecoveryBackup,
}

#[derive(Serialize)]
pub struct RestoredPhoneSummary {
    pub vault_address: String,
    pub old_phone_public_key: String,
    pub new_phone_public_key: String,
    pub requires_rotation: bool,
}

impl WalletApplication {
    pub fn propose_restore(&self, backup: &CloudRecoveryBackup) -> Result<RestoreKeyRequest> {
        let config = self.recovery_source()?;
        cloud_backup::validate_backup(backup)?;
        let request = RestoreKeyRequest {
            version: 1,
            vault_descriptor: config.vault_descriptor.clone(),
            backup_id: sha256::Hash::hash(&serde_json::to_vec(backup)?).to_string(),
            recipient: self.public_identity()?,
            wrapped_key: backup.hww_encrypted_symmetric_key.clone(),
        };
        request.validate(&config)?;
        storage::write_json(
            &self.data_dir.join("phone/pending-restore.json"),
            &PendingRestore {
                config,
                request: request.clone(),
                backup: backup.clone(),
            },
        )?;
        Ok(request)
    }

    pub fn store_restored_key(&self, message: &DeviceMessage) -> Result<RestoredPhoneSummary> {
        let config = self.recovery_source()?;
        let pending: PendingRestore =
            storage::read_json(&self.data_dir.join("phone/pending-restore.json"))?;
        pending.request.validate(&config)?;
        if serde_json::to_vec(&pending.config)? != serde_json::to_vec(&config)?
            || pending.request.recipient.vault_pubkey != self.public_identity()?.vault_pubkey
            || pending.request.backup_id
                != sha256::Hash::hash(&serde_json::to_vec(&pending.backup)?).to_string()
            || pending.request.wrapped_key != pending.backup.hww_encrypted_symmetric_key
        {
            bail!("phone, recovery source or cloud backup changed since the restore request");
        }
        let phone = load_device_keys(&self.data_dir, PHONE_DEVICE_FILE)?;
        let response: RestoreKeyResponse = device_channel::open(
            &phone,
            XOnlyPublicKey::from_str(&config.hww_vault_pubkey)?,
            &pending.request.response_purpose()?,
            message,
        )?;
        if response.request_id != pending.request.id()? {
            bail!("restore approval belongs to another request");
        }
        let payload = Zeroizing::new(cloud_backup::decrypt_with_key(
            &pending.backup,
            &response.symmetric_key,
        )?);
        self.store_recovered_payload(&config, &payload)?;
        self.restored_phone()
    }

    pub(super) fn store_recovered_payload(
        &self,
        config: &VaultConfig,
        payload: &RecoveryPayload,
    ) -> Result<()> {
        let phone = load_device_keys(&self.data_dir, PHONE_DEVICE_FILE)?;
        payload.validate_against(config)?;
        let bytes = Zeroizing::new(serde_json::to_vec(payload)?);
        let encrypted = crypto::encrypt(&phone.seed, &self.restored_key_purpose(config), &bytes)?;
        storage::write_json(&self.restored_key_path(config), &encrypted)
    }

    pub fn restored_phone(&self) -> Result<RestoredPhoneSummary> {
        let config = self.recovery_source()?;
        let restored = self.restored_keys(&config)?;
        Ok(RestoredPhoneSummary {
            vault_address: config.vault_address,
            old_phone_public_key: restored.vault_pubkey.to_string(),
            new_phone_public_key: self.public_identity()?.vault_pubkey,
            requires_rotation: true,
        })
    }

    pub(super) fn restored_keys(&self, config: &VaultConfig) -> Result<DeviceKeys> {
        let phone = load_device_keys(&self.data_dir, PHONE_DEVICE_FILE)?;
        let encrypted: crypto::EncryptedBlob = storage::read_json(&self.restored_key_path(config))?;
        let bytes = crypto::decrypt(&phone.seed, &self.restored_key_purpose(config), &encrypted)?;
        let payload = Zeroizing::new(serde_json::from_slice::<RecoveryPayload>(&bytes)?);
        payload.validate_against(config)?;
        DeviceKeys::parse_for_network_at_index(
            &Secp256k1::new(),
            &payload.phone_mnemonic,
            config.bitcoin_network()?,
            payload.phone_vault_key_index,
        )
    }

    fn restored_key_purpose(&self, config: &VaultConfig) -> String {
        format!("anzen/restored-phone-key/v1/{}", config.vault_address)
    }

    fn restored_key_path(&self, config: &VaultConfig) -> PathBuf {
        self.data_dir.join("phone/restored-keys").join(format!(
            "{}.json",
            sha256::Hash::hash(config.vault_descriptor.as_bytes())
        ))
    }
}
