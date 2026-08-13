#![no_std]

//! Platform-independent cold-signer logic shared by hardware-wallet apps.
//!
//! This crate intentionally starts with no signing surface. Protocol parsing,
//! policy validation, and signing will move here as the device protocol is
//! implemented, without pulling filesystem, network, or hot-wallet code onto
//! the signer.

pub mod benchmark;

/// Name presented by hardware-wallet implementations of the protocol.
pub const PROTOCOL_NAME: &str = "Anzen";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn protocol_name_is_stable() {
        assert_eq!(PROTOCOL_NAME, "Anzen");
    }
}
