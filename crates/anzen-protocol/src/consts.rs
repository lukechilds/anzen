//! Fixed protocol parameters. Changing any value changes vault addresses or transaction graphs.

/// Relative block delay before the phone alone can spend a vault output.
pub const PHONE_RECOVERY_BLOCKS: u16 = 61_200;
/// Relative block delay before the hardware wallet alone can spend a vault output.
pub const HWW_RECOVERY_BLOCKS: u16 = 65_535;
/// Monthly allowance authorizations presigned in one annual policy.
pub const MONTHS_PER_ROLLOVER: usize = 12;
/// The MVP pays exactly one satoshi per estimated virtual byte.
pub const FEE_RATE_SAT_VB: u64 = 1;
/// Value carried by each live connector output.
pub const CONNECTOR_VALUE_SATS: u64 = 1_000;
/// BIP68 time locks use 512-second units: ceil(2_592_000 / 512) * 512 seconds.
pub const MONTHLY_ALLOWANCE_DELAY_SECONDS: u32 = 5_063 * 512;
/// BIP68 time locks use 512-second units: ceil(604_800 / 512) * 512 seconds.
pub const EMERGENCY_ACCESS_DELAY_SECONDS: u32 = 1_182 * 512;
/// Largest number of inputs any single request may spend.
pub const MAX_INPUTS: usize = 1_000;

const SEQUENCE_TYPE_FLAG: u32 = 1 << 22;
/// Final sequence: no relative lock and no replaceability signal.
pub const SEQUENCE_FINAL: u32 = u32::MAX;
/// BIP125 replaceable without a relative lock (`Sequence::ENABLE_RBF_NO_LOCKTIME`).
pub const SEQUENCE_RBF: u32 = 0xffff_fffd;
/// BIP68 time-based sequence for [`MONTHLY_ALLOWANCE_DELAY_SECONDS`].
pub const MONTHLY_DELAY_SEQUENCE: u32 =
    SEQUENCE_TYPE_FLAG | (MONTHLY_ALLOWANCE_DELAY_SECONDS / 512);
/// BIP68 time-based sequence for [`EMERGENCY_ACCESS_DELAY_SECONDS`].
pub const EMERGENCY_DELAY_SEQUENCE: u32 =
    SEQUENCE_TYPE_FLAG | (EMERGENCY_ACCESS_DELAY_SECONDS / 512);

/// BIP341's provably unspendable internal key.
pub const NUMS_KEY: [u8; 32] = [
    0x50, 0x92, 0x9b, 0x74, 0xc1, 0xa0, 0x49, 0x54, 0xb7, 0x8b, 0x4b, 0x60, 0x35, 0xe9, 0x7a, 0x5e,
    0x07, 0x8a, 0x5a, 0x0f, 0x28, 0xec, 0x96, 0xd5, 0x47, 0xbf, 0xee, 0x9a, 0xce, 0x80, 0x3a, 0xc0,
];
