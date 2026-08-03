use anyhow::{Context, Result, bail};
use chacha20poly1305::{
    XChaCha20Poly1305, XNonce,
    aead::{Aead, KeyInit, OsRng, rand_core::RngCore},
};
use hkdf::Hkdf;
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use zeroize::Zeroizing;

const FORMAT_VERSION: u8 = 1;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EncryptedBlob {
    pub version: u8,
    pub purpose: String,
    pub nonce: [u8; 24],
    pub ciphertext: Vec<u8>,
}

pub fn derive_key(seed: &[u8], purpose: &str) -> Result<Zeroizing<[u8; 32]>> {
    let hk = Hkdf::<Sha256>::new(Some(b"renewable-bitcoin-vault/mvp/v1"), seed);
    let mut key = Zeroizing::new([0_u8; 32]);
    hk.expand(purpose.as_bytes(), key.as_mut())
        .map_err(|_| anyhow::anyhow!("invalid HKDF output length"))?;
    Ok(key)
}

pub fn encrypt(seed: &[u8], purpose: &str, plaintext: &[u8]) -> Result<EncryptedBlob> {
    let key = derive_key(seed, purpose)?;
    let cipher = XChaCha20Poly1305::new((&*key).into());
    let mut nonce = [0_u8; 24];
    OsRng.fill_bytes(&mut nonce);
    let ciphertext = cipher
        .encrypt(XNonce::from_slice(&nonce), plaintext)
        .map_err(|_| anyhow::anyhow!("encryption failed"))?;
    Ok(EncryptedBlob {
        version: FORMAT_VERSION,
        purpose: purpose.to_owned(),
        nonce,
        ciphertext,
    })
}

pub fn decrypt(
    seed: &[u8],
    expected_purpose: &str,
    blob: &EncryptedBlob,
) -> Result<Zeroizing<Vec<u8>>> {
    if blob.version != FORMAT_VERSION {
        bail!("unsupported encrypted blob version {}", blob.version);
    }
    if blob.purpose != expected_purpose {
        bail!(
            "encrypted blob purpose mismatch: expected {expected_purpose}, got {}",
            blob.purpose
        );
    }
    let key = derive_key(seed, expected_purpose)?;
    let cipher = XChaCha20Poly1305::new((&*key).into());
    let plaintext = cipher
        .decrypt(XNonce::from_slice(&blob.nonce), blob.ciphertext.as_ref())
        .map_err(|_| anyhow::anyhow!("authentication or decryption failed"))
        .context("unable to decrypt encrypted blob")?;
    Ok(Zeroizing::new(plaintext))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_and_domain_separation() {
        let seed = [7_u8; 64];
        let blob = encrypt(&seed, "phone-backup", b"secret words").unwrap();
        assert_eq!(
            decrypt(&seed, "phone-backup", &blob).unwrap().as_slice(),
            b"secret words"
        );
        assert!(decrypt(&seed, "monthly-transaction", &blob).is_err());
    }

    #[test]
    fn wrong_seed_fails_authentication() {
        let blob = encrypt(&[1_u8; 64], "phone-backup", b"secret words").unwrap();
        assert!(decrypt(&[2_u8; 64], "phone-backup", &blob).is_err());
    }

    #[test]
    fn tampering_fails_authentication() {
        let seed = [1_u8; 64];
        let mut blob = encrypt(&seed, "phone-backup", b"secret words").unwrap();
        blob.ciphertext[0] ^= 1;
        assert!(decrypt(&seed, "phone-backup", &blob).is_err());
    }
}
