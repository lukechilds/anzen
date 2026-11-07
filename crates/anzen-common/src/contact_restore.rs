//! A recovery contact receives only its OpenPGP key wrapper and the new recipient identity.
use super::{
    social::{self, FriendKeyWrapper},
    storage::PublicDeviceFile,
};
use anyhow::{Result, ensure};
use bitcoin::{
    hashes::{Hash, sha256},
    secp256k1::XOnlyPublicKey,
};
use serde::{Deserialize, Serialize};
use std::str::FromStr;

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ContactRestoreRequest {
    pub version: u8,
    pub backup_id: String,
    pub recipient: PublicDeviceFile,
    pub contact_public_key: String,
    pub friend: FriendKeyWrapper,
}

impl ContactRestoreRequest {
    pub fn validate(&self) -> Result<()> {
        ensure!(self.version == 1, "unsupported contact restore request");
        sha256::Hash::from_str(&self.backup_id)?;
        self.recipient.validate_kind("phone-public-key")?;
        XOnlyPublicKey::from_str(&self.contact_public_key)?;
        ensure!(
            self.contact_public_key != self.recipient.vault_pubkey,
            "contact and recipient must be independent devices"
        );
        ensure!(
            self.friend.public_key_armored.len() <= 64 * 1024
                && self.friend.encrypted_symmetric_key.len() <= 64 * 1024,
            "contact restore wrapper exceeds 64 KiB"
        );
        ensure!(
            social::friend_fingerprint(self.friend.public_key_armored.as_bytes())?
                == self.friend.fingerprint,
            "contact restore fingerprint mismatch"
        );
        Ok(())
    }

    pub fn id(&self) -> Result<String> {
        Ok(sha256::Hash::hash(&serde_json::to_vec(self)?).to_string())
    }

    pub fn response_purpose(&self) -> Result<String> {
        Ok(format!("contact-restore-response/{}", self.id()?))
    }
}
