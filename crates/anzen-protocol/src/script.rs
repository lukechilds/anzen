//! The two fixed Taproot policies every vault uses.
//!
//! Vault principal: `tr(musig(phone,hww),and_v(v:after(T),and_v(v:older(4259839),pk(hww))))`:
//! both devices together through the key path with one MuSig2 signature, or the hardware wallet
//! alone after about 388 days (a time-based relative lock). `T` is a long-past absolute locktime
//! that never delays recovery; vanity searches grind it to pick the address.
//! Connectors: `tr(phone,pk(hww))`: the phone alone through the key path, as in every policy
//! action, or the hardware wallet alone through the only leaf when it revokes.

use crate::{
    Error, Result,
    consts::{HWW_RECOVERY_SEQUENCE, RECOVERY_LOCKTIMES},
    crypto::{Crypto, TAPLEAF_TAG, TAPTWEAK_TAG, TweakedKey, tagged_hash},
};
use alloc::vec::Vec;
use serde::{Deserialize, Serialize};

const OP_CHECKSIG: u8 = 0xac;
const OP_CLTV: u8 = 0xb1;
const OP_CSV: u8 = 0xb2;
const OP_VERIFY: u8 = 0x69;
const TAPSCRIPT_LEAF_VERSION: u8 = 0xc0;

/// Length of every P2TR scriptPubKey.
pub const P2TR_SCRIPT_LEN: usize = 34;
/// Length of every vault recovery leaf: its locktime is always a four-byte script number.
pub const RECOVERY_SCRIPT_LEN: usize = 47;

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
        crate::hex::serialize(&self.0, serializer)
    }
}

impl<'de> Deserialize<'de> for XOnlyKey {
    fn deserialize<D: serde::Deserializer<'de>>(
        deserializer: D,
    ) -> core::result::Result<Self, D::Error> {
        crate::hex::deserialize(deserializer).map(Self)
    }
}

/// The two device keys that define one vault epoch, and the recovery leaf's locktime.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct VaultKeys {
    pub phone: XOnlyKey,
    pub hww: XOnlyKey,
    /// The recovery leaf's `after()` value, from [`RECOVERY_LOCKTIMES`]. Recovery transactions
    /// use it as their lock time.
    pub recovery_locktime: u32,
}

/// `<locktime> OP_CLTV OP_VERIFY <sequence> OP_CSV OP_VERIFY <key> OP_CHECKSIG`
/// (`and_v(v:after(locktime),and_v(v:older(sequence),pk(key)))`).
pub fn recovery_script(key: &XOnlyKey, locktime: u32, sequence: u32) -> Vec<u8> {
    let mut script = push_script_number(i64::from(locktime));
    script.push(OP_CLTV);
    script.push(OP_VERIFY);
    script.extend(push_script_number(i64::from(sequence)));
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

fn output_key<C: Crypto + ?Sized>(
    crypto: &mut C,
    internal_key: &[u8; 32],
    merkle_root: [u8; 32],
) -> Result<TweakedKey> {
    let tweak = tagged_hash(crypto, &TAPTWEAK_TAG, &[internal_key, &merkle_root]);
    crypto.taproot_tweak(internal_key, &tweak)
}

fn validate_keys<C: Crypto + ?Sized>(crypto: &mut C, keys: &VaultKeys) -> Result<()> {
    if !crypto.is_valid_xonly_key(&keys.phone.0) || !crypto.is_valid_xonly_key(&keys.hww.0) {
        return Err(Error::InvalidKey);
    }
    if !RECOVERY_LOCKTIMES.contains(&keys.recovery_locktime) {
        return Err(Error::InvalidRecoveryLocktime);
    }
    Ok(())
}

/// The hash of the vault's only leaf, the signer's time-locked recovery path. It depends only on
/// the signer key and the locktime, not the phone key.
pub fn hww_recovery_leaf<C: Crypto + ?Sized>(
    crypto: &mut C,
    hww: &XOnlyKey,
    locktime: u32,
) -> [u8; 32] {
    tapleaf_hash(
        crypto,
        &recovery_script(hww, locktime, HWW_RECOVERY_SEQUENCE),
    )
}

/// The vault's output key from its internal key, signer key and recovery locktime. Vanity
/// searches compute the internal key once and call this per locktime.
pub fn vault_output_key<C: Crypto + ?Sized>(
    crypto: &mut C,
    internal_key: &[u8; 32],
    hww: &XOnlyKey,
    recovery_locktime: u32,
) -> Result<TweakedKey> {
    if !RECOVERY_LOCKTIMES.contains(&recovery_locktime) {
        return Err(Error::InvalidRecoveryLocktime);
    }
    let leaf = hww_recovery_leaf(crypto, hww, recovery_locktime);
    output_key(crypto, internal_key, leaf)
}

/// Internal key, leaf, output key and scriptPubKey of the principal-holding vault policy.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VaultScript {
    pub keys: VaultKeys,
    /// The MuSig2 aggregate of both keys; the key path spends with one joint signature.
    pub internal_key: [u8; 32],
    pub hww_recovery_leaf: [u8; 32],
    /// The tree has one leaf, so its root is that leaf's hash. Joint signatures commit to it
    /// through the Taproot tweak.
    pub merkle_root: [u8; 32],
    pub output: TweakedKey,
    pub script_pubkey: Vec<u8>,
}

impl VaultScript {
    pub fn new<C: Crypto + ?Sized>(crypto: &mut C, keys: &VaultKeys) -> Result<Self> {
        validate_keys(crypto, keys)?;
        let internal_key = crate::musig::aggregate_key(crypto, keys)?;
        let hww_recovery_leaf = hww_recovery_leaf(crypto, &keys.hww, keys.recovery_locktime);
        let output = output_key(crypto, &internal_key, hww_recovery_leaf)?;
        Ok(Self {
            keys: *keys,
            internal_key,
            hww_recovery_leaf,
            merkle_root: hww_recovery_leaf,
            script_pubkey: p2tr_script_pubkey(&output.output_key),
            output,
        })
    }
}

/// Leaf, output key and scriptPubKey of the 1-of-2 connector policy. The phone key is the internal
/// key: the phone spends connectors far more often, so its spends take the smaller key path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ControllerScript {
    pub keys: VaultKeys,
    pub hww_leaf: [u8; 32],
    /// The tree has one leaf, so its root is that leaf's hash. The phone's key-path signature
    /// commits to it through the Taproot tweak.
    pub merkle_root: [u8; 32],
    pub output: TweakedKey,
    pub script_pubkey: Vec<u8>,
}

impl ControllerScript {
    pub fn new<C: Crypto + ?Sized>(crypto: &mut C, keys: &VaultKeys) -> Result<Self> {
        validate_keys(crypto, keys)?;
        let hww_leaf = tapleaf_hash(crypto, &single_key_script(&keys.hww));
        let output = output_key(crypto, &keys.phone.0, hww_leaf)?;
        Ok(Self {
            keys: *keys,
            hww_leaf,
            merkle_root: hww_leaf,
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
