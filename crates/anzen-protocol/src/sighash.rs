//! BIP341 script-path signature messages with `SIGHASH_DEFAULT`.
//!
//! Anzen signs every input with `SIGHASH_DEFAULT`, which commits to all inputs, their amounts and
//! scripts, and all outputs. No other hash type is representable here.

use crate::{
    Error, Result,
    crypto::{Crypto, TAPSIGHASH_TAG, tagged_hash},
    tx::{LOCK_TIME, Transaction, TxOut, VERSION, write_compact_size, write_output},
};
use alloc::vec::Vec;

/// Precomputed per-transaction digests shared by every input's message.
pub struct SighashCache {
    prevouts: [u8; 32],
    amounts: [u8; 32],
    script_pubkeys: [u8; 32],
    sequences: [u8; 32],
    outputs: [u8; 32],
}

impl SighashCache {
    pub fn new<C: Crypto + ?Sized>(
        crypto: &mut C,
        transaction: &Transaction,
        prevouts: &[TxOut],
    ) -> Result<Self> {
        if prevouts.len() != transaction.input.len() {
            return Err(Error::PrevoutCountMismatch);
        }
        let mut buffer = Vec::new();
        for input in &transaction.input {
            buffer.extend_from_slice(&input.previous_output.txid.0);
            buffer.extend_from_slice(&input.previous_output.vout.to_le_bytes());
        }
        let prevout_hash = crypto.sha256(&[&buffer]);

        buffer.clear();
        for prevout in prevouts {
            buffer.extend_from_slice(&prevout.value.to_le_bytes());
        }
        let amounts = crypto.sha256(&[&buffer]);

        buffer.clear();
        for prevout in prevouts {
            write_compact_size(&mut buffer, prevout.script_pubkey.len() as u64);
            buffer.extend_from_slice(&prevout.script_pubkey);
        }
        let script_pubkeys = crypto.sha256(&[&buffer]);

        buffer.clear();
        for input in &transaction.input {
            buffer.extend_from_slice(&input.sequence.to_le_bytes());
        }
        let sequences = crypto.sha256(&[&buffer]);

        buffer.clear();
        for output in &transaction.output {
            write_output(&mut buffer, output);
        }
        let outputs = crypto.sha256(&[&buffer]);

        Ok(Self {
            prevouts: prevout_hash,
            amounts,
            script_pubkeys,
            sequences,
            outputs,
        })
    }

    /// The message a key signs to satisfy `leaf_hash` at `input_index`.
    pub fn script_path<C: Crypto + ?Sized>(
        &self,
        crypto: &mut C,
        input_index: usize,
        leaf_hash: &[u8; 32],
    ) -> [u8; 32] {
        let mut message = Vec::with_capacity(210);
        message.push(0); // Sighash epoch.
        message.push(0); // SIGHASH_DEFAULT.
        message.extend_from_slice(&VERSION.to_le_bytes());
        message.extend_from_slice(&LOCK_TIME.to_le_bytes());
        message.extend_from_slice(&self.prevouts);
        message.extend_from_slice(&self.amounts);
        message.extend_from_slice(&self.script_pubkeys);
        message.extend_from_slice(&self.sequences);
        message.extend_from_slice(&self.outputs);
        message.push(2); // ext_flag = 1 (script path), no annex.
        message.extend_from_slice(&(input_index as u32).to_le_bytes());
        message.extend_from_slice(leaf_hash);
        message.push(0); // Key version 0.
        message.extend_from_slice(&u32::MAX.to_le_bytes()); // No OP_CODESEPARATOR.
        tagged_hash(crypto, &TAPSIGHASH_TAG, &[&message])
    }
}
