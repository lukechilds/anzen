//! A minimal version-2 SegWit transaction model with consensus serialization.

use crate::{
    Error, Result,
    consts::FEE_RATE_SAT_VB,
    crypto::{Crypto, double_sha256},
    hex,
};
use alloc::{format, string::String, vec::Vec};
use core::{fmt, str::FromStr};
use serde::{Deserialize, Serialize};

/// A transaction ID in internal (hashing) byte order. Text uses Bitcoin's reversed display order.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Txid(pub [u8; 32]);

impl Txid {
    pub fn to_hex(&self) -> String {
        let mut reversed = self.0;
        reversed.reverse();
        hex::encode(&reversed)
    }
}

impl fmt::Debug for Txid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_hex())
    }
}

impl fmt::Display for Txid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_hex())
    }
}

impl FromStr for Txid {
    type Err = Error;
    fn from_str(text: &str) -> Result<Self> {
        let mut bytes = hex::decode_array::<32>(text)?;
        bytes.reverse();
        Ok(Self(bytes))
    }
}

impl Serialize for Txid {
    fn serialize<S: serde::Serializer>(
        &self,
        serializer: S,
    ) -> core::result::Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.to_hex())
    }
}

impl<'de> Deserialize<'de> for Txid {
    fn deserialize<D: serde::Deserializer<'de>>(
        deserializer: D,
    ) -> core::result::Result<Self, D::Error> {
        let text = <alloc::borrow::Cow<'de, str>>::deserialize(deserializer)?;
        text.parse().map_err(serde::de::Error::custom)
    }
}

/// A reference to one transaction output. Text form is `txid:vout`.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct OutPoint {
    pub txid: Txid,
    pub vout: u32,
}

impl OutPoint {
    pub const fn new(txid: Txid, vout: u32) -> Self {
        Self { txid, vout }
    }

    /// The all-zero outpoint, used only as a size placeholder in fee templates.
    pub const fn null() -> Self {
        Self {
            txid: Txid([0; 32]),
            vout: u32::MAX,
        }
    }
}

impl fmt::Debug for OutPoint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}", self.txid, self.vout)
    }
}

impl fmt::Display for OutPoint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}", self.txid, self.vout)
    }
}

impl FromStr for OutPoint {
    type Err = Error;
    fn from_str(text: &str) -> Result<Self> {
        let (txid, vout) = text.split_once(':').ok_or(Error::InvalidHex)?;
        Ok(Self {
            txid: txid.parse()?,
            vout: vout.parse().map_err(|_| Error::InvalidHex)?,
        })
    }
}

impl Serialize for OutPoint {
    fn serialize<S: serde::Serializer>(
        &self,
        serializer: S,
    ) -> core::result::Result<S::Ok, S::Error> {
        serializer.serialize_str(&format!("{self}"))
    }
}

impl<'de> Deserialize<'de> for OutPoint {
    fn deserialize<D: serde::Deserializer<'de>>(
        deserializer: D,
    ) -> core::result::Result<Self, D::Error> {
        let text = <alloc::borrow::Cow<'de, str>>::deserialize(deserializer)?;
        text.parse().map_err(serde::de::Error::custom)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TxIn {
    pub previous_output: OutPoint,
    pub sequence: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TxOut {
    pub value: u64,
    pub script_pubkey: Vec<u8>,
}

/// Every Anzen transaction is version 2 with a zero lock time.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Transaction {
    pub input: Vec<TxIn>,
    pub output: Vec<TxOut>,
}

pub const VERSION: i32 = 2;
pub const LOCK_TIME: u32 = 0;

impl Transaction {
    /// Consensus serialization without witness data.
    pub fn serialize(&self) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(self.base_size());
        bytes.extend_from_slice(&VERSION.to_le_bytes());
        write_compact_size(&mut bytes, self.input.len() as u64);
        for input in &self.input {
            bytes.extend_from_slice(&input.previous_output.txid.0);
            bytes.extend_from_slice(&input.previous_output.vout.to_le_bytes());
            bytes.push(0); // Empty scriptSig.
            bytes.extend_from_slice(&input.sequence.to_le_bytes());
        }
        write_compact_size(&mut bytes, self.output.len() as u64);
        for output in &self.output {
            write_output(&mut bytes, output);
        }
        bytes.extend_from_slice(&LOCK_TIME.to_le_bytes());
        bytes
    }

    pub fn txid<C: Crypto + ?Sized>(&self, crypto: &mut C) -> Txid {
        Txid(double_sha256(crypto, &self.serialize()))
    }

    fn base_size(&self) -> usize {
        4 + compact_size_len(self.input.len() as u64)
            + self.input.len() * (32 + 4 + 1 + 4)
            + compact_size_len(self.output.len() as u64)
            + self
                .output
                .iter()
                .map(|output| {
                    8 + compact_size_len(output.script_pubkey.len() as u64)
                        + output.script_pubkey.len()
                })
                .sum::<usize>()
            + 4
    }

    /// Virtual size once every input carries the witness for its spend path.
    pub fn estimated_vsize(&self, spends: &[SpendPath]) -> Result<u64> {
        if spends.len() != self.input.len() {
            return Err(Error::PrevoutCountMismatch);
        }
        // Weight = 3 × base size + total size; the SegWit marker and flag add two bytes.
        let witness: usize = spends.iter().map(|spend| spend.witness_size()).sum();
        let weight = self.base_size() * 4 + 2 + witness;
        Ok(weight.div_ceil(4) as u64)
    }

    /// Fee at the fixed protocol fee rate.
    pub fn fixed_fee(&self, spends: &[SpendPath]) -> Result<u64> {
        self.estimated_vsize(spends)?
            .checked_mul(FEE_RATE_SAT_VB)
            .ok_or(Error::AmountOverflow)
    }

    pub fn output_sum(&self) -> Result<u64> {
        self.output.iter().try_fold(0_u64, |sum, output| {
            sum.checked_add(output.value).ok_or(Error::AmountOverflow)
        })
    }
}

/// The Taproot script path an input will be satisfied with.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SpendPath {
    /// Vault `multi_a(2,phone,hww)` at depth one.
    Cooperative,
    /// Vault `and_v(v:older(61200),pk(phone))` at depth two.
    PhoneRecovery,
    /// Vault `and_v(v:older(65535),pk(hww))` at depth two.
    HwwRecovery,
    /// Connector `pk(phone)` at depth one.
    ControllerPhone,
    /// Connector `pk(hww)` at depth one.
    ControllerHww,
}

impl SpendPath {
    /// Serialized witness stack size, including its item count.
    pub fn witness_size(self) -> usize {
        let item = |len: usize| compact_size_len(len as u64) + len;
        let control_block = |depth: usize| 33 + 32 * depth;
        match self {
            Self::Cooperative => 1 + item(64) * 2 + item(70) + item(control_block(1)),
            Self::PhoneRecovery | Self::HwwRecovery => {
                1 + item(64) + item(40) + item(control_block(2))
            }
            Self::ControllerPhone | Self::ControllerHww => {
                1 + item(64) + item(34) + item(control_block(1))
            }
        }
    }

    pub fn is_controller(self) -> bool {
        matches!(self, Self::ControllerPhone | Self::ControllerHww)
    }
}

/// Bitcoin Core's default dust threshold at 3 sat/vB (`Script::minimal_non_dust`).
pub fn minimal_non_dust(script_pubkey: &[u8]) -> u64 {
    const DUST_RELAY_FEE_SAT_PER_KVB: u64 = 3_000;
    let encoded = (compact_size_len(script_pubkey.len() as u64) + script_pubkey.len()) as u64;
    let spend_size = if script_pubkey.first() == Some(&0x6a) {
        return 0;
    } else if is_witness_program(script_pubkey) {
        32 + 4 + 1 + (107 / 4) + 4
    } else {
        32 + 4 + 1 + 107 + 4
    };
    DUST_RELAY_FEE_SAT_PER_KVB * (spend_size + 8 + encoded) / 1000
}

fn is_witness_program(script: &[u8]) -> bool {
    let version_ok = script
        .first()
        .is_some_and(|op| *op == 0x00 || (0x51..=0x60).contains(op));
    version_ok && (4..=42).contains(&script.len()) && usize::from(script[1]) + 2 == script.len()
}

pub(crate) fn write_output(bytes: &mut Vec<u8>, output: &TxOut) {
    bytes.extend_from_slice(&output.value.to_le_bytes());
    write_compact_size(bytes, output.script_pubkey.len() as u64);
    bytes.extend_from_slice(&output.script_pubkey);
}

pub(crate) fn compact_size_len(value: u64) -> usize {
    match value {
        0..=0xfc => 1,
        0xfd..=0xffff => 3,
        0x1_0000..=0xffff_ffff => 5,
        _ => 9,
    }
}

pub(crate) fn write_compact_size(bytes: &mut Vec<u8>, value: u64) {
    match value {
        0..=0xfc => bytes.push(value as u8),
        0xfd..=0xffff => {
            bytes.push(0xfd);
            bytes.extend_from_slice(&(value as u16).to_le_bytes());
        }
        0x1_0000..=0xffff_ffff => {
            bytes.push(0xfe);
            bytes.extend_from_slice(&(value as u32).to_le_bytes());
        }
        _ => {
            bytes.push(0xff);
            bytes.extend_from_slice(&value.to_le_bytes());
        }
    }
}
