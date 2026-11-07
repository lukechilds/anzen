//! Key-only recovery exchange. The HWW never receives the phone seed or its ciphertext.
//!
//! A replacement device has a fresh independent key. The user must compare its public
//! identity on both screens before approving release of a cloud backup's decryption key.

use super::{
    crypto::EncryptedBlob,
    storage::{PublicDeviceFile, VaultConfig},
};
use anyhow::{Result, bail};
use bitcoin::hashes::{Hash, sha256};
use serde::{Deserialize, Serialize};
use std::str::FromStr;

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RestoreKeyRequest {
    pub version: u8,
    pub vault_descriptor: String,
    pub backup_id: String,
    pub recipient: PublicDeviceFile,
    pub wrapped_key: EncryptedBlob,
}

impl RestoreKeyRequest {
    pub fn validate(&self, config: &VaultConfig) -> Result<()> {
        config.validate_public_policy()?;
        self.recipient.validate_kind("phone-public-key")?;
        sha256::Hash::from_str(&self.backup_id)?;
        if self.version != 1
            || self.vault_descriptor != config.vault_descriptor
            || self.recipient.network != config.network
        {
            bail!("restore request does not match the accepted vault and network");
        }
        if self.recipient.vault_pubkey == config.phone_vault_pubkey
            || self.recipient.vault_pubkey == config.hww_vault_pubkey
        {
            bail!("restore into a fresh independent phone key, then rotate away from the old key");
        }
        Ok(())
    }

    pub fn id(&self) -> Result<String> {
        Ok(sha256::Hash::hash(&serde_json::to_vec(self)?).to_string())
    }

    pub fn response_purpose(&self) -> Result<String> {
        Ok(format!("cloud-restore-response/{}", self.id()?))
    }
}

#[derive(Serialize, Deserialize, zeroize::Zeroize, zeroize::ZeroizeOnDrop)]
#[serde(deny_unknown_fields)]
pub struct RestoreKeyResponse {
    pub request_id: String,
    pub symmetric_key: [u8; 32],
}
