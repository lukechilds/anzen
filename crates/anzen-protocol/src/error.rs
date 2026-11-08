use core::fmt;

/// Every way a protocol request can be rejected. Messages are stable user-facing text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Error {
    InvalidKey,
    InvalidAddress,
    WrongNetwork,
    UnsupportedDestination,
    InvalidHex,
    UnsupportedVersion,
    NoVaultInputs,
    TooManyInputs,
    DuplicateInput,
    AmountOverflow,
    DustMonthlyLimit,
    DustEmergencyAmount,
    DustEmergencyStaging,
    DustEmergencyChange,
    DustAllowanceChain,
    DustDestination,
    FinalAllowanceNotExhausted,
    InsufficientRolloverFee,
    InsufficientRemainder,
    InsufficientEmergencyFunds,
    InsufficientAllowanceFunds,
    InsufficientFee,
    AllowanceCountMismatch,
    EmergencyDestinationMismatch,
    InputIndexOutOfRange,
    PrevoutCountMismatch,
    InvalidSignature,
    SignatureCountMismatch,
    NonceCountMismatch,
    InvalidNonce,
    SigningFailed,
    NotPaired,
    AlreadyPaired,
    WrongVault,
    UnlinkedPreviousPolicy,
    ImplausibleCompletedAllowances,
    PolicyLimitsChanged,
    ReviewMismatch,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::InvalidKey => "invalid public key",
            Self::InvalidAddress => "invalid address",
            Self::WrongNetwork => "address belongs to another network",
            Self::UnsupportedDestination => "destination must be a Taproot address",
            Self::InvalidHex => "invalid hexadecimal value",
            Self::UnsupportedVersion => "unsupported protocol message version",
            Self::NoVaultInputs => "vault has no confirmed UTXOs to roll over",
            Self::TooManyInputs => "request must contain between 1 and 1000 inputs",
            Self::DuplicateInput => "request repeats an input",
            Self::AmountOverflow => "amount overflowed",
            Self::DustMonthlyLimit => "monthly limit would create a dust hot-wallet output",
            Self::DustEmergencyAmount => {
                "emergency access amount would create a dust hot-wallet output"
            }
            Self::DustEmergencyStaging => "emergency staging output would be dust",
            Self::DustEmergencyChange => "emergency access trigger would create dust vault change",
            Self::DustAllowanceChain => "allowance authorization would create a dust chain output",
            Self::DustDestination => "transaction would create a dust destination output",
            Self::FinalAllowanceNotExhausted => "final allowance does not exhaust its chain output",
            Self::InsufficientRolloverFee => "vault balance cannot pay the rollover fee",
            Self::InsufficientRemainder => {
                "vault balance cannot create a non-dust rollover remainder"
            }
            Self::InsufficientEmergencyFunds => {
                "vault balance cannot fund the configured emergency access amount and fees"
            }
            Self::InsufficientAllowanceFunds => "allowance chain cannot fund its authorization",
            Self::InsufficientFee => "inputs cannot pay the transaction fee",
            Self::AllowanceCountMismatch => {
                "policy destinations do not match the number of funded allowances"
            }
            Self::EmergencyDestinationMismatch => {
                "emergency destination does not match the emergency access limit"
            }
            Self::InputIndexOutOfRange => "input index is out of range",
            Self::PrevoutCountMismatch => "previous outputs do not match the transaction inputs",
            Self::InvalidSignature => "invalid signature",
            Self::SignatureCountMismatch => "signature count does not match the transaction graph",
            Self::NonceCountMismatch => "nonce count does not match the cooperative signatures",
            Self::InvalidNonce => "invalid MuSig2 nonce",
            Self::SigningFailed => "signing failed",
            Self::NotPaired => "signer is not paired with a vault",
            Self::AlreadyPaired => {
                "signer already paired; use an explicitly approved rotation to change vaults"
            }
            Self::WrongVault => "request does not belong to the paired vault",
            Self::UnlinkedPreviousPolicy => {
                "policy does not replace the previously approved policy; it would leave earlier presigned actions live"
            }
            Self::ImplausibleCompletedAllowances => {
                "previous monthly allowances cannot all have been used yet"
            }
            Self::PolicyLimitsChanged => "rotation must preserve the approved policy limits",
            Self::ReviewMismatch => "request changed since it was reviewed",
        })
    }
}

impl core::error::Error for Error {}
