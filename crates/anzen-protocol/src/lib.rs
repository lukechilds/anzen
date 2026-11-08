#![no_std]

//! The Anzen protocol as pure, deterministic functions.
//!
//! Every device that participates in a vault derives the same scripts, transaction graphs and
//! BIP341 signature messages from the same high-level parameters. The phone uses these builders
//! to propose actions; the signer uses the same builders to reconstruct what it is asked to sign.
//! Nothing in this crate reads files, talks to a network, keeps state or generates randomness.
//!
//! Hashing and elliptic-curve arithmetic are supplied through [`crypto::Crypto`] so hardware
//! wallets can use their own primitives. The optional `software` feature provides a host
//! implementation.

extern crate alloc;

pub mod address;
pub mod consts;
pub mod crypto;
pub mod error;
pub mod hex;
pub mod musig;
pub mod policy;
pub mod script;
pub mod sighash;
pub mod spend;
pub mod tx;
pub mod wire;

pub use error::Error;

pub type Result<T> = core::result::Result<T, Error>;
