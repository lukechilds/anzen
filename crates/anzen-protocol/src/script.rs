//! The two fixed Taproot policies every vault uses.
//!
//! Vault principal: `tr(NUMS,{multi_a(2,phone,hww),{and_v(v:older(61200),pk(phone)),
//! and_v(v:older(65535),pk(hww))}})`. Connectors: `tr(NUMS,{pk(phone),pk(hww)})`.

use crate::{
    Error, Result,
    consts::{HWW_RECOVERY_BLOCKS, NUMS_KEY, PHONE_RECOVERY_BLOCKS},
    crypto::{Crypto, TAPBRANCH_TAG, TAPLEAF_TAG, TAPTWEAK_TAG, TweakedKey, tagged_hash},
};
use alloc::vec::Vec;
use serde::{Deserialize, Serialize};

const OP_CHECKSIG: u8 = 0xac;
const OP_CHECKSIGADD: u8 = 0xba;
const OP_NUMEQUAL: u8 = 0x9c;
const OP_CSV: u8 = 0xb2;
const OP_VERIFY: u8 = 0x69;
const OP_2: u8 = 0x52;
const TAPSCRIPT_LEAF_VERSION: u8 = 0xc0;

/// Length of every P2TR scriptPubKey.
pub const P2TR_SCRIPT_LEN: usize = 34;

/// A BIP340 x-only public key.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct XOnlyKey(pub [u8; 32]);

impl XOnlyKey {
    pub fn to_hex(&self) -> alloc::string::String {
        crate::hex::encode(&self.0)
    }

    pub fn from_hex(text: &str) -> Result<Self> {
        crate::hex::decode_array(text)
            .map(Self)
            .map_err(|_| Error::InvalidKey)
    }
}

impl Serialize for XOnlyKey {
    fn serialize<S: serde::Serializer>(
        &self,
        serializer: S,
    ) -> core::result::Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.to_hex())
    }
}

impl<'de> Deserialize<'de> for XOnlyKey {
    fn deserialize<D: serde::Deserializer<'de>>(
        deserializer: D,
    ) -> core::result::Result<Self, D::Error> {
        let text = <alloc::borrow::Cow<'de, str>>::deserialize(deserializer)?;
        Self::from_hex(&text).map_err(serde::de::Error::custom)
    }
}

/// The two device keys that define one vault epoch.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct VaultKeys {
    pub phone: XOnlyKey,
    pub hww: XOnlyKey,
}

/// `<phone> OP_CHECKSIG <hww> OP_CHECKSIGADD 2 OP_NUMEQUAL` (`multi_a(2,phone,hww)`).
pub fn cooperative_script(keys: &VaultKeys) -> [u8; 70] {
    let mut script = [0_u8; 70];
    script[0] = 32;
    script[1..33].copy_from_slice(&keys.phone.0);
    script[33] = OP_CHECKSIG;
    script[34] = 32;
    script[35..67].copy_from_slice(&keys.hww.0);
    script[67] = OP_CHECKSIGADD;
    script[68] = OP_2;
    script[69] = OP_NUMEQUAL;
    script
}

/// `<delay> OP_CSV OP_VERIFY <key> OP_CHECKSIG` (`and_v(v:older(delay),pk(key))`).
pub fn recovery_script(key: &XOnlyKey, delay: u16) -> Vec<u8> {
    let mut script = push_script_number(i64::from(delay));
    script.push(OP_CSV);
    script.push(OP_VERIFY);
    script.push(32);
    script.extend_from_slice(&key.0);
    script.push(OP_CHECKSIG);
    script
}

/// `<key> OP_CHECKSIG` (`pk(key)`).
pub fn single_key_script(key: &XOnlyKey) -> [u8; 34] {
    let mut script = [0_u8; 34];
    script[0] = 32;
    script[1..33].copy_from_slice(&key.0);
    script[33] = OP_CHECKSIG;
    script
}

/// `OP_1 <32-byte output key>`.
pub fn p2tr_script_pubkey(output_key: &[u8; 32]) -> Vec<u8> {
    let mut script = Vec::with_capacity(P2TR_SCRIPT_LEN);
    script.push(0x51);
    script.push(32);
    script.extend_from_slice(output_key);
    script
}

pub fn is_p2tr(script_pubkey: &[u8]) -> bool {
    script_pubkey.len() == P2TR_SCRIPT_LEN && script_pubkey[0] == 0x51 && script_pubkey[1] == 32
}

pub fn tapleaf_hash<C: Crypto + ?Sized>(crypto: &mut C, script: &[u8]) -> [u8; 32] {
    // Every Anzen leaf is shorter than 253 bytes, so its compact-size length is one byte.
    debug_assert!(script.len() < 0xfd);
    tagged_hash(
        crypto,
        &TAPLEAF_TAG,
        &[&[TAPSCRIPT_LEAF_VERSION], &[script.len() as u8], script],
    )
}

pub fn tapbranch_hash<C: Crypto + ?Sized>(
    crypto: &mut C,
    left: [u8; 32],
    right: [u8; 32],
) -> [u8; 32] {
    let (first, second) = if left <= right {
        (left, right)
    } else {
        (right, left)
    };
    tagged_hash(crypto, &TAPBRANCH_TAG, &[&first, &second])
}

fn output_key<C: Crypto + ?Sized>(crypto: &mut C, merkle_root: [u8; 32]) -> Result<TweakedKey> {
    let tweak = tagged_hash(crypto, &TAPTWEAK_TAG, &[&NUMS_KEY, &merkle_root]);
    crypto.taproot_tweak(&NUMS_KEY, &tweak)
}

fn validate_keys<C: Crypto + ?Sized>(crypto: &mut C, keys: &VaultKeys) -> Result<()> {
    if !crypto.is_valid_xonly_key(&keys.phone.0) || !crypto.is_valid_xonly_key(&keys.hww.0) {
        return Err(Error::InvalidKey);
    }
    Ok(())
}

/// Leaf hashes, output key and scriptPubKey of the principal-holding vault policy.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VaultScript {
    pub keys: VaultKeys,
    pub cooperative_leaf: [u8; 32],
    pub phone_recovery_leaf: [u8; 32],
    pub hww_recovery_leaf: [u8; 32],
    pub output: TweakedKey,
    pub script_pubkey: Vec<u8>,
}

impl VaultScript {
    pub fn new<C: Crypto + ?Sized>(crypto: &mut C, keys: &VaultKeys) -> Result<Self> {
        validate_keys(crypto, keys)?;
        let cooperative_leaf = tapleaf_hash(crypto, &cooperative_script(keys));
        let phone_recovery_leaf =
            tapleaf_hash(crypto, &recovery_script(&keys.phone, PHONE_RECOVERY_BLOCKS));
        let hww_recovery_leaf =
            tapleaf_hash(crypto, &recovery_script(&keys.hww, HWW_RECOVERY_BLOCKS));
        let recoveries = tapbranch_hash(crypto, phone_recovery_leaf, hww_recovery_leaf);
        let root = tapbranch_hash(crypto, cooperative_leaf, recoveries);
        let output = output_key(crypto, root)?;
        Ok(Self {
            keys: *keys,
            cooperative_leaf,
            phone_recovery_leaf,
            hww_recovery_leaf,
            script_pubkey: p2tr_script_pubkey(&output.output_key),
            output,
        })
    }
}

/// Leaf hashes, output key and scriptPubKey of the 1-of-2 connector policy.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ControllerScript {
    pub keys: VaultKeys,
    pub phone_leaf: [u8; 32],
    pub hww_leaf: [u8; 32],
    pub output: TweakedKey,
    pub script_pubkey: Vec<u8>,
}

impl ControllerScript {
    pub fn new<C: Crypto + ?Sized>(crypto: &mut C, keys: &VaultKeys) -> Result<Self> {
        validate_keys(crypto, keys)?;
        let phone_leaf = tapleaf_hash(crypto, &single_key_script(&keys.phone));
        let hww_leaf = tapleaf_hash(crypto, &single_key_script(&keys.hww));
        let root = tapbranch_hash(crypto, phone_leaf, hww_leaf);
        let output = output_key(crypto, root)?;
        Ok(Self {
            keys: *keys,
            phone_leaf,
            hww_leaf,
            script_pubkey: p2tr_script_pubkey(&output.output_key),
            output,
        })
    }
}

/// Both scripts of one vault epoch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Scripts {
    pub vault: VaultScript,
    pub controller: ControllerScript,
}

impl Scripts {
    pub fn new<C: Crypto + ?Sized>(crypto: &mut C, keys: &VaultKeys) -> Result<Self> {
        Ok(Self {
            vault: VaultScript::new(crypto, keys)?,
            controller: ControllerScript::new(crypto, keys)?,
        })
    }
}

/// Minimal CScriptNum push, matching `script::Builder::push_int`.
fn push_script_number(value: i64) -> Vec<u8> {
    match value {
        0 => alloc::vec![0x00],
        -1 => alloc::vec![0x4f],
        1..=16 => alloc::vec![0x50 + value as u8],
        _ => {
            let negative = value < 0;
            let mut magnitude = value.unsigned_abs();
            let mut bytes = Vec::new();
            while magnitude > 0 {
                bytes.push((magnitude & 0xff) as u8);
                magnitude >>= 8;
            }
            if bytes.last().is_some_and(|last| last & 0x80 != 0) {
                bytes.push(if negative { 0x80 } else { 0x00 });
            } else if negative {
                *bytes.last_mut().expect("nonzero value has a byte") |= 0x80;
            }
            let mut script = Vec::with_capacity(bytes.len() + 1);
            script.push(bytes.len() as u8);
            script.extend_from_slice(&bytes);
            script
        }
    }
}
