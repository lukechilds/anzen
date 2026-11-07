//! Hashing and curve arithmetic supplied by the host device.

use crate::Result;

/// A BIP341 output key and the parity needed by control blocks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TweakedKey {
    pub output_key: [u8; 32],
    pub odd_y: bool,
}

/// Primitives every protocol computation needs. Implementations must be exact: an incorrect
/// hash or tweak produces different addresses and signature messages.
pub trait Crypto {
    /// SHA-256 over the concatenation of `parts`.
    fn sha256(&mut self, parts: &[&[u8]]) -> [u8; 32];

    /// Whether `key` is a valid BIP340 x-only public key.
    fn is_valid_xonly_key(&mut self, key: &[u8; 32]) -> bool;

    /// BIP341 key tweak `Q = P + t·G` for an x-only internal key.
    fn taproot_tweak(&mut self, internal_key: &[u8; 32], tweak: &[u8; 32]) -> Result<TweakedKey>;
}

pub(crate) fn tagged_hash<C: Crypto + ?Sized>(
    crypto: &mut C,
    tag_hash: &[u8; 32],
    parts: &[&[u8]],
) -> [u8; 32] {
    let mut all: alloc::vec::Vec<&[u8]> = alloc::vec::Vec::with_capacity(parts.len() + 2);
    all.push(tag_hash);
    all.push(tag_hash);
    all.extend_from_slice(parts);
    crypto.sha256(&all)
}

pub(crate) fn double_sha256<C: Crypto + ?Sized>(crypto: &mut C, bytes: &[u8]) -> [u8; 32] {
    let first = crypto.sha256(&[bytes]);
    crypto.sha256(&[&first])
}

/// SHA256("TapLeaf").
pub(crate) const TAPLEAF_TAG: [u8; 32] = [
    0xae, 0xea, 0x8f, 0xdc, 0x42, 0x08, 0x98, 0x31, 0x05, 0x73, 0x4b, 0x58, 0x08, 0x1d, 0x1e, 0x26,
    0x38, 0xd3, 0x5f, 0x1c, 0xb5, 0x40, 0x08, 0xd4, 0xd3, 0x57, 0xca, 0x03, 0xbe, 0x78, 0xe9, 0xee,
];
/// SHA256("TapBranch").
pub(crate) const TAPBRANCH_TAG: [u8; 32] = [
    0x19, 0x41, 0xa1, 0xf2, 0xe5, 0x6e, 0xb9, 0x5f, 0xa2, 0xa9, 0xf1, 0x94, 0xbe, 0x5c, 0x01, 0xf7,
    0x21, 0x6f, 0x33, 0xed, 0x82, 0xb0, 0x91, 0x46, 0x34, 0x90, 0xd0, 0x5b, 0xf5, 0x16, 0xa0, 0x15,
];
/// SHA256("TapTweak").
pub(crate) const TAPTWEAK_TAG: [u8; 32] = [
    0xe8, 0x0f, 0xe1, 0x63, 0x9c, 0x9c, 0xa0, 0x50, 0xe3, 0xaf, 0x1b, 0x39, 0xc1, 0x43, 0xc6, 0x3e,
    0x42, 0x9c, 0xbc, 0xeb, 0x15, 0xd9, 0x40, 0xfb, 0xb5, 0xc5, 0xa1, 0xf4, 0xaf, 0x57, 0xc5, 0xe9,
];
/// SHA256("TapSighash").
pub(crate) const TAPSIGHASH_TAG: [u8; 32] = [
    0xf4, 0x0a, 0x48, 0xdf, 0x4b, 0x2a, 0x70, 0xc8, 0xb4, 0x92, 0x4b, 0xf2, 0x65, 0x46, 0x61, 0xed,
    0x3d, 0x95, 0xfd, 0x66, 0xa3, 0x13, 0xeb, 0x87, 0x23, 0x75, 0x97, 0xc6, 0x28, 0xe4, 0xa0, 0x31,
];

#[cfg(feature = "software")]
pub use software::SoftwareCrypto;

#[cfg(feature = "software")]
mod software {
    use super::*;
    use crate::Error;
    use secp256k1::{Parity, Scalar, Secp256k1, VerifyOnly, XOnlyPublicKey};
    use sha2::{Digest, Sha256};

    /// Pure-software primitives for phones, desktops and tests.
    pub struct SoftwareCrypto {
        secp: Secp256k1<VerifyOnly>,
    }

    impl Default for SoftwareCrypto {
        fn default() -> Self {
            Self {
                secp: Secp256k1::verification_only(),
            }
        }
    }

    impl SoftwareCrypto {
        pub fn new() -> Self {
            Self::default()
        }
    }

    impl Crypto for SoftwareCrypto {
        fn sha256(&mut self, parts: &[&[u8]]) -> [u8; 32] {
            let mut hasher = Sha256::new();
            for part in parts {
                hasher.update(part);
            }
            hasher.finalize().into()
        }

        fn is_valid_xonly_key(&mut self, key: &[u8; 32]) -> bool {
            XOnlyPublicKey::from_slice(key).is_ok()
        }

        fn taproot_tweak(
            &mut self,
            internal_key: &[u8; 32],
            tweak: &[u8; 32],
        ) -> Result<TweakedKey> {
            let internal =
                XOnlyPublicKey::from_slice(internal_key).map_err(|_| Error::InvalidKey)?;
            let tweak = Scalar::from_be_bytes(*tweak).map_err(|_| Error::InvalidKey)?;
            let (output, parity) = internal
                .add_tweak(&self.secp, &tweak)
                .map_err(|_| Error::InvalidKey)?;
            Ok(TweakedKey {
                output_key: output.serialize(),
                odd_y: parity == Parity::Odd,
            })
        }
    }
}
