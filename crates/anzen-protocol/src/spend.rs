//! Single-output spends: vault sweeps through any vault path and connector revocations.

use crate::{
    Error, Result,
    consts::{FEE_RATE_SAT_VB, HWW_RECOVERY_SEQUENCE, MAX_INPUTS, SEQUENCE_FINAL, SEQUENCE_RBF},
    crypto::Crypto,
    policy::Input,
    script::Scripts,
    sighash::SighashCache,
    tx::{OutPoint, SpendPath, Transaction, TxIn, TxOut, Txid, minimal_non_dust},
};
use alloc::{collections::BTreeSet, vec, vec::Vec};

/// A fully determined single-output transaction and its signature context.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Spend {
    pub transaction: Transaction,
    pub txid: Txid,
    pub prevouts: Vec<TxOut>,
    pub spends: Vec<SpendPath>,
    pub amount_sats: u64,
    pub fee_sats: u64,
}

impl Spend {
    /// Key-path signature messages for every input: the vault's joint MuSig2 signatures.
    pub fn key_path_sighashes<C: Crypto + ?Sized>(&self, crypto: &mut C) -> Result<Vec<[u8; 32]>> {
        let cache = SighashCache::new(crypto, &self.transaction, &self.prevouts)?;
        Ok((0..self.transaction.input.len())
            .map(|index| cache.key_path(crypto, index))
            .collect())
    }

    /// Signature messages for every input, all satisfied through `leaf_hash`.
    pub fn sighashes<C: Crypto + ?Sized>(
        &self,
        crypto: &mut C,
        leaf_hash: &[u8; 32],
    ) -> Result<Vec<[u8; 32]>> {
        let cache = SighashCache::new(crypto, &self.transaction, &self.prevouts)?;
        Ok((0..self.transaction.input.len())
            .map(|index| cache.script_path(crypto, index, leaf_hash))
            .collect())
    }
}

/// The relative lock each vault spend path requires.
pub fn vault_sequence(path: SpendPath) -> u32 {
    match path {
        SpendPath::HwwRecovery => HWW_RECOVERY_SEQUENCE,
        _ => SEQUENCE_FINAL,
    }
}

/// Sweep vault outputs through one vault path to a single destination.
///
/// The destination receives everything left after the fixed fee. Callers that must avoid dust
/// outputs check [`Spend::amount_sats`] against their own policy.
pub fn vault_sweep<C: Crypto + ?Sized>(
    crypto: &mut C,
    scripts: &Scripts,
    inputs: &[Input],
    path: SpendPath,
    destination: &[u8],
) -> Result<Spend> {
    if path.is_controller() {
        return Err(Error::UnsupportedDestination);
    }
    single_output(
        crypto,
        inputs,
        &scripts.vault.script_pubkey,
        path,
        vault_sequence(path),
        destination,
    )
}

/// Spend connectors through one device's controller leaf, creating no new controller output.
pub fn controller_revocation<C: Crypto + ?Sized>(
    crypto: &mut C,
    scripts: &Scripts,
    connectors: &[Input],
    path: SpendPath,
    destination: &[u8],
) -> Result<Spend> {
    if !path.is_controller() {
        return Err(Error::UnsupportedDestination);
    }
    let spend = single_output(
        crypto,
        connectors,
        &scripts.controller.script_pubkey,
        path,
        SEQUENCE_RBF,
        destination,
    )?;
    if spend.amount_sats < minimal_non_dust(destination) {
        return Err(Error::DustDestination);
    }
    Ok(spend)
}

fn single_output<C: Crypto + ?Sized>(
    crypto: &mut C,
    inputs: &[Input],
    script_pubkey: &[u8],
    path: SpendPath,
    sequence: u32,
    destination: &[u8],
) -> Result<Spend> {
    if inputs.is_empty() || inputs.len() > MAX_INPUTS {
        return Err(Error::TooManyInputs);
    }
    let mut seen = BTreeSet::<OutPoint>::new();
    if inputs.iter().any(|input| !seen.insert(input.outpoint)) {
        return Err(Error::DuplicateInput);
    }
    let input_sats = inputs.iter().try_fold(0_u64, |sum, input| {
        sum.checked_add(input.value_sats)
            .ok_or(Error::AmountOverflow)
    })?;
    let mut transaction = Transaction {
        input: inputs
            .iter()
            .map(|input| TxIn {
                previous_output: input.outpoint,
                sequence,
            })
            .collect(),
        output: vec![TxOut {
            value: input_sats,
            script_pubkey: destination.to_vec(),
        }],
    };
    let spends = vec![path; inputs.len()];
    let fee_sats = transaction
        .estimated_vsize(&spends)?
        .checked_mul(FEE_RATE_SAT_VB)
        .ok_or(Error::AmountOverflow)?;
    let amount_sats = input_sats
        .checked_sub(fee_sats)
        .ok_or(Error::InsufficientFee)?;
    transaction.output[0].value = amount_sats;
    Ok(Spend {
        txid: transaction.txid(crypto),
        prevouts: inputs
            .iter()
            .map(|input| TxOut {
                value: input.value_sats,
                script_pubkey: script_pubkey.to_vec(),
            })
            .collect(),
        transaction,
        spends,
        amount_sats,
        fee_sats,
    })
}
