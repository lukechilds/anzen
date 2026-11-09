//! Messages between the phone and the signer.
//!
//! Requests carry parameters, never transactions: the signer rebuilds every transaction with the
//! same builders the phone used and returns only signatures.
//!
//! Vault spends that need both devices use one MuSig2 (BIP327) signature through the key path.
//! The phone signs first: its requests carry one public nonce per joint signature. The signer
//! signs last and returns its own public nonce with its partial signature, which lets it derive
//! that nonce deterministically and keep no state. Input values are taken on trust
//! because `SIGHASH_DEFAULT` commits to every input amount and script: a signature over a false
//! value is invalid on chain, so a valid transaction always moves exactly what was reviewed.

use crate::{
    Error, Result,
    address::{self, Network},
    consts::MAX_INPUTS,
    crypto::Crypto,
    hex,
    policy::{self, Destinations, EpochAnchor, Input, Limits, PolicyGraph, PolicyInputs},
    script::{Scripts, VaultKeys, XOnlyKey},
    spend::{self, Spend},
    tx::{OutPoint, SpendPath},
};
use alloc::{collections::BTreeSet, string::String, vec::Vec};
use serde::{Deserialize, Serialize};

pub const VERSION: u8 = 3;

/// A BIP340 signature.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Signature(pub [u8; 64]);

impl Serialize for Signature {
    fn serialize<S: serde::Serializer>(
        &self,
        serializer: S,
    ) -> core::result::Result<S::Ok, S::Error> {
        serializer.serialize_str(&hex::encode(&self.0))
    }
}

impl<'de> Deserialize<'de> for Signature {
    fn deserialize<D: serde::Deserializer<'de>>(
        deserializer: D,
    ) -> core::result::Result<Self, D::Error> {
        let text = <alloc::borrow::Cow<'de, str>>::deserialize(deserializer)?;
        hex::decode_array(&text)
            .map(Self)
            .map_err(serde::de::Error::custom)
    }
}

/// A MuSig2 public nonce: two compressed points (BIP327 `PubNonce`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PubNonce(pub [u8; 66]);

/// A MuSig2 partial signature (BIP327 `PartialSig`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PartialSignature(pub [u8; 32]);

macro_rules! hex_serde {
    ($type:ty) => {
        impl Serialize for $type {
            fn serialize<S: serde::Serializer>(
                &self,
                serializer: S,
            ) -> core::result::Result<S::Ok, S::Error> {
                serializer.serialize_str(&hex::encode(&self.0))
            }
        }

        impl<'de> Deserialize<'de> for $type {
            fn deserialize<D: serde::Deserializer<'de>>(
                deserializer: D,
            ) -> core::result::Result<Self, D::Error> {
                let text = <alloc::borrow::Cow<'de, str>>::deserialize(deserializer)?;
                hex::decode_array(&text)
                    .map(Self)
                    .map_err(serde::de::Error::custom)
            }
        }
    };
}
hex_serde!(PubNonce);
hex_serde!(PartialSignature);

/// The signer's half of one joint signature: its public nonce and partial signature.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CooperativeSignature {
    pub nonce: PubNonce,
    pub partial: PartialSignature,
}

/// Every joint signature needs exactly one phone nonce.
pub fn check_nonces(nonces: &[PubNonce], signatures: usize) -> Result<()> {
    if nonces.len() != signatures {
        return Err(Error::NonceCountMismatch);
    }
    Ok(())
}

/// Approve the annual policy these parameters define.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PolicyRequest {
    pub version: u8,
    pub network: Network,
    pub keys: VaultKeys,
    pub vault_inputs: Vec<Input>,
    #[serde(default)]
    pub controller_inputs: Vec<Input>,
    pub limits: Limits,
    pub emergency_destination: Option<String>,
    pub allowance_destinations: Vec<String>,
    /// The phone reports that every allowance of the previously approved policy was used.
    #[serde(default)]
    pub previous_allowances_complete: bool,
    /// The phone's nonce for each joint signature, in
    /// [`PolicyGraph::cooperative_signing_jobs`] order.
    pub phone_nonces: Vec<PubNonce>,
}

impl PolicyRequest {
    pub fn build<C: Crypto + ?Sized>(&self, crypto: &mut C) -> Result<(Scripts, PolicyGraph)> {
        if self.version != VERSION {
            return Err(Error::UnsupportedVersion);
        }
        let scripts = Scripts::new(crypto, &self.keys)?;
        let destinations = Destinations {
            emergency: self
                .emergency_destination
                .as_deref()
                .map(|text| address::taproot_script_pubkey(crypto, text, self.network))
                .transpose()?,
            allowances: self
                .allowance_destinations
                .iter()
                .map(|text| address::taproot_script_pubkey(crypto, text, self.network))
                .collect::<Result<_>>()?,
        };
        let graph = policy::build(
            crypto,
            &PolicyInputs {
                scripts: &scripts,
                vault: &self.vault_inputs,
                controllers: &self.controller_inputs,
                limits: self.limits,
            },
            &destinations,
        )?;
        Ok((scripts, graph))
    }

    pub fn spent_outpoints(&self) -> BTreeSet<OutPoint> {
        self.vault_inputs
            .iter()
            .chain(&self.controller_inputs)
            .map(|input| input.outpoint)
            .collect()
    }
}

/// The signer's halves of the joint signatures for a [`PolicyRequest`], in
/// [`PolicyGraph::cooperative_signing_jobs`] order.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PolicyApproval {
    pub version: u8,
    pub request: PolicyRequest,
    pub signatures: Vec<CooperativeSignature>,
}

/// Move vault outputs to one destination through the cooperative path.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SweepRequest {
    pub version: u8,
    pub network: Network,
    pub keys: VaultKeys,
    pub inputs: Vec<Input>,
    pub destination: String,
    /// The phone's nonce for each input's joint signature.
    pub phone_nonces: Vec<PubNonce>,
}

impl SweepRequest {
    pub fn build<C: Crypto + ?Sized>(&self, crypto: &mut C) -> Result<(Scripts, Spend)> {
        if self.version != VERSION {
            return Err(Error::UnsupportedVersion);
        }
        let scripts = Scripts::new(crypto, &self.keys)?;
        let destination = address::script_pubkey(crypto, &self.destination, self.network)?;
        let spend = spend::vault_sweep(
            crypto,
            &scripts,
            &self.inputs,
            SpendPath::Cooperative,
            &destination,
        )?;
        if spend.amount_sats < crate::tx::minimal_non_dust(&destination) {
            return Err(Error::DustDestination);
        }
        Ok((scripts, spend))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SweepApproval {
    pub version: u8,
    pub request: SweepRequest,
    pub signatures: Vec<CooperativeSignature>,
}

/// Actions the signer can take alone.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OfflineAction {
    /// Sweep vault outputs through the HWW recovery path after its relative delay.
    HwwRecovery,
    /// Spend live connectors through the HWW controller leaf, invalidating their actions.
    RevokeActions,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OfflineActionRequest {
    pub version: u8,
    pub network: Network,
    pub keys: VaultKeys,
    pub action: OfflineAction,
    pub inputs: Vec<Input>,
    pub destination: String,
}

impl OfflineActionRequest {
    pub fn build<C: Crypto + ?Sized>(&self, crypto: &mut C) -> Result<(Scripts, Spend, [u8; 32])> {
        if self.version != VERSION {
            return Err(Error::UnsupportedVersion);
        }
        let scripts = Scripts::new(crypto, &self.keys)?;
        let destination = address::script_pubkey(crypto, &self.destination, self.network)?;
        let (spend, leaf) = match self.action {
            OfflineAction::HwwRecovery => (
                spend::vault_sweep(
                    crypto,
                    &scripts,
                    &self.inputs,
                    SpendPath::HwwRecovery,
                    &destination,
                )?,
                scripts.vault.hww_recovery_leaf,
            ),
            OfflineAction::RevokeActions => (
                spend::controller_revocation(
                    crypto,
                    &scripts,
                    &self.inputs,
                    SpendPath::ControllerHww,
                    &destination,
                )?,
                scripts.controller.hww_leaf,
            ),
        };
        if spend.amount_sats < crate::tx::minimal_non_dust(&destination) {
            return Err(Error::DustDestination);
        }
        Ok((scripts, spend, leaf))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OfflineActionApproval {
    pub version: u8,
    pub request: OfflineActionRequest,
    pub signatures: Vec<Signature>,
}

/// Replace the phone key: revoke old connectors to the new vault, sweep the old vault into the
/// new one and, when a policy is enabled, renew it on the sweep output under the new keys.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RotationRequest {
    pub version: u8,
    pub network: Network,
    pub old_keys: VaultKeys,
    pub new_phone: XOnlyKey,
    /// The new vault's recovery locktime: ground when the new phone key has a vanity address.
    pub new_recovery_locktime: u32,
    pub sweep_inputs: Vec<Input>,
    /// Old connectors the signer revokes through its own controller leaf.
    #[serde(default)]
    pub revoke_connectors: Vec<Input>,
    pub renewed_policy: Option<PolicyRequest>,
    #[serde(default)]
    pub previous_allowances_complete: bool,
    /// The phone's nonce, under the old keys, for each sweep input's joint signature. The
    /// renewed policy carries its own nonces under the new keys.
    pub sweep_nonces: Vec<PubNonce>,
}

/// Every transaction a rotation request asks the signer to sign.
pub struct RotationPlan {
    pub old_scripts: Scripts,
    pub new_keys: VaultKeys,
    pub new_scripts: Scripts,
    pub sweep: Spend,
    pub revocation: Option<Spend>,
    pub renewed: Option<(Scripts, PolicyGraph)>,
}

impl RotationRequest {
    /// The rotated vault keeps the signer key.
    pub fn new_keys(&self) -> VaultKeys {
        VaultKeys {
            phone: self.new_phone,
            hww: self.old_keys.hww,
            recovery_locktime: self.new_recovery_locktime,
        }
    }

    pub fn build<C: Crypto + ?Sized>(&self, crypto: &mut C) -> Result<RotationPlan> {
        if self.version != VERSION {
            return Err(Error::UnsupportedVersion);
        }
        if self.new_phone == self.old_keys.phone {
            return Err(Error::WrongVault);
        }
        let old_scripts = Scripts::new(crypto, &self.old_keys)?;
        let new_keys = self.new_keys();
        let new_scripts = Scripts::new(crypto, &new_keys)?;
        let sweep = spend::vault_sweep(
            crypto,
            &old_scripts,
            &self.sweep_inputs,
            SpendPath::Cooperative,
            &new_scripts.vault.script_pubkey,
        )?;
        let revocation = if self.revoke_connectors.is_empty() {
            None
        } else {
            Some(spend::controller_revocation(
                crypto,
                &old_scripts,
                &self.revoke_connectors,
                SpendPath::ControllerHww,
                &new_scripts.vault.script_pubkey,
            )?)
        };
        let renewed = self
            .renewed_policy
            .as_ref()
            .map(|policy| {
                check_renewal(policy, self.network, &new_keys, &sweep)?;
                policy.build(crypto)
            })
            .transpose()?;
        Ok(RotationPlan {
            old_scripts,
            new_keys,
            new_scripts,
            sweep,
            revocation,
            renewed,
        })
    }

    /// Outpoints of the old epoch this rotation consumes.
    pub fn spent_outpoints(&self) -> BTreeSet<OutPoint> {
        self.sweep_inputs
            .iter()
            .chain(&self.revoke_connectors)
            .map(|input| input.outpoint)
            .collect()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RotationApproval {
    pub version: u8,
    pub request: RotationRequest,
    pub sweep_signatures: Vec<CooperativeSignature>,
    pub revocation_signatures: Vec<Signature>,
    pub policy_signatures: Vec<CooperativeSignature>,
}

/// A renewed policy must spend exactly the funding transaction's single output under the new
/// keys and consume nothing else.
fn check_renewal(
    policy: &PolicyRequest,
    network: Network,
    new_keys: &VaultKeys,
    funding: &Spend,
) -> Result<()> {
    let expected = Input {
        outpoint: OutPoint::new(funding.txid, 0),
        value_sats: funding.amount_sats,
    };
    if policy.network != network
        || policy.keys != *new_keys
        || policy.vault_inputs != [expected]
        || !policy.controller_inputs.is_empty()
    {
        return Err(Error::WrongVault);
    }
    Ok(())
}

/// Reject requests that exceed the protocol's input bound before doing any work.
pub fn check_input_count(count: usize) -> Result<()> {
    if count == 0 || count > MAX_INPUTS {
        return Err(Error::TooManyInputs);
    }
    Ok(())
}

/// Checks that keep a later request from running alongside an earlier approval.
pub fn check_linkage(
    anchor: Option<&EpochAnchor>,
    spent: &BTreeSet<OutPoint>,
    previous_allowances_complete: bool,
) -> Result<policy::Linkage> {
    match anchor {
        None => Ok(policy::Linkage::First),
        Some(anchor) => anchor.linkage(spent, previous_allowances_complete),
    }
}

/// Limits carried unchanged by a rotation.
pub fn same_limits(renewed: Option<&PolicyRequest>, previous: Limits) -> bool {
    match renewed {
        Some(policy) => policy.limits == previous,
        None => previous.monthly_limit_sats == 0 && previous.emergency_access_limit_sats == 0,
    }
}
