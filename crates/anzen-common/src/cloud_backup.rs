//! The versioned, encrypted phone backup kept in the user's cloud account.
//!
//! The phone mnemonic and public vault descriptor are encrypted with a random symmetric key, and
//! only the hardware wallet can unwrap that key. A lost phone is restored from this backup with
//! the signer; a lost signer is restored from its own maker's backup, which yields the same key.

use super::{
    crypto::{self, EncryptedBlob},
    keys::DeviceKeys,
    storage::VaultConfig,
};
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

const PAYLOAD_PURPOSE: &str = "cloud/vault-recovery-payload/v1";
const HWW_KEY_PURPOSE: &str = "cloud/vault-recovery-key/hww/v1";
const BACKUP_VERSION: u8 = 2;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, zeroize::Zeroize)]
pub struct RecoveryPayload {
    pub version: u8,
    pub kind: String,
    pub network: String,
    pub phone_mnemonic: String,
    #[serde(default)]
    pub phone_vault_key_index: u32,
    pub phone_vault_pubkey: String,
    pub vault_descriptor: String,
    pub vault_address: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CloudRecoveryBackup {
    pub version: u8,
    pub kind: String,
    pub encrypted_payload: EncryptedBlob,
    pub hww_encrypted_symmetric_key: EncryptedBlob,
}

/// Only the random wrapping key crosses the device channel. The seed ciphertext stays on the
/// phone/cloud side: a stolen signer plus its protocol transcript must not reveal the phone seed.
#[derive(Clone, Serialize, Deserialize, zeroize::Zeroize, zeroize::ZeroizeOnDrop)]
#[serde(deny_unknown_fields)]
pub struct DeviceBackupKeyRequest {
    pub backup_id: String,
    pub symmetric_key: [u8; 32],
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeviceBackupKeyResponse {
    pub backup_id: String,
    pub wrapped_key: EncryptedBlob,
}

#[derive(Serialize, Deserialize)]
pub struct CloudBackupDraft {
    pub encrypted_payload: EncryptedBlob,
}

impl CloudRecoveryBackup {
    /// Checks the envelope's version and kind; its contents are authenticated on decryption.
    pub fn validate(&self) -> Result<()> {
        if self.version != BACKUP_VERSION || self.kind != "vault-cloud-recovery" {
            bail!("unsupported cloud recovery backup");
        }
        Ok(())
    }
}

impl RecoveryPayload {
    /// Reconstruct only the on-chain recovery policy from a self-contained backup. Spending
    /// limits are not encoded by this descriptor: callers must report them as unavailable.
    pub fn recovery_config(&self) -> Result<VaultConfig> {
        use super::{HWW_RECOVERY_SECONDS, policy::VaultPolicy, storage};
        use bitcoin::{key::Secp256k1, secp256k1::XOnlyPublicKey};
        use miniscript::ForEachKey;
        let network = storage::parse_network_name(&self.network)?;
        storage::validate_supported_network(network)?;
        let secp = Secp256k1::new();
        let phone = DeviceKeys::parse_for_network_at_index(
            &secp,
            &self.phone_mnemonic,
            network,
            self.phone_vault_key_index,
        )?;
        let policy = VaultPolicy::from_descriptor_for_network(&self.vault_descriptor, network)?;
        let mut keys = std::collections::BTreeSet::new();
        policy.descriptor.for_each_key(|key| {
            keys.insert(key.to_string());
            true
        });
        let (external, internal) = phone.hot_descriptors(&secp)?;
        for key in keys {
            let Ok(hww) = key.parse::<XOnlyPublicKey>() else {
                continue;
            };
            let config = VaultConfig {
                version: storage::VAULT_CONFIG_VERSION,
                network: self.network.clone(),
                phone_vault_pubkey: phone.vault_pubkey.to_string(),
                hww_vault_pubkey: hww.to_string(),
                phone_hot_external_descriptor: external.clone(),
                phone_hot_internal_descriptor: internal.clone(),
                vault_descriptor: self.vault_descriptor.clone(),
                vault_address: self.vault_address.clone(),
                hww_recovery_seconds: HWW_RECOVERY_SECONDS,
                // Unknown, not evidence that the old spending policy was disabled.
                monthly_limit_sats: 0,
                emergency_access_limit_sats: 0,
            };
            if config.validate_public_policy().is_ok() {
                self.validate_against(&config)?;
                return Ok(config);
            }
        }
        bail!("backup descriptor is not the fixed Anzen policy for its phone key")
    }

    pub fn new(config: &VaultConfig, phone: &DeviceKeys) -> Result<Self> {
        if phone.vault_pubkey.to_string() != config.phone_vault_pubkey {
            bail!("phone key does not match the configured vault policy");
        }
        Ok(Self {
            version: 1,
            kind: "vault-recovery-payload".to_owned(),
            network: config.network.clone(),
            phone_mnemonic: phone.mnemonic.to_string(),
            phone_vault_key_index: phone.vault_key_index,
            phone_vault_pubkey: phone.vault_pubkey.to_string(),
            vault_descriptor: config.vault_descriptor.clone(),
            vault_address: config.vault_address.clone(),
        })
    }

    pub fn validate_against(&self, config: &VaultConfig) -> Result<()> {
        if self.version != 1
            || self.kind != "vault-recovery-payload"
            || self.network != config.network
            || self.phone_vault_pubkey != config.phone_vault_pubkey
            || self.vault_descriptor != config.vault_descriptor
            || self.vault_address != config.vault_address
        {
            bail!("recovery payload does not match the configured vault");
        }
        let phone = DeviceKeys::parse_for_network_at_index(
            &bitcoin::key::Secp256k1::new(),
            &self.phone_mnemonic,
            config.bitcoin_network()?,
            self.phone_vault_key_index,
        )?;
        if phone.vault_pubkey.to_string() != self.phone_vault_pubkey {
            bail!("recovery payload mnemonic does not match its phone public key");
        }
        Ok(())
    }
}

pub fn create_backup(payload: &RecoveryPayload, hww_seed: &[u8]) -> Result<CloudRecoveryBackup> {
    let (draft, key) = prepare_backup(payload)?;
    complete_backup(draft, wrap_backup_key(hww_seed, &key)?)
}

pub fn prepare_backup(
    payload: &RecoveryPayload,
) -> Result<(CloudBackupDraft, zeroize::Zeroizing<[u8; 32]>)> {
    let symmetric_key = crypto::random_key();
    let payload_bytes = zeroize::Zeroizing::new(serde_json::to_vec(payload)?);
    let encrypted_payload = crypto::encrypt(&*symmetric_key, PAYLOAD_PURPOSE, &payload_bytes)?;
    Ok((CloudBackupDraft { encrypted_payload }, symmetric_key))
}

pub fn wrap_backup_key(hww_seed: &[u8], key: &[u8; 32]) -> Result<EncryptedBlob> {
    let wrapped = crypto::encrypt(hww_seed, HWW_KEY_PURPOSE, key)?;
    if crypto::decrypt(hww_seed, HWW_KEY_PURPOSE, &wrapped)?.as_slice() != key {
        bail!("backup key wrapping did not round-trip");
    }
    Ok(wrapped)
}

pub fn complete_backup(
    draft: CloudBackupDraft,
    hww_encrypted_symmetric_key: EncryptedBlob,
) -> Result<CloudRecoveryBackup> {
    if hww_encrypted_symmetric_key.version != 1
        || hww_encrypted_symmetric_key.purpose != HWW_KEY_PURPOSE
        || hww_encrypted_symmetric_key.ciphertext.len() != 48
    {
        bail!("invalid wrapped backup key");
    }
    Ok(CloudRecoveryBackup {
        version: BACKUP_VERSION,
        kind: "vault-cloud-recovery".to_owned(),
        encrypted_payload: draft.encrypted_payload,
        hww_encrypted_symmetric_key,
    })
}

pub fn decrypt_with_hww(backup: &CloudRecoveryBackup, hww_seed: &[u8]) -> Result<RecoveryPayload> {
    let symmetric_key = unwrap_backup_key(hww_seed, &backup.hww_encrypted_symmetric_key)?;
    decrypt_with_key(backup, &symmetric_key)
}

/// Used by an offline signer: the encrypted phone payload is deliberately not an argument.
pub fn unwrap_backup_key(
    hww_seed: &[u8],
    wrapped: &EncryptedBlob,
) -> Result<zeroize::Zeroizing<[u8; 32]>> {
    let plaintext = crypto::decrypt(hww_seed, HWW_KEY_PURPOSE, wrapped)?;
    let key: [u8; 32] = plaintext
        .as_slice()
        .try_into()
        .context("backup wrapper contains an invalid symmetric key")?;
    Ok(zeroize::Zeroizing::new(key))
}

/// Authenticate the cloud envelope before returning the descriptor-bound payload.
pub fn decrypt_with_key(backup: &CloudRecoveryBackup, key: &[u8; 32]) -> Result<RecoveryPayload> {
    validate_backup(backup)?;
    decrypt_payload(backup, key)
}

fn decrypt_payload(backup: &CloudRecoveryBackup, symmetric_key: &[u8]) -> Result<RecoveryPayload> {
    let plaintext = crypto::decrypt(symmetric_key, PAYLOAD_PURPOSE, &backup.encrypted_payload)?;
    serde_json::from_slice(&plaintext).context("decrypted cloud recovery payload is invalid")
}

pub fn validate_backup(backup: &CloudRecoveryBackup) -> Result<()> {
    backup.validate()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A paired vault's public configuration and both device keys, without any device app.
    fn fixture() -> (VaultConfig, DeviceKeys, DeviceKeys) {
        let secp = bitcoin::key::Secp256k1::new();
        let phone = DeviceKeys::generate(&secp).unwrap();
        let hww = DeviceKeys::generate(&secp).unwrap();
        (config_for(&phone, &hww), phone, hww)
    }

    fn config_for(phone: &DeviceKeys, hww: &DeviceKeys) -> VaultConfig {
        let policy = crate::policy::VaultPolicy::new(phone.vault_pubkey, hww.vault_pubkey).unwrap();
        let (external, internal) = phone
            .hot_descriptors(&bitcoin::key::Secp256k1::new())
            .unwrap();
        VaultConfig {
            version: crate::storage::VAULT_CONFIG_VERSION,
            network: "regtest".into(),
            phone_vault_pubkey: phone.vault_pubkey.to_string(),
            hww_vault_pubkey: hww.vault_pubkey.to_string(),
            phone_hot_external_descriptor: external,
            phone_hot_internal_descriptor: internal,
            vault_descriptor: policy.descriptor_string(),
            vault_address: policy.address.to_string(),
            hww_recovery_seconds: crate::HWW_RECOVERY_SECONDS,
            monthly_limit_sats: 0,
            emergency_access_limit_sats: 0,
        }
    }

    #[test]
    fn only_the_signer_decrypts_the_authenticated_payload() {
        let (config, phone, hww) = fixture();
        let payload = RecoveryPayload::new(&config, &phone).unwrap();
        let backup = create_backup(&payload, &hww.seed).unwrap();
        assert_eq!(decrypt_with_hww(&backup, &hww.seed).unwrap(), payload);
        assert!(decrypt_with_hww(&backup, &phone.seed).is_err());

        let mut tampered = backup;
        tampered.encrypted_payload.ciphertext[0] ^= 1;
        assert!(decrypt_with_hww(&tampered, &hww.seed).is_err());
    }

    #[test]
    fn encrypted_recovery_payload_preserves_a_vanity_vault_key_index() {
        use crate::policy::VaultPolicy;

        let secp = bitcoin::key::Secp256k1::new();
        let (_, phone, hww) = fixture();
        let phone = phone.with_vault_key_index(&secp, 42).unwrap();
        let config = config_for(&phone, &hww);
        let backup =
            create_backup(&RecoveryPayload::new(&config, &phone).unwrap(), &hww.seed).unwrap();

        let recovered = decrypt_with_hww(&backup, &hww.seed).unwrap();
        assert_eq!(recovered.phone_vault_key_index, 42);
        recovered.validate_against(&config).unwrap();
        let reconstructed = recovered.recovery_config().unwrap();
        assert_eq!(reconstructed.vault_descriptor, config.vault_descriptor);
        assert_eq!(reconstructed.vault_address, config.vault_address);
        assert_eq!(reconstructed.hww_vault_pubkey, config.hww_vault_pubkey);
        assert_eq!(reconstructed.phone_vault_pubkey, config.phone_vault_pubkey);
        assert_eq!(reconstructed.monthly_limit_sats, 0); // Unknown from this payload.

        let mut wrong_key = recovered.clone();
        wrong_key.phone_vault_key_index = 0;
        assert!(wrong_key.recovery_config().is_err());
        let mut wrong_network = recovered.clone();
        wrong_network.network = "bitcoin".into();
        assert!(wrong_network.recovery_config().is_err());

        // Even a valid descriptor/address pair must use the fixed vault construction.
        // Recompute the address/checksum so this tests policy validation, not a stale checksum.
        let modified = recovered
            .vault_descriptor
            .split('#')
            .next()
            .unwrap()
            .replace("older(4259839)", "older(10)");
        let weakened =
            VaultPolicy::from_descriptor_for_network(&modified, bitcoin::Network::Regtest).unwrap();
        let mut wrong_policy = recovered.clone();
        wrong_policy.vault_descriptor = weakened.descriptor.to_string();
        wrong_policy.vault_address = weakened.address.to_string();
        assert!(wrong_policy.recovery_config().is_err());
    }
}
