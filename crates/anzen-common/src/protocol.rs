//! Conversions between rust-bitcoin wallet types and the `anzen_protocol` builders.
//!
//! The protocol crate owns every transaction shape and signature message. This module only
//! translates identifiers and serializations so PSBT tooling can operate on the same bytes.

use super::{storage::VaultConfig, types::VaultUtxo};
use anyhow::{Context, Result, bail};
pub use anzen_protocol::crypto::SoftwareCrypto;
use anzen_protocol::{
    address::Network as ProtocolNetwork,
    policy::Input,
    script::{VaultKeys, XOnlyKey},
    tx,
};
use bitcoin::{Network, OutPoint, Transaction, Txid, hashes::Hash, secp256k1::XOnlyPublicKey};
use std::str::FromStr;

pub fn network(network: Network) -> Result<ProtocolNetwork> {
    Ok(match network {
        Network::Bitcoin => ProtocolNetwork::Mainnet,
        Network::Signet => ProtocolNetwork::Mutinynet,
        Network::Regtest => ProtocolNetwork::Regtest,
        other => bail!("unsupported vault network: {other}"),
    })
}

pub fn bitcoin_network(network: ProtocolNetwork) -> Network {
    match network {
        ProtocolNetwork::Mainnet => Network::Bitcoin,
        ProtocolNetwork::Mutinynet => Network::Signet,
        ProtocolNetwork::Regtest => Network::Regtest,
    }
}

pub fn key(key: XOnlyPublicKey) -> XOnlyKey {
    XOnlyKey(key.serialize())
}

pub fn bitcoin_key(key: XOnlyKey) -> Result<XOnlyPublicKey> {
    XOnlyPublicKey::from_slice(&key.0).context("invalid x-only public key")
}

/// The two device keys and recovery locktime of a validated vault configuration.
pub fn vault_keys(config: &VaultConfig) -> Result<VaultKeys> {
    Ok(VaultKeys {
        phone: key(XOnlyPublicKey::from_str(&config.phone_vault_pubkey)
            .context("invalid configured phone vault key")?),
        hww: key(XOnlyPublicKey::from_str(&config.hww_vault_pubkey)
            .context("invalid configured HWW vault key")?),
        recovery_locktime: config.recovery_locktime,
    })
}

pub fn config_network(config: &VaultConfig) -> Result<ProtocolNetwork> {
    network(config.bitcoin_network()?)
}

pub fn outpoint(outpoint: OutPoint) -> tx::OutPoint {
    tx::OutPoint::new(tx::Txid(outpoint.txid.to_byte_array()), outpoint.vout)
}

pub fn bitcoin_outpoint(outpoint: tx::OutPoint) -> OutPoint {
    OutPoint::new(Txid::from_byte_array(outpoint.txid.0), outpoint.vout)
}

pub fn txid(txid: tx::Txid) -> Txid {
    Txid::from_byte_array(txid.0)
}

pub fn input(utxo: &VaultUtxo) -> Input {
    Input {
        outpoint: outpoint(utxo.outpoint),
        value_sats: utxo.txout.value.to_sat(),
    }
}

pub fn inputs(utxos: &[VaultUtxo]) -> Vec<Input> {
    utxos.iter().map(input).collect()
}

/// The unsigned transaction exactly as the protocol serialized it.
pub fn transaction(transaction: &tx::Transaction) -> Result<Transaction> {
    bitcoin::consensus::deserialize(&transaction.serialize())
        .context("protocol transaction did not decode as a Bitcoin transaction")
}

pub fn prevouts(prevouts: &[tx::TxOut]) -> Vec<bitcoin::TxOut> {
    prevouts
        .iter()
        .map(|prevout| bitcoin::TxOut {
            value: bitcoin::Amount::from_sat(prevout.value),
            script_pubkey: bitcoin::ScriptBuf::from_bytes(prevout.script_pubkey.clone()),
        })
        .collect()
}

/// Map protocol errors, whose messages are stable user-facing text, into this crate's errors.
pub fn protocol<T>(result: anzen_protocol::Result<T>) -> Result<T> {
    result.map_err(|error| anyhow::anyhow!("{error}"))
}
