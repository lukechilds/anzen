//! The protocol of a hardware signer app that stores nothing: its host carries its state.
//!
//! [`super`] is the protocol of a signer with storage of its own. A Trezor app has none, so
//! whatever the signer must remember (the vault it accepted, the last policy it approved, a
//! rotation it approved) goes back to the host under the signer's seal
//! ([`crate::software::SoftwareKey::seal`]), and the host returns it with the next request. The
//! host can lose it or withhold it, never change it; presenting an older one is possible, and
//! accepted (see the seal's notes).
//!
//! One request is one message and one answer: a message id and bytes each way, which is what a
//! Trezor app is given. Requests are the messages every signer reviews
//! ([`anzen_protocol::wire`]) in their compact form (postcard), whole, as in [`super`].
//!
//! A Trezor's link carries about 8.7 kB in one message, less than a large request or its
//! signatures, so either side sends a long body in pieces of at most [`PIECE`] bytes: the
//! receiver acknowledges each piece but the last, and puts them together before reading
//! anything. The app holds the whole request in memory either way; nothing is signed from a part.
//!
//! Its one implementation is the Trezor app (`trezor-app/`).

use super::Operation;
use alloc::vec::Vec;
use serde::{Deserialize, Serialize};

/// Bumped whenever a message or its encoding changes.
pub const PROTOCOL_VERSION: u8 = 1;

/// The most inputs one request may spend. The request, the transactions rebuilt from it and the
/// answer are all in the app's memory at once.
pub const MAX_INPUTS: usize = 64;
/// The most bytes of a body one message carries.
pub const PIECE: usize = 4096;
/// The largest request, all its pieces together.
pub const MAX_REQUEST: usize = 48 * 1024;

/// Message ids, host to device.
pub mod message {
    /// No data → `[PROTOCOL_VERSION]`.
    pub const INFO: u16 = 1;
    /// `[network code]` → the signer's 32-byte x-only vault key on that network.
    pub const IDENTITY: u16 = 2;
    /// `RUN + operation code`, data a [`super::Call`] → an [`super::Outcome`].
    pub const RUN: u16 = 0x10;
    /// A piece of the body of the message that follows, which carries the last piece
    /// → [`super::answer::MORE`].
    pub const PART: u16 = 0x7e;
    /// After [`super::answer::PART`]: send the next piece of the answer.
    pub const NEXT: u16 = 0x7f;
}

/// Message ids, device to host.
pub mod answer {
    /// The data is what the message's own notes say.
    pub const OK: u16 = 1;
    /// The person rejected the review on the device. No data.
    pub const REJECTED: u16 = 2;
    /// The request was refused; the data is a short message saying why.
    pub const REFUSED: u16 = 3;
    /// A piece of the request was taken; send the next.
    pub const MORE: u16 = 4;
    /// A piece of the answer, which is not finished: ask for the next with
    /// [`super::message::NEXT`]. The last piece comes with the answer's own id.
    pub const PART: u16 = 5;
}

/// The id of the message that runs `operation`. [`Operation::ForgetVault`] has none: a signer
/// that stores nothing has nothing to forget, and the host simply drops what it carried.
pub fn run(operation: Operation) -> u16 {
    message::RUN + operation as u16
}

pub fn operation(message: u16) -> Option<Operation> {
    let code = u8::try_from(message.checked_sub(message::RUN)?).ok()?;
    Operation::from_code(code).filter(|operation| *operation != Operation::ForgetVault)
}

/// State the signer sealed for its host to keep: a [`crate::State`] in compact form, and the
/// signer's seal over it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Sealed {
    pub state: Vec<u8>,
    pub seal: [u8; 32],
}

/// One operation's request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Call {
    /// What the signer would have had stored: the vault's state for every operation but
    /// [`Operation::Pair`], which takes none, and [`Operation::AdoptRotation`], which takes the
    /// state its rotation produced.
    pub carried: Option<Sealed>,
    /// The operation's own request (see [`Operation`]), in compact form. [`super::Pair`] names
    /// its network; an operation with no request of its own takes one byte, the network's code.
    pub request: Vec<u8>,
}

/// One operation's answer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Outcome {
    /// What the signer would have stored, for the host to keep in its place: the vault's new
    /// state, or after [`Operation::Rotation`] the state [`Operation::AdoptRotation`] takes.
    /// `None` leaves what the host carries as it was.
    pub carry: Option<Sealed>,
    /// The operation's own answer (see [`Operation`]), in compact form; empty if it has none.
    pub answer: Vec<u8>,
}
