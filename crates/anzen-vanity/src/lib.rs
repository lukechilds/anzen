//! GPU search for vanity vault addresses by script nonce grinding.
//!
//! Both device keys are fixed and each candidate is the recovery leaf's long-past `after()`
//! locktime, which only changes the leaf hash and so the Taproot tweak. The shader compares the
//! top bits of each output key with the target. Only public data reaches the GPU, and callers
//! must re-derive the address with the library code before trusting a result.
//!
//! The same code runs on Metal, Vulkan and DirectX natively and on WebGPU in browsers.

/// Everything a search needs, all of it public.
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

/// Recovery locktimes a search may try: Unix times, all long past. The protocol's
/// `RECOVERY_LOCKTIMES` are the same range; the wallet's tests check it.
pub const NONCE_LOCKTIMES: std::ops::Range<u32> = 500_000_000..1_700_000_000;

#[cfg(feature = "gpu")]
pub mod bench;
#[cfg(feature = "gpu")]
mod gpu;
#[cfg(feature = "gpu")]
pub use gpu::{GpuSearch, Prepared, SearchResult};
