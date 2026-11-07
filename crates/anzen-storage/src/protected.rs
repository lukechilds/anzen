//! Native at-rest JSON protection. Platform code obtains the key from Keychain/Keystore;
//! it never goes through JavaScript. This synchronous scope cannot cross an async suspension.
//!
//! The SQLite chain cache contains public descriptors/history and remains covered by the OS's
//! app-private file protection. Device seeds, configuration and policy JSON are authenticated
//! ciphertext. CLI/browser storage is unchanged unless this scope is explicitly installed.

use anyhow::{Context, Result, bail};
use base64::{Engine, engine::general_purpose::STANDARD};
use chacha20poly1305::{
    ChaCha20Poly1305, KeyInit, Nonce,
    aead::{Aead, Payload},
};
use rand::RngCore;
use serde::{Deserialize, Serialize};
use std::{
    cell::RefCell,
    path::{Path, PathBuf},
};
use zeroize::Zeroizing;

const DOMAIN: &[u8] = b"anzen-native-storage-v1";
thread_local! {
    static ACTIVE: RefCell<Option<Protection>> = const { RefCell::new(None) };
}

struct Protection {
    root: PathBuf,
    key: Zeroizing<[u8; 32]>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Envelope {
    anzen_native_storage: u8,
    nonce: String,
    ciphertext: String,
}

/// Execute synchronous library operations with an OS-managed, device-specific encryption key.
/// Native adapters must serialize operations, use an absolute app-private root, and retain the
/// same key across launches. A panic also clears the thread-local key.
pub fn with_protected_storage<T>(
    root: &Path,
    key: [u8; 32],
    operation: impl FnOnce() -> Result<T>,
) -> Result<T> {
    if !root.is_absolute()
        || root
            .components()
            .any(|part| matches!(part, std::path::Component::ParentDir))
    {
        bail!("protected storage requires an absolute normalized app-private directory");
    }
    ACTIVE.with(|active| -> Result<()> {
        let mut active = active.borrow_mut();
        if active.is_some() {
            bail!("nested protected storage scopes are not supported");
        }
        *active = Some(Protection {
            root: root.to_path_buf(),
            key: Zeroizing::new(key),
        });
        Ok(())
    })?;
    struct Clear;
    impl Drop for Clear {
        fn drop(&mut self) {
            ACTIVE.with(|active| {
                active.borrow_mut().take();
            });
        }
    }
    let _clear = Clear;
    operation()
}

pub(super) fn encode(path: &Path, plaintext: &[u8]) -> Result<Vec<u8>> {
    ACTIVE.with(|active| {
        let active = active.borrow();
        let Some(protection) = active.as_ref().filter(|p| path.starts_with(&p.root)) else {
            return Ok(plaintext.to_vec());
        };
        let mut nonce = [0u8; 12];
        rand::rngs::OsRng.fill_bytes(&mut nonce);
        let ciphertext = ChaCha20Poly1305::new((&*protection.key).into())
            .encrypt(
                Nonce::from_slice(&nonce),
                Payload {
                    msg: plaintext,
                    aad: DOMAIN,
                },
            )
            .map_err(|_| anyhow::anyhow!("native storage encryption failed"))?;
        Ok(serde_json::to_vec(&Envelope {
            anzen_native_storage: 1,
            nonce: STANDARD.encode(nonce),
            ciphertext: STANDARD.encode(ciphertext),
        })?)
    })
}

pub(super) fn decode(path: &Path, bytes: Vec<u8>) -> Result<Zeroizing<Vec<u8>>> {
    ACTIVE.with(|active| {
        let active = active.borrow();
        let protection = active.as_ref().filter(|p| path.starts_with(&p.root));
        let envelope = serde_json::from_slice::<Envelope>(&bytes);
        match (protection, envelope) {
            (None, Err(_)) => Ok(Zeroizing::new(bytes)),
            (None, Ok(_)) => bail!("unlock the native storage key before opening this device"),
            (Some(_), Err(_)) => bail!("refusing unprotected or malformed native state"),
            (Some(protection), Ok(envelope)) => {
                if envelope.anzen_native_storage != 1 {
                    bail!("unsupported native storage version");
                }
                let nonce: [u8; 12] = STANDARD
                    .decode(envelope.nonce)?
                    .try_into()
                    .ok()
                    .context("invalid native storage nonce")?;
                let ciphertext = STANDARD.decode(envelope.ciphertext)?;
                let plaintext = ChaCha20Poly1305::new((&*protection.key).into())
                    .decrypt(
                        Nonce::from_slice(&nonce),
                        Payload {
                            msg: &ciphertext,
                            aad: DOMAIN,
                        },
                    )
                    .map_err(|_| anyhow::anyhow!("native storage authentication failed"))?;
                Ok(Zeroizing::new(plaintext))
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{read_json, write_json};

    #[test]
    fn authenticated_storage_roundtrips_and_rejects_wrong_key_or_plaintext() {
        let dir = crate::tempdir().unwrap();
        let path = dir.path().join("phone/device.json");
        with_protected_storage(dir.path(), [42; 32], || write_json(&path, &"secret seed")).unwrap();
        assert!(
            !crate::fs::read_to_string(&path)
                .unwrap()
                .contains("secret seed")
        );
        assert!(read_json::<String>(&path).is_err());
        assert!(
            with_protected_storage(dir.path(), [43; 32], || read_json::<String>(&path)).is_err()
        );
        let value: String =
            with_protected_storage(dir.path(), [42; 32], || read_json(&path)).unwrap();
        assert_eq!(value, "secret seed");
        write_json(&path, &"unprotected").unwrap();
        assert!(
            with_protected_storage(dir.path(), [42; 32], || read_json::<String>(&path)).is_err()
        );
    }

    #[test]
    fn scope_is_cleared_after_panic_and_does_not_encrypt_peer_packages() {
        let dir = crate::tempdir().unwrap();
        let peer = crate::tempdir().unwrap();
        let peer_path = peer.path().join("package.json");
        let _ = std::panic::catch_unwind(|| {
            let _: Result<()> = with_protected_storage(dir.path(), [42; 32], || {
                write_json(&peer_path, &"public package")?;
                panic!("test interruption")
            });
        });
        assert_eq!(read_json::<String>(&peer_path).unwrap(), "public package");
        with_protected_storage(dir.path(), [42; 32], || Ok(())).unwrap();
    }
}
