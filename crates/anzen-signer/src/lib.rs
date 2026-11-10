#![no_std]

//! The Anzen cold signer.
//!
//! This crate decides everything the hardware wallet signs. Each request carries parameters,
//! never transactions: the signer rebuilds the transactions with [`anzen_protocol`], shows a
//! review of what they do, and returns signatures only after the same request is approved.
//! It has no filesystem, network, clock or randomness of its own. The host supplies keys through
//! [`Keys`], persists [`State`], and may pass the current time where it has a trusted clock.
//!
//! The same code runs in the Ledger app, the mobile hardware-wallet mode, the browser signer and
//! the simulated CLI signer.

extern crate alloc;

pub mod benchmark;
pub mod hardware;
pub mod musig;

/// Name presented by hardware-wallet implementations of the protocol.
pub const PROTOCOL_NAME: &str = "Anzen";

use alloc::{string::String, vec::Vec};
use anzen_protocol::{
    Error, Result,
    address::{self, Network},
    consts::{
        EMERGENCY_ACCESS_DELAY_SECONDS, HWW_RECOVERY_SECONDS, MONTHLY_ALLOWANCE_DELAY_SECONDS,
    },
    crypto::Crypto,
    policy::{EpochAnchor, Limits, Linkage, PolicyGraph},
    script::{Scripts, VaultKeys, XOnlyKey},
    spend::Spend,
    wire::{
        self, CooperativeSignature, OfflineAction, OfflineActionApproval, OfflineActionRequest,
        PolicyApproval, PolicyRequest, PubNonce, RotationApproval, RotationRequest, Signature,
        SweepApproval, SweepRequest,
    },
};
use serde::{Deserialize, Serialize};

/// One joint vault signature, which the signer completes as the last MuSig2 signer.
pub struct CooperativeSession<'a> {
    /// The vault's keys. BIP327 aggregates their even-y points in sorted order
    /// ([`anzen_protocol::musig`]).
    pub keys: &'a VaultKeys,
    /// The root of the vault's script tree, committed to by the Taproot tweak.
    pub merkle_root: &'a [u8; 32],
    /// The BIP341 key-path message.
    pub message: &'a [u8; 32],
    /// The phone's public nonce for this signature.
    pub phone_nonce: &'a PubNonce,
}

/// The signer's private key, held by the device.
pub trait Keys {
    /// The x-only public key this signer contributes to the vault.
    fn vault_key(&mut self) -> Result<XOnlyKey>;
    /// A BIP340 signature over a 32-byte message, for the script paths the signer uses alone.
    fn sign(&mut self, message: &[u8; 32]) -> Result<[u8; 64]>;
    /// The signer's half of a joint MuSig2 signature: its public nonce and partial signature.
    ///
    /// Nonce handling: reusing a MuSig2 secret nonce for two different signing sessions reveals
    /// the private key. The signer avoids keeping nonces at all by signing last, with BIP327
    /// `DeterministicSign`: it derives its nonce from its secret key, the phone's nonce, the
    /// tweaked aggregate key and the message, signs, and forgets it. The same request yields the
    /// same nonce and the same answer; any change to those inputs, including the phone's fresh
    /// nonce on a retry, yields a different nonce. Implementations must not cache, store or
    /// derive the nonce any other way, and must never sign as the first signer.
    fn sign_cooperative(
        &mut self,
        session: &CooperativeSession<'_>,
    ) -> Result<CooperativeSignature>;
}

/// Everything the signer remembers between requests.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct State {
    pub version: u8,
    pub network: Network,
    pub keys: VaultKeys,
    /// Limits of the last approved policy; rotations must preserve them.
    pub limits: Limits,
    /// Live-state outpoints of the last approved policy graph for these keys.
    pub anchor: Option<EpochAnchor>,
    /// When that policy was approved, if the host supplied a trusted time.
    pub anchor_approved_at: Option<u64>,
}

pub const STATE_VERSION: u8 = 2;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PolicyReview {
    pub network: Network,
    pub vault_address: String,
    pub vault_input_sats: u64,
    pub controller_input_sats: u64,
    pub monthly_limit_sats: u64,
    pub funded_withdrawals: usize,
    pub emergency_access_limit_sats: u64,
    pub emergency_delay_seconds: Option<u32>,
    pub rollover_fee_sats: u64,
    pub transaction_count: usize,
    pub allowance_destinations: Vec<String>,
    pub emergency_destination: Option<String>,
    pub previous_policy: Linkage,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SweepReview {
    pub network: Network,
    pub vault_address: String,
    pub destination: String,
    pub amount_sats: u64,
    pub fee_sats: u64,
    pub input_count: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct OfflineActionReview {
    pub network: Network,
    pub action: OfflineAction,
    pub vault_address: String,
    pub destination: String,
    pub amount_sats: u64,
    pub fee_sats: u64,
    pub input_count: usize,
    /// Required by script; the signer does not know the chain height.
    pub recovery_delay_seconds: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RotationReview {
    pub network: Network,
    pub old_vault_address: String,
    pub new_vault_address: String,
    pub new_phone_public_key: String,
    pub amount_sats: u64,
    pub sweep_fee_sats: u64,
    pub monthly_limit_sats: u64,
    pub emergency_access_limit_sats: u64,
    pub funded_withdrawals: usize,
    pub revokes_connectors: bool,
    pub previous_policy: Linkage,
}

/// Reviews and signs requests with host-supplied primitives.
pub struct Signer<'a, C: Crypto + ?Sized, K: Keys + ?Sized> {
    crypto: &'a mut C,
    keys: &'a mut K,
}

// Hardware signer memory: three things in this impl exist only for devices with a few kilobytes
// of stack and heap (today the Ledger app, `ledger-app/`), and do nothing for phones:
//   - `#[inline(never)]` on the entry points keeps each one's locals in its own stack frame;
//   - the `sign_*_request` methods return signatures without a copy of the request;
//   - `sign_policy` consumes the graph, freeing each transaction once it is signed.
// A signer that streams requests instead of holding them would need none of them. Search for
// "Hardware signer memory:" to find every such accommodation in the shared crates.
impl<'a, C: Crypto + ?Sized, K: Keys + ?Sized> Signer<'a, C, K> {
    pub fn new(crypto: &'a mut C, keys: &'a mut K) -> Self {
        Self { crypto, keys }
    }

    /// Accept a vault with the phone key and the recovery locktime the phone chose. Pairing never
    /// approves a spending policy.
    #[inline(never)] // Hardware signer memory: see the note on this impl.
    pub fn pair(
        &mut self,
        network: Network,
        phone: XOnlyKey,
        recovery_locktime: u32,
    ) -> Result<State> {
        let keys = VaultKeys {
            phone,
            hww: self.keys.vault_key()?,
            recovery_locktime,
        };
        Scripts::new(self.crypto, &keys)?;
        Ok(State {
            version: STATE_VERSION,
            network,
            keys,
            limits: Limits {
                monthly_limit_sats: 0,
                emergency_access_limit_sats: 0,
            },
            anchor: None,
            anchor_approved_at: None,
        })
    }

    #[inline(never)] // Hardware signer memory: see the note on this impl.
    pub fn review_policy(
        &mut self,
        state: &State,
        request: &PolicyRequest,
        now: Option<u64>,
    ) -> Result<PolicyReview> {
        self.check_state(state)?;
        let (scripts, graph, linkage) = self.policy(state, request, now)?;
        let transaction_count = graph.transactions().len();
        Ok(PolicyReview {
            network: request.network,
            vault_address: self.address(&scripts.vault.script_pubkey, request.network)?,
            vault_input_sats: graph.plan.vault_input_sats,
            controller_input_sats: graph.plan.controller_input_sats,
            monthly_limit_sats: request.limits.monthly_limit_sats,
            funded_withdrawals: graph.allowances.len(),
            emergency_access_limit_sats: request.limits.emergency_access_limit_sats,
            emergency_delay_seconds: graph
                .emergency
                .as_ref()
                .map(|_| EMERGENCY_ACCESS_DELAY_SECONDS),
            rollover_fee_sats: graph.rollover.fee_sats,
            transaction_count,
            allowance_destinations: request.allowance_destinations.clone(),
            emergency_destination: request.emergency_destination.clone(),
            previous_policy: linkage,
        })
    }

    /// Sign every cooperative vault input of the policy graph and remember it as the new anchor.
    pub fn approve_policy(
        &mut self,
        state: &State,
        request: &PolicyRequest,
        now: Option<u64>,
    ) -> Result<(PolicyApproval, State)> {
        let (signatures, next) = self.sign_policy_request(state, request, now)?;
        Ok((
            PolicyApproval {
                version: wire::VERSION,
                request: request.clone(),
                signatures,
            },
            next,
        ))
    }

    /// [`Self::approve_policy`] without the copy of the request. Hardware signer memory: see the
    /// note on this impl.
    #[inline(never)] // Hardware signer memory: see the note on this impl.
    pub fn sign_policy_request(
        &mut self,
        state: &State,
        request: &PolicyRequest,
        now: Option<u64>,
    ) -> Result<(Vec<CooperativeSignature>, State)> {
        self.check_state(state)?;
        let (scripts, graph, _) = self.policy(state, request, now)?;
        let anchor = graph.anchor();
        let signatures = self.sign_policy(&scripts, graph, &request.phone_nonces)?;
        Ok((
            signatures,
            State {
                version: state.version,
                network: state.network,
                keys: state.keys,
                limits: request.limits,
                anchor: Some(anchor),
                anchor_approved_at: now,
            },
        ))
    }

    #[inline(never)] // Hardware signer memory: see the note on this impl.
    pub fn review_sweep(&mut self, state: &State, request: &SweepRequest) -> Result<SweepReview> {
        self.check_state(state)?;
        let (scripts, spend) = self.sweep(state, request)?;
        Ok(SweepReview {
            network: request.network,
            vault_address: self.address(&scripts.vault.script_pubkey, request.network)?,
            destination: request.destination.clone(),
            amount_sats: spend.amount_sats,
            fee_sats: spend.fee_sats,
            input_count: request.inputs.len(),
        })
    }

    pub fn approve_sweep(
        &mut self,
        state: &State,
        request: &SweepRequest,
    ) -> Result<SweepApproval> {
        Ok(SweepApproval {
            version: wire::VERSION,
            signatures: self.sign_sweep_request(state, request)?,
            request: request.clone(),
        })
    }

    /// [`Self::approve_sweep`] without the copy of the request. Hardware signer memory: see the
    /// note on this impl.
    #[inline(never)] // Hardware signer memory: see the note on this impl.
    pub fn sign_sweep_request(
        &mut self,
        state: &State,
        request: &SweepRequest,
    ) -> Result<Vec<CooperativeSignature>> {
        self.check_state(state)?;
        let (scripts, spend) = self.sweep(state, request)?;
        self.sign_joint_spend(&scripts, &spend, &request.phone_nonces)
    }

    #[inline(never)] // Hardware signer memory: see the note on this impl.
    pub fn review_offline_action(
        &mut self,
        state: &State,
        request: &OfflineActionRequest,
    ) -> Result<OfflineActionReview> {
        self.check_state(state)?;
        let (scripts, spend, _) = self.offline_action(state, request)?;
        Ok(OfflineActionReview {
            network: request.network,
            action: request.action,
            vault_address: self.address(&scripts.vault.script_pubkey, request.network)?,
            destination: request.destination.clone(),
            amount_sats: spend.amount_sats,
            fee_sats: spend.fee_sats,
            input_count: request.inputs.len(),
            recovery_delay_seconds: (request.action == OfflineAction::HwwRecovery)
                .then_some(HWW_RECOVERY_SECONDS),
        })
    }

    pub fn approve_offline_action(
        &mut self,
        state: &State,
        request: &OfflineActionRequest,
    ) -> Result<OfflineActionApproval> {
        Ok(OfflineActionApproval {
            version: wire::VERSION,
            signatures: self.sign_offline_action_request(state, request)?,
            request: request.clone(),
        })
    }

    /// [`Self::approve_offline_action`] without the copy of the request. Hardware signer memory:
    /// see the note on this impl.
    #[inline(never)] // Hardware signer memory: see the note on this impl.
    pub fn sign_offline_action_request(
        &mut self,
        state: &State,
        request: &OfflineActionRequest,
    ) -> Result<Vec<Signature>> {
        self.check_state(state)?;
        let (_, spend, leaf) = self.offline_action(state, request)?;
        self.sign_spend(&spend, &leaf)
    }

    #[inline(never)] // Hardware signer memory: see the note on this impl.
    pub fn review_rotation(
        &mut self,
        state: &State,
        request: &RotationRequest,
        now: Option<u64>,
    ) -> Result<RotationReview> {
        self.check_state(state)?;
        let (plan, linkage) = self.rotation(state, request, now)?;
        Ok(RotationReview {
            network: request.network,
            old_vault_address: self
                .address(&plan.old_scripts.vault.script_pubkey, request.network)?,
            new_vault_address: self
                .address(&plan.new_scripts.vault.script_pubkey, request.network)?,
            new_phone_public_key: request.new_phone.to_hex(),
            amount_sats: plan.sweep.amount_sats,
            sweep_fee_sats: plan.sweep.fee_sats,
            monthly_limit_sats: state.limits.monthly_limit_sats,
            emergency_access_limit_sats: state.limits.emergency_access_limit_sats,
            funded_withdrawals: plan
                .renewed
                .as_ref()
                .map_or(0, |(_, graph)| graph.allowances.len()),
            revokes_connectors: plan.revocation.is_some(),
            previous_policy: linkage,
        })
    }

    /// Sign the sweep, connector revocation and renewed policy. The returned state belongs to
    /// the new vault; hosts adopt it once the user confirms the rotation was broadcast.
    pub fn approve_rotation(
        &mut self,
        state: &State,
        request: &RotationRequest,
        now: Option<u64>,
    ) -> Result<(RotationApproval, State)> {
        let (signatures, next) = self.sign_rotation_request(state, request, now)?;
        Ok((
            RotationApproval {
                version: wire::VERSION,
                request: request.clone(),
                sweep_signatures: signatures.sweep,
                revocation_signatures: signatures.revocation,
                policy_signatures: signatures.policy,
            },
            next,
        ))
    }

    /// [`Self::approve_rotation`] without the copy of the request. Hardware signer memory: see the
    /// note on this impl.
    #[inline(never)] // Hardware signer memory: see the note on this impl.
    pub fn sign_rotation_request(
        &mut self,
        state: &State,
        request: &RotationRequest,
        now: Option<u64>,
    ) -> Result<(hardware::RotationSignatures, State)> {
        self.check_state(state)?;
        let (plan, _) = self.rotation(state, request, now)?;
        let sweep_signatures =
            self.sign_joint_spend(&plan.old_scripts, &plan.sweep, &request.sweep_nonces)?;
        let revocation_signatures = match &plan.revocation {
            Some(spend) => self.sign_spend(spend, &plan.old_scripts.controller.hww_leaf)?,
            None => Vec::new(),
        };
        let (policy_signatures, anchor) = match (plan.renewed, &request.renewed_policy) {
            (Some((scripts, graph)), Some(renewed)) => {
                let anchor = graph.anchor();
                (
                    self.sign_policy(&scripts, graph, &renewed.phone_nonces)?,
                    Some(anchor),
                )
            }
            _ => (Vec::new(), None),
        };
        let next = State {
            version: STATE_VERSION,
            network: state.network,
            keys: plan.new_keys,
            limits: state.limits,
            anchor,
            anchor_approved_at: now,
        };
        Ok((
            hardware::RotationSignatures {
                sweep: sweep_signatures,
                revocation: revocation_signatures,
                policy: policy_signatures,
            },
            next,
        ))
    }

    fn check_state(&mut self, state: &State) -> Result<()> {
        if state.version != STATE_VERSION {
            return Err(Error::UnsupportedVersion);
        }
        if state.keys.hww != self.keys.vault_key()? {
            return Err(Error::WrongVault);
        }
        Ok(())
    }

    fn check_vault(state: &State, network: Network, keys: &VaultKeys) -> Result<()> {
        if network != state.network || *keys != state.keys {
            return Err(Error::WrongVault);
        }
        Ok(())
    }

    fn policy(
        &mut self,
        state: &State,
        request: &PolicyRequest,
        now: Option<u64>,
    ) -> Result<(Scripts, PolicyGraph, Linkage)> {
        Self::check_vault(state, request.network, &request.keys)?;
        let linkage = linkage(
            state,
            &request.spent_outpoints(),
            request.previous_allowances_complete,
            now,
        )?;
        let (scripts, graph) = request.build(self.crypto)?;
        wire::check_nonces(&request.phone_nonces, graph.cooperative_signature_count())?;
        Ok((scripts, graph, linkage))
    }

    fn sweep(&mut self, state: &State, request: &SweepRequest) -> Result<(Scripts, Spend)> {
        Self::check_vault(state, request.network, &request.keys)?;
        wire::check_nonces(&request.phone_nonces, request.inputs.len())?;
        request.build(self.crypto)
    }

    fn offline_action(
        &mut self,
        state: &State,
        request: &OfflineActionRequest,
    ) -> Result<(Scripts, Spend, [u8; 32])> {
        Self::check_vault(state, request.network, &request.keys)?;
        request.build(self.crypto)
    }

    fn rotation(
        &mut self,
        state: &State,
        request: &RotationRequest,
        now: Option<u64>,
    ) -> Result<(wire::RotationPlan, Linkage)> {
        Self::check_vault(state, request.network, &request.old_keys)?;
        if !wire::same_limits(request.renewed_policy.as_ref(), state.limits) {
            return Err(Error::PolicyLimitsChanged);
        }
        let linkage = linkage(
            state,
            &request.spent_outpoints(),
            request.previous_allowances_complete,
            now,
        )?;
        let plan = request.build(self.crypto)?;
        wire::check_nonces(&request.sweep_nonces, request.sweep_inputs.len())?;
        if let (Some((_, graph)), Some(renewed)) = (&plan.renewed, &request.renewed_policy) {
            wire::check_nonces(&renewed.phone_nonces, graph.cooperative_signature_count())?;
        }
        Ok((plan, linkage))
    }

    /// Sign every joint signature of `graph`, in [`PolicyGraph::cooperative_signing_jobs`]
    /// order. Hardware signer memory: the graph is consumed, each transaction freed once signed,
    /// so a device never holds the whole graph and all its signatures at once.
    fn sign_policy(
        &mut self,
        scripts: &Scripts,
        graph: PolicyGraph,
        nonces: &[PubNonce],
    ) -> Result<Vec<CooperativeSignature>> {
        wire::check_nonces(nonces, graph.cooperative_signature_count())?;
        let mut signatures = Vec::with_capacity(nonces.len());
        let keys = &mut *self.keys;
        let mut nonces = nonces.iter();
        graph.sign_each(self.crypto, |message| {
            let phone_nonce = nonces.next().ok_or(Error::NonceCountMismatch)?;
            signatures.push(keys.sign_cooperative(&CooperativeSession {
                keys: &scripts.vault.keys,
                merkle_root: &scripts.vault.merkle_root,
                message,
                phone_nonce,
            })?);
            Ok(())
        })?;
        Ok(signatures)
    }

    fn sign_joint_spend(
        &mut self,
        scripts: &Scripts,
        spend: &Spend,
        nonces: &[PubNonce],
    ) -> Result<Vec<CooperativeSignature>> {
        let messages = spend.key_path_sighashes(self.crypto)?;
        wire::check_nonces(nonces, messages.len())?;
        messages
            .iter()
            .zip(nonces)
            .map(|(message, nonce)| self.sign_jointly(scripts, message, nonce))
            .collect()
    }

    fn sign_jointly(
        &mut self,
        scripts: &Scripts,
        message: &[u8; 32],
        phone_nonce: &PubNonce,
    ) -> Result<CooperativeSignature> {
        self.keys.sign_cooperative(&CooperativeSession {
            keys: &scripts.vault.keys,
            merkle_root: &scripts.vault.merkle_root,
            message,
            phone_nonce,
        })
    }

    fn sign_spend(&mut self, spend: &Spend, leaf: &[u8; 32]) -> Result<Vec<Signature>> {
        spend
            .sighashes(self.crypto, leaf)?
            .into_iter()
            .map(|sighash| self.keys.sign(&sighash).map(Signature))
            .collect()
    }

    fn address(&mut self, script: &[u8], network: Network) -> Result<String> {
        address::from_script_pubkey(self.crypto, script, network)
    }
}

/// A new epoch must consume the previous approval's live state. A phone may report that every
/// monthly allowance was already used; where the host has a trusted clock, reject reports that
/// the sequential 30-day delays make impossible.
fn linkage(
    state: &State,
    spent: &alloc::collections::BTreeSet<anzen_protocol::tx::OutPoint>,
    previous_allowances_complete: bool,
    now: Option<u64>,
) -> Result<Linkage> {
    let linkage = wire::check_linkage(state.anchor.as_ref(), spent, previous_allowances_complete)?;
    if linkage == Linkage::MonthlyReportedComplete {
        if let (Some(anchor), Some(approved_at), Some(now)) =
            (&state.anchor, state.anchor_approved_at, now)
        {
            let earliest = approved_at.saturating_add(
                u64::from(anchor.monthly_allowances) * u64::from(MONTHLY_ALLOWANCE_DELAY_SECONDS),
            );
            if now < earliest {
                return Err(Error::ImplausibleCompletedAllowances);
            }
        }
    }
    Ok(linkage)
}
