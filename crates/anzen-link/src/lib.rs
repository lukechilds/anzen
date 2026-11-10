//! Links from a phone or computer to a hardware signer.
//!
//! The operating system owns the radio: this crate reaches it through `btleplug` (CoreBluetooth on
//! iOS and macOS, BlueZ on Linux), so one implementation serves the app and the CLI. It carries
//! bytes only. What the bytes mean, and whether to trust the answer, is the caller's business.

pub mod ledger;
