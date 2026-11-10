//! A Trezor running the Anzen app: Trezor's own encrypted protocol to the device, the app loaded
//! over it, and the app's requests on top.
//!
//! Proof of concept. It is exercised end to end against the Trezor emulator. Against a real
//! Safe 7 it has been run by hand from a Mac over Bluetooth: pairing by code, loading the app,
//! and a vault setup and policy.

mod app;
mod ble;
mod cpace;
pub mod debug;
mod proto;
mod signer;
mod thp;
mod udp;

pub use crate::bridge::{Message, Rejected};
pub use app::AppImage;
pub use ble::{Bluetooth, TrezorFound, open, scan};
pub use signer::{Directory, Memory, Slot, TrezorSigner, Volatile};
pub use thp::{CodeEntry, Failure, Host, Identity, Transport, Trezor};
pub use udp::{DEFAULT_PORT, Emulator};
