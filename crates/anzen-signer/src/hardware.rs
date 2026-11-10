//! The command protocol between a host and a hardware signer app, shared by both ends.
//!
//! Requests are the same messages every signer reviews ([`anzen_protocol::wire`]), in their
//! compact form (postcard). A request is larger than one APDU, so the host uploads it in chunks,
//! runs one operation on it, and downloads the answer in chunks. The device holds one request at
//! a time and shows its review while the host waits for the `RUN` command's reply.
//!
//! The device keeps one vault per network.
//!
//! Proof of concept. Its one implementation is the Ledger app (`ledger-app/`), which holds a
//! whole request in memory and so accepts at most [`MAX_INPUTS`] inputs. Streaming a request
//! through the device would remove that limit, and is deliberately not planned: it would give
//! the shared transaction builder a second, incremental shape for the sake of one device, and
//! every other signer (the phone's signer mode, Trezor, Passport) has memory to spare.

use alloc::vec::Vec;
use anzen_protocol::{
    address::Network,
    script::XOnlyKey,
    wire::{CooperativeSignature, Signature},
};
use serde::{Deserialize, Serialize};

pub const CLA: u8 = 0xe0;
/// Bumped whenever a command or its encoding changes.
pub const PROTOCOL_VERSION: u8 = 1;

/// The largest request the device accepts.
pub const MAX_REQUEST: usize = 6_144;
/// The most inputs one request may spend. A limit of the proof of concept, not of the protocol:
/// a Ledger Flex or Stax has 36 KB of RAM, and a 16-input policy (30 joint signatures) is the
/// largest that fits while the whole request is held in memory. A vault with more deposits than
/// this cannot renew its policy on such a device.
pub const MAX_INPUTS: usize = 16;
/// Data bytes per APDU, in either direction.
pub const CHUNK: usize = 250;

pub mod ins {
    /// → `[PROTOCOL_VERSION]`
    pub const INFO: u8 = 0x01;
    /// p2 = network → the signer's 32-byte x-only vault key.
    pub const IDENTITY: u8 = 0x02;
    /// p2 = network → a download of `Option<State>`: the vault this device accepted, if any.
    pub const STATE: u8 = 0x03;
    /// p1 = 0 starts a request, 1 continues it; data is the next chunk.
    pub const UPLOAD: u8 = 0x10;
    /// p1 = [`super::Operation`], p2 = network → a download of the operation's answer.
    pub const RUN: u8 = 0x11;
    /// → the next chunk of the current download.
    pub const DOWNLOAD: u8 = 0x12;
    /// Development builds only → four big-endian `u16`s: the stack's size, its deepest use, its
    /// use now, and the heap's largest free block. p1 = 1 resets the deepest-use mark.
    pub const MEMORY: u8 = 0x7f;
}

pub mod status {
    pub const OK: u16 = 0x9000;
    /// The person rejected the review on the device.
    pub const REJECTED: u16 = 0x6985;
    /// The request was refused; the data is a short message saying why.
    pub const REFUSED: u16 = 0xb001;
    /// The upload, operation or download was out of order or too large.
    pub const BAD_SEQUENCE: u16 = 0xb002;
}

/// What `RUN` does with the uploaded request. Every operation that signs or forgets shows a
/// review on the device first.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum Operation {
    /// [`Pair`] → nothing. Accepts a vault; never approves spending.
    Pair = 1,
    /// `PolicyRequest` → `Vec<CooperativeSignature>`.
    Policy = 2,
    /// `SweepRequest` → `Vec<CooperativeSignature>`.
    Sweep = 3,
    /// `OfflineActionRequest` → `Vec<Signature>`.
    OfflineAction = 4,
    /// `RotationRequest` → [`RotationSignatures`]. The new vault is adopted separately.
    Rotation = 5,
    /// No request → nothing. Switches to the vault of the last approved rotation.
    AdoptRotation = 6,
    /// No request → nothing. Forgets the last approved policy, keeping the vault.
    ForgetPolicy = 7,
    /// No request → nothing. Forgets the vault, so the device can be set up again.
    ForgetVault = 8,
}

impl Operation {
    pub fn from_code(code: u8) -> Option<Self> {
        Some(match code {
            1 => Self::Pair,
            2 => Self::Policy,
            3 => Self::Sweep,
            4 => Self::OfflineAction,
            5 => Self::Rotation,
            6 => Self::AdoptRotation,
            7 => Self::ForgetPolicy,
            8 => Self::ForgetVault,
            _ => return None,
        })
    }
}

pub fn network_code(network: Network) -> u8 {
    match network {
        Network::Mainnet => 0,
        Network::Mutinynet => 1,
        Network::Regtest => 2,
    }
}

pub fn network_from_code(code: u8) -> Option<Network> {
    Some(match code {
        0 => Network::Mainnet,
        1 => Network::Mutinynet,
        2 => Network::Regtest,
        _ => return None,
    })
}

/// Accept the vault of this phone key and recovery locktime.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Pair {
    pub network: Network,
    pub phone: XOnlyKey,
    pub recovery_locktime: u32,
}

/// The signatures of an approved rotation, as `RotationApproval` carries them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RotationSignatures {
    pub sweep: Vec<CooperativeSignature>,
    pub revocation: Vec<Signature>,
    pub policy: Vec<CooperativeSignature>,
}

/// A download's first reply starts with the answer's total length.
pub fn download_header(total: usize) -> [u8; 2] {
    (total as u16).to_be_bytes()
}
