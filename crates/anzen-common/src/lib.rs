//! Records, envelopes and messages shared by the Anzen wallet and software signer applications.
//!
//! Protocol rules live in `anzen_protocol`; this crate holds what both applications persist or
//! exchange around them: vault and device records, software key derivation, encrypted backup
//! envelopes, the device channel, the outbox and the wrapper messages for multi-device flows.
//! It has no chain client and no PSBT or wallet logic.

pub mod cloud_backup;
pub mod cloud_restore;
pub mod crypto;
pub mod device_channel;
pub mod device_rotation;
pub mod keys;
pub mod musig;
pub mod outbox;
pub mod policy;
pub mod protocol;
pub mod recovery_bundle;
pub mod storage;
pub mod types;

/// The hardware wallet's solo path opens after the longest BIP68 time lock: 65,535 × 512
/// seconds, about 388 days. Renewing the vault each year restarts it.
pub use anzen_protocol::consts::{
    DEFAULT_RECOVERY_LOCKTIME, HWW_RECOVERY_SECONDS, HWW_RECOVERY_SEQUENCE, RECOVERY_LOCKTIMES,
};
pub const MONTHS_PER_ROLLOVER: usize = 12;
pub const DEFAULT_MONTHLY_LIMIT_SATS: u64 = 10_000_000;
pub const DEFAULT_FEE_RATE_SAT_VB: u64 = 1;
/// Fixed value carried by each live connector UTXO in the current protocol version. At the MVP's
/// 1 sat/vB fee rate, 1,000 sats pays a standalone revocation's fee (111 vbytes from the phone, 129
/// from the HWW) and leaves non-dust P2TR wallet change. Production fee selection may add ordinary wallet inputs at higher feerates.
pub const CONNECTOR_VALUE_SATS: u64 = 1_000;
// BIP68 time locks use 512-second units. This is the smallest representable delay that is at
// least 30 days: ceil(2_592_000 / 512) * 512 = 2_592_256 seconds.
pub const MONTHLY_ALLOWANCE_DELAY_SECONDS: u32 = 5_063 * 512;
// BIP68 time locks use 512-second units. This is the smallest representable delay that is at
// least one week: ceil(604_800 / 512) * 512 = 605_184 seconds.
pub const EMERGENCY_ACCESS_DELAY_SECONDS: u32 = 1_182 * 512;
