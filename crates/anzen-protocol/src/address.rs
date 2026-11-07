//! Networks and the address encodings a signer must display.

use crate::{Error, Result, crypto::Crypto, crypto::double_sha256};
use alloc::{string::String, vec::Vec};
use bech32::{Fe32, Hrp, segwit};
use serde::{Deserialize, Serialize};

/// The networks Anzen supports. Mutinynet uses signet address encoding.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Network {
    #[serde(alias = "bitcoin")]
    Mainnet,
    Mutinynet,
    Regtest,
}

impl Network {
    pub fn name(self) -> &'static str {
        match self {
            Self::Mainnet => "mainnet",
            Self::Mutinynet => "mutinynet",
            Self::Regtest => "regtest",
        }
    }

    pub fn from_name(name: &str) -> Result<Self> {
        match name {
            "mainnet" | "bitcoin" => Ok(Self::Mainnet),
            "mutinynet" => Ok(Self::Mutinynet),
            "regtest" => Ok(Self::Regtest),
            _ => Err(Error::WrongNetwork),
        }
    }

    fn hrp(self) -> Hrp {
        match self {
            Self::Mainnet => bech32::hrp::BC,
            Self::Mutinynet => bech32::hrp::TB,
            Self::Regtest => bech32::hrp::BCRT,
        }
    }

    fn base58_prefixes(self) -> (u8, u8) {
        match self {
            Self::Mainnet => (0x00, 0x05),
            Self::Mutinynet | Self::Regtest => (0x6f, 0xc4),
        }
    }
}

/// Decode an address into its scriptPubKey, rejecting addresses for any other network.
pub fn script_pubkey<C: Crypto + ?Sized>(
    crypto: &mut C,
    address: &str,
    network: Network,
) -> Result<Vec<u8>> {
    if let Ok((hrp, version, program)) = segwit::decode(address) {
        if hrp != network.hrp() {
            return Err(Error::WrongNetwork);
        }
        let version = version.to_u8();
        let mut script = Vec::with_capacity(program.len() + 2);
        script.push(if version == 0 { 0x00 } else { 0x50 + version });
        script.push(program.len() as u8);
        script.extend_from_slice(&program);
        return Ok(script);
    }
    let payload = base58check_decode(crypto, address)?;
    let (p2pkh, p2sh) = network.base58_prefixes();
    if payload.len() != 21 {
        return Err(Error::InvalidAddress);
    }
    let hash = &payload[1..];
    let mut script = Vec::with_capacity(25);
    if payload[0] == p2pkh {
        script.extend_from_slice(&[0x76, 0xa9, 20]);
        script.extend_from_slice(hash);
        script.extend_from_slice(&[0x88, 0xac]);
    } else if payload[0] == p2sh {
        script.extend_from_slice(&[0xa9, 20]);
        script.extend_from_slice(hash);
        script.push(0x87);
    } else if [0x00, 0x05, 0x6f, 0xc4].contains(&payload[0]) {
        return Err(Error::WrongNetwork);
    } else {
        return Err(Error::InvalidAddress);
    }
    Ok(script)
}

/// Decode an address that must be a Taproot (P2TR) output.
pub fn taproot_script_pubkey<C: Crypto + ?Sized>(
    crypto: &mut C,
    address: &str,
    network: Network,
) -> Result<Vec<u8>> {
    let script = script_pubkey(crypto, address, network)?;
    if !crate::script::is_p2tr(&script) {
        return Err(Error::UnsupportedDestination);
    }
    Ok(script)
}

/// Encode a standard scriptPubKey for display.
pub fn from_script_pubkey<C: Crypto + ?Sized>(
    crypto: &mut C,
    script: &[u8],
    network: Network,
) -> Result<String> {
    let is_witness = script.len() >= 4
        && script.len() <= 42
        && (script[0] == 0x00 || (0x51..=0x60).contains(&script[0]))
        && usize::from(script[1]) + 2 == script.len();
    if is_witness {
        let version = if script[0] == 0 { 0 } else { script[0] - 0x50 };
        let version = Fe32::try_from(version).map_err(|_| Error::InvalidAddress)?;
        return segwit::encode(network.hrp(), version, &script[2..])
            .map_err(|_| Error::InvalidAddress);
    }
    let (p2pkh, p2sh) = network.base58_prefixes();
    if script.len() == 25 && script[..3] == [0x76, 0xa9, 20] && script[23..] == [0x88, 0xac] {
        return Ok(base58check_encode(crypto, p2pkh, &script[3..23]));
    }
    if script.len() == 23 && script[..2] == [0xa9, 20] && script[22] == 0x87 {
        return Ok(base58check_encode(crypto, p2sh, &script[2..22]));
    }
    Err(Error::InvalidAddress)
}

const BASE58: &[u8; 58] = b"123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz";

fn base58check_decode<C: Crypto + ?Sized>(crypto: &mut C, text: &str) -> Result<Vec<u8>> {
    if text.is_empty() || text.len() > 64 {
        return Err(Error::InvalidAddress);
    }
    let mut bytes: Vec<u8> = Vec::new();
    for character in text.bytes() {
        let mut carry = BASE58
            .iter()
            .position(|digit| *digit == character)
            .ok_or(Error::InvalidAddress)? as u32;
        for byte in bytes.iter_mut().rev() {
            carry += u32::from(*byte) * 58;
            *byte = (carry & 0xff) as u8;
            carry >>= 8;
        }
        while carry > 0 {
            bytes.insert(0, (carry & 0xff) as u8);
            carry >>= 8;
        }
    }
    let leading_zeros = text.bytes().take_while(|byte| *byte == b'1').count();
    let mut decoded = alloc::vec![0_u8; leading_zeros];
    decoded.extend_from_slice(&bytes);
    if decoded.len() < 4 {
        return Err(Error::InvalidAddress);
    }
    let (payload, checksum) = decoded.split_at(decoded.len() - 4);
    if double_sha256(crypto, payload)[..4] != *checksum {
        return Err(Error::InvalidAddress);
    }
    Ok(payload.to_vec())
}

fn base58check_encode<C: Crypto + ?Sized>(crypto: &mut C, prefix: u8, hash: &[u8]) -> String {
    let mut payload = Vec::with_capacity(hash.len() + 5);
    payload.push(prefix);
    payload.extend_from_slice(hash);
    let checksum = double_sha256(crypto, &payload);
    payload.extend_from_slice(&checksum[..4]);
    let mut digits: Vec<u8> = Vec::new();
    for byte in &payload {
        let mut carry = u32::from(*byte);
        for digit in digits.iter_mut() {
            carry += u32::from(*digit) << 8;
            *digit = (carry % 58) as u8;
            carry /= 58;
        }
        while carry > 0 {
            digits.push((carry % 58) as u8);
            carry /= 58;
        }
    }
    let mut text = String::new();
    for _ in payload.iter().take_while(|byte| **byte == 0) {
        text.push('1');
    }
    for digit in digits.iter().rev() {
        text.push(BASE58[usize::from(*digit)] as char);
    }
    text
}
