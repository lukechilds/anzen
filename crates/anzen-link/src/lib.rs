//! Links from a phone or computer to a hardware signer.
//!
//! The operating system owns the radio: this crate reaches it through `btleplug` (CoreBluetooth on
//! iOS and macOS, BlueZ on Linux), so one implementation serves the app and the CLI.
//!
//! [`ledger`] carries commands to the Anzen app on a Ledger. [`trezor`] opens Trezor's own
//! encrypted channel, loads the Anzen app over it and carries its requests. [`bridge`] answers the
//! phone app's protocol messages with either. None of it can approve anything: every signature
//! waits for the person at the device.

pub mod bridge;
pub mod ledger;
pub mod trezor;
