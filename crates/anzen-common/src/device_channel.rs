//! Confidential random-key wrapping messages for backup ceremonies, not Bitcoin signatures.
//!
//! Public pairing must already have authenticated both x-only keys. Normalize both endpoints
//! to even Y before ECDH. Bind direction, purpose and peer identities into HKDF's info string.
//! This static-key channel is not forward secret. Never send a seed or its ciphertext through
//! it; the backup protocol keeps that data exclusively on the phone/cloud side.
//! It does not replace transaction signature verification or an explicit approval ceremony.

use super::{
    crypto::{self, EncryptedBlob},
    keys::DeviceKeys,
};
use anyhow::{Result, bail};
use bitcoin::secp256k1::{Parity, XOnlyPublicKey, ecdh::SharedSecret};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use zeroize::Zeroizing;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeviceMessage {
    pub version: u8,
    pub sender: String,
    pub recipient: String,
    pub payload: EncryptedBlob,
}

fn shared(own: &DeviceKeys, peer: XOnlyPublicKey) -> Zeroizing<[u8; 32]> {
    let (_, parity) = own.vault_keypair.x_only_public_key();
    let mut secret = own.vault_keypair.secret_key();
    if parity == Parity::Odd {
        secret = secret.negate();
    }
    let mut shared = SharedSecret::new(&peer.public_key(Parity::Even), &secret);
    let result = Zeroizing::new(shared.secret_bytes());
    shared.non_secure_erase();
    secret.non_secure_erase();
    result
}

fn purpose(kind: &str, sender: &str, recipient: &str) -> String {
    format!("anzen/device-channel/v1/{kind}/{sender}/{recipient}")
}

pub fn seal<T: Serialize>(
    own: &DeviceKeys,
    recipient: XOnlyPublicKey,
    kind: &str,
    value: &T,
) -> Result<DeviceMessage> {
    if own.vault_pubkey == recipient {
        bail!("device channel requires independent peer keys");
    }
    let sender = own.vault_pubkey.to_string();
    let plaintext = Zeroizing::new(serde_json::to_vec(value)?);
    let key = shared(own, recipient);
    let payload = crypto::encrypt(
        key.as_ref(),
        &purpose(kind, &sender, &recipient.to_string()),
        &plaintext,
    )?;
    Ok(DeviceMessage {
        version: 1,
        sender,
        recipient: recipient.to_string(),
        payload,
    })
}

pub fn open<T: DeserializeOwned>(
    own: &DeviceKeys,
    expected_sender: XOnlyPublicKey,
    kind: &str,
    message: &DeviceMessage,
) -> Result<T> {
    if message.version != 1
        || message.sender != expected_sender.to_string()
        || message.recipient != own.vault_pubkey.to_string()
        || own.vault_pubkey == expected_sender
    {
        bail!("encrypted message is not bound to the expected independent devices");
    }
    let key = shared(own, expected_sender);
    let plaintext = crypto::decrypt(
        key.as_ref(),
        &purpose(kind, &message.sender, &message.recipient),
        &message.payload,
    )?;
    Ok(serde_json::from_slice(&plaintext)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use bitcoin::{Network, key::Secp256k1};

    #[test]
    fn channel_handles_both_key_parities_and_binds_peer_direction_and_purpose() {
        let secp = Secp256k1::new();
        let mut keys = Vec::new();
        for parity in [Parity::Even, Parity::Odd] {
            loop {
                let key = DeviceKeys::generate_for_network(&secp, Network::Regtest).unwrap();
                if key.vault_keypair.x_only_public_key().1 == parity {
                    keys.push(key);
                    break;
                }
            }
        }
        for own in &keys {
            for _ in 0..4 {
                let peer = DeviceKeys::generate_for_network(&secp, Network::Regtest).unwrap();
                let message = seal(own, peer.vault_pubkey, "test/backup", &"secret words").unwrap();
                let result: String =
                    open(&peer, own.vault_pubkey, "test/backup", &message).unwrap();
                assert_eq!(result, "secret words");
                assert!(open::<String>(&peer, own.vault_pubkey, "test/other", &message).is_err());
                assert!(open::<String>(own, peer.vault_pubkey, "test/backup", &message).is_err());
                let mut tampered = message;
                tampered.payload.ciphertext[0] ^= 1;
                assert!(open::<String>(&peer, own.vault_pubkey, "test/backup", &tampered).is_err());
            }
        }
    }
}
