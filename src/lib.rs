pub mod crypto;
pub mod hot;
pub mod keys;
pub mod policy;
pub mod rpc;
pub mod state;
pub mod transactions;

pub const PHONE_RECOVERY_BLOCKS: u16 = 61_200;
pub const HWW_RECOVERY_BLOCKS: u16 = 65_535;
pub const MONTHS_PER_ROLLOVER: usize = 12;
pub const DEFAULT_HARD_LIMIT_SATS: u64 = 10_000_000;
pub const DEFAULT_FEE_RATE_SAT_VB: u64 = 1;
