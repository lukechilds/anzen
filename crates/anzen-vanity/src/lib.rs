//! GPU search for vanity vault addresses, two ways.
//!
//! - Key grinding ([`SearchInputs`]): each candidate is a phone vault key index. The shader
//!   derives the child key from the phone's vault xpub, aggregates it with the signer key
//!   (BIP327) and applies the Taproot tweak with the signer's leaf (BIP341).
//! - Script nonce grinding ([`NonceInputs`]): both keys are fixed and each candidate is the
//!   recovery leaf's past `after()` locktime, which only changes the leaf hash and so the tweak.
//!
//! Either way the shader compares the top bits of the output key with the target. Only public
//! data reaches the GPU, and callers must re-derive the address with the library code before
//! trusting a result.
//!
//! The same code runs on Metal, Vulkan and DirectX natively and on WebGPU in browsers.

/// Everything a search needs, all of it public.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SearchInputs {
    /// The compressed public key of the phone's vault parent (`m/86'/coin'/100'/0`).
    pub parent_public_key: [u8; 33],
    pub chain_code: [u8; 32],
    /// The signer's x-only vault key.
    pub signer_key: [u8; 32],
    /// The root of the vault's script tree (the signer's leaf hash).
    pub merkle_root: [u8; 32],
    /// The output key's top `target_bits` bits must equal `target_value`.
    pub target_value: u32,
    pub target_bits: u32,
}

/// Everything a script nonce search needs, all of it public.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NonceInputs {
    /// The vault's internal key: the x-only MuSig2 aggregate of both device keys.
    pub internal_key: [u8; 32],
    /// The signer's x-only vault key, named by the recovery leaf.
    pub signer_key: [u8; 32],
    /// The output key's top `target_bits` bits must equal `target_value`.
    pub target_value: u32,
    pub target_bits: u32,
}

/// Recovery locktimes a nonce search may try: Unix times, all long past.
pub const NONCE_LOCKTIMES: std::ops::Range<u32> = 500_000_000..1_700_000_000;

/// The largest non-hardened BIP32 index.
pub const MAX_INDEX: u32 = (1 << 31) - 1;

#[cfg(feature = "gpu")]
mod gpu;
#[cfg(feature = "gpu")]
pub use gpu::{GpuSearch, Prepared, SearchResult};

#[cfg(all(feature = "metal", target_vendor = "apple"))]
mod metal;
#[cfg(all(feature = "metal", target_vendor = "apple"))]
pub use metal::{MetalPrepared, MetalSearch};
