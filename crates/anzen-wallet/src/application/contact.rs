//! A recovery contact's private OpenPGP key stays on that contact's own wallet device.
use super::*;
use anzen_common::social;
use anzen_common::storage::fs::StoragePath;
use zeroize::Zeroize;

#[derive(Clone, Serialize, Deserialize, Zeroize)]
#[serde(deny_unknown_fields)]
pub struct RecoveryContact {
    pub version: u8,
    pub name: String,
    pub fingerprint: String,
    pub public_key_armored: String,
    #[serde(default)]
    pub transport_public_key: String,
}

impl RecoveryContact {
    pub fn validate(&self) -> Result<()> {
        if self.version != 1 || self.name.trim().is_empty() || self.name.len() > 128 {
            bail!("invalid recovery contact version or name");
        }
        if self.public_key_armored.len() > 64 * 1024 {
            bail!("recovery contact public key exceeds 64 KiB");
        }
        if social::friend_fingerprint(self.public_key_armored.as_bytes())? != self.fingerprint {
            bail!("recovery contact fingerprint does not match its OpenPGP key");
        }
        if !self.transport_public_key.is_empty() {
            bitcoin::secp256k1::XOnlyPublicKey::from_str(&self.transport_public_key)?;
        }
        Ok(())
    }
}

#[derive(Serialize, Deserialize, Zeroize, zeroize::ZeroizeOnDrop)]
struct PrivateContact {
    public: RecoveryContact,
    private_key_armored: String,
}

impl WalletApplication {
    pub fn generate_recovery_contact(&self, name: &str) -> Result<RecoveryContact> {
        self.public_identity()?;
        if name.trim().is_empty() || name.len() > 128 {
            bail!("use a recovery contact name between 1 and 128 bytes");
        }
        let path = self.data_dir.join("phone/recovery-contact.json");
        if path.storage_exists() {
            bail!("this device already has a recovery contact key; refusing to replace it");
        }
        let key = social::generate_friend_key(name)?;
        let private = PrivateContact {
            public: RecoveryContact {
                version: 1,
                name: name.trim().into(),
                fingerprint: key.fingerprint,
                public_key_armored: key.public_key_armored,
                transport_public_key: self.public_identity()?.vault_pubkey,
            },
            private_key_armored: key.private_key_armored,
        };
        private.public.validate()?;
        storage::write_json(&path, &private)?;
        Ok(private.public.clone())
    }

    pub fn recovery_contact(&self) -> Result<Option<RecoveryContact>> {
        self.public_identity()?;
        let path = self.data_dir.join("phone/recovery-contact.json");
        if !path.storage_exists() {
            return Ok(None);
        }
        let private: PrivateContact = storage::read_json(&path)?;
        private.public.validate()?;
        let mut public = private.public.clone();
        public.transport_public_key = self.public_identity()?.vault_pubkey;
        Ok(Some(public))
    }

    pub(super) fn unwrap_contact_key(
        &self,
        wrapper: &social::FriendKeyWrapper,
    ) -> Result<zeroize::Zeroizing<[u8; 32]>> {
        self.public_identity()?;
        let private: PrivateContact =
            storage::read_json(&self.data_dir.join("phone/recovery-contact.json"))?;
        private.public.validate()?;
        if wrapper.fingerprint != private.public.fingerprint {
            bail!("this device is not the requested recovery contact");
        }
        social::unwrap_friend_wrapper(wrapper, private.private_key_armored.as_bytes())
    }
}
