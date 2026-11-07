use super::*;
use anzen_common::{
    cloud_restore::RestoreKeyResponse,
    crypto, device_channel,
    keys::DeviceKeys,
    social::{self, RecoveryPayload},
};
use bitcoin::{key::Secp256k1, secp256k1::XOnlyPublicKey};
use zeroize::Zeroizing;

#[derive(Serialize, Deserialize)]
struct PendingRestore {
    config: VaultConfig,
    request: RestoreKeyRequest,
    backup: CloudRecoveryBackup,
}

/// One authenticated, atomic record binds the recovered seed to its verified contact list.
/// Legacy records contain only the flattened payload; they remain usable for fund recovery,
/// but must repeat backup authentication before a rotation can promise to preserve contacts.
#[derive(Serialize, Deserialize, zeroize::Zeroize)]
struct RestoredMaterial {
    #[serde(flatten)]
    payload: RecoveryPayload,
    #[serde(default)]
    friend_public_keys: Option<Vec<String>>,
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
        if backup.version != 1 || backup.kind != "vault-cloud-recovery" {
            bail!("unsupported cloud recovery backup");
        }
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
        let payload = Zeroizing::new(social::decrypt_with_key(
            &pending.backup,
            &response.symmetric_key,
        )?);
        self.store_recovered_payload(&config, &payload, &pending.backup.friends)?;
        self.restored_phone()
    }

    pub(super) fn store_recovered_payload(
        &self,
        config: &VaultConfig,
        payload: &RecoveryPayload,
        authenticated_friends: &[social::FriendKeyWrapper],
    ) -> Result<()> {
        let phone = load_device_keys(&self.data_dir, PHONE_DEVICE_FILE)?;
        payload.validate_against(config)?;
        // Both callers authenticate the complete friend manifest with the backup key first.
        let friend_public_keys = authenticated_friends
            .iter()
            .map(|friend| friend.public_key_armored.clone())
            .collect::<Vec<_>>();
        social::recovery_contact_fingerprints(&friend_public_keys)?;
        let material = Zeroizing::new(RestoredMaterial {
            payload: payload.clone(),
            friend_public_keys: Some(friend_public_keys),
        });
        let bytes = Zeroizing::new(serde_json::to_vec(&*material)?);
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
        let material = self.restored_material(config)?;
        DeviceKeys::parse_for_network_at_index(
            &Secp256k1::new(),
            &material.payload.phone_mnemonic,
            config.bitcoin_network()?,
            material.payload.phone_vault_key_index,
        )
    }

    pub(super) fn restored_contact_public_keys(&self, config: &VaultConfig) -> Result<Vec<String>> {
        let material = self.restored_material(config)?;
        let keys = material.friend_public_keys.as_ref()
            .context("repeat cloud backup recovery to authenticate contacts before rotating this legacy recovered key")?;
        social::recovery_contact_fingerprints(keys)?;
        Ok(keys.clone())
    }

    fn restored_material(&self, config: &VaultConfig) -> Result<Zeroizing<RestoredMaterial>> {
        let phone = load_device_keys(&self.data_dir, PHONE_DEVICE_FILE)?;
        let encrypted: crypto::EncryptedBlob = storage::read_json(&self.restored_key_path(config))?;
        let bytes = crypto::decrypt(&phone.seed, &self.restored_key_purpose(config), &encrypted)?;
        let material = Zeroizing::new(serde_json::from_slice::<RestoredMaterial>(&bytes)?);
        material.payload.validate_against(config)?;
        Ok(material)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legacy_recovered_keys_remain_usable_but_cannot_assume_an_empty_contact_list() {
        let original = anzen_common::storage::tempdir().unwrap();
        let config = crate::test_support::initialize(original.path())
            .unwrap()
            .config;
        let old_phone = load_device_keys(original.path(), PHONE_DEVICE_FILE).unwrap();
        let payload = Zeroizing::new(RecoveryPayload::new(&config, &old_phone).unwrap());
        let directory = anzen_common::storage::tempdir().unwrap();
        let app = WalletApplication::new(directory.path().into(), false);
        app.initialize(Network::Regtest).unwrap();
        let recipient = load_device_keys(directory.path(), PHONE_DEVICE_FILE).unwrap();
        let bytes = Zeroizing::new(serde_json::to_vec(&*payload).unwrap());
        let legacy =
            crypto::encrypt(&recipient.seed, &app.restored_key_purpose(&config), &bytes).unwrap();
        storage::write_json(&app.restored_key_path(&config), &legacy).unwrap();
        assert_eq!(
            app.restored_keys(&config).unwrap().vault_pubkey,
            old_phone.vault_pubkey
        );
        assert!(
            app.restored_contact_public_keys(&config)
                .unwrap_err()
                .to_string()
                .contains("repeat cloud backup recovery")
        );
        app.store_recovered_payload(&config, &payload, &[]).unwrap();
        assert!(
            app.restored_contact_public_keys(&config)
                .unwrap()
                .is_empty()
        );
        assert_eq!(
            app.restored_keys(&config).unwrap().vault_pubkey,
            old_phone.vault_pubkey
        );
        assert_eq!(
            app.public_identity().unwrap().vault_pubkey,
            recipient.vault_pubkey.to_string()
        );
    }
}
