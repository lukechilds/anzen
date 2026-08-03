//! Security-critical types and deterministic Bitcoin protocol rules shared by every client.
//!
//! This module does not depend on either device implementation. It defines the policy, PSBT
//! validation/signing primitives, serialized protocol objects, storage formats, and chain access
//! used by the phone and hardware-wallet libraries.

pub mod ceremony;
pub mod chain;
pub mod crypto;
pub mod keys;
pub mod policy;
pub mod recovery;
pub mod storage;
pub mod transactions;
pub mod types;

pub const PHONE_RECOVERY_BLOCKS: u16 = 61_200;
pub const HWW_RECOVERY_BLOCKS: u16 = 65_535;
pub const MONTHS_PER_ROLLOVER: usize = 12;
pub const DEFAULT_MONTHLY_LIMIT_SATS: u64 = 10_000_000;
pub const DEFAULT_FEE_RATE_SAT_VB: u64 = 1;
