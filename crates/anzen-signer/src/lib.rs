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
        self, OfflineAction, OfflineActionApproval, OfflineActionRequest, PolicyApproval,
        PolicyRequest, RotationApproval, RotationRequest, Signature, SweepApproval, SweepRequest,
    },
};
use serde::{Deserialize, Serialize};

/// The signer's private key, held by the device.
pub trait Keys {
    /// The x-only public key this signer contributes to the vault.
    fn vault_key(&mut self) -> Result<XOnlyKey>;
    /// A BIP340 signature over a 32-byte message.
    fn sign(&mut self, message: &[u8; 32]) -> Result<[u8; 64]>;
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

pub const STATE_VERSION: u8 = 1;

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

impl<'a, C: Crypto + ?Sized, K: Keys + ?Sized> Signer<'a, C, K> {
    pub fn new(crypto: &'a mut C, keys: &'a mut K) -> Self {
        Self { crypto, keys }
    }

    /// Accept a vault with the phone key. Pairing never approves a spending policy.
    pub fn pair(&mut self, network: Network, phone: XOnlyKey) -> Result<State> {
        let keys = VaultKeys {
            phone,
            hww: self.keys.vault_key()?,
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
        self.check_state(state)?;
        let (_, graph, _) = self.policy(state, request, now)?;
        let signatures = self.sign_policy(&graph)?;
        let mut next = state.clone();
        next.limits = request.limits;
        next.anchor = Some(graph.anchor());
        next.anchor_approved_at = now;
        Ok((
            PolicyApproval {
                version: wire::VERSION,
                request: request.clone(),
                signatures,
            },
            next,
        ))
    }

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
        self.check_state(state)?;
        let (scripts, spend) = self.sweep(state, request)?;
        Ok(SweepApproval {
            version: wire::VERSION,
            request: request.clone(),
            signatures: self.sign_spend(&spend, &scripts.vault.cooperative_leaf)?,
        })
    }

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
        self.check_state(state)?;
        let (_, spend, leaf) = self.offline_action(state, request)?;
        Ok(OfflineActionApproval {
            version: wire::VERSION,
            request: request.clone(),
            signatures: self.sign_spend(&spend, &leaf)?,
        })
    }

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
        self.check_state(state)?;
        let (plan, _) = self.rotation(state, request, now)?;
        let sweep_signatures =
            self.sign_spend(&plan.sweep, &plan.old_scripts.vault.cooperative_leaf)?;
        let revocation_signatures = match &plan.revocation {
            Some(spend) => self.sign_spend(spend, &plan.old_scripts.controller.hww_leaf)?,
            None => Vec::new(),
        };
        let (policy_signatures, anchor) = match &plan.renewed {
            Some((_, graph)) => (self.sign_policy(graph)?, Some(graph.anchor())),
            None => (Vec::new(), None),
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
            RotationApproval {
                version: wire::VERSION,
                request: request.clone(),
                sweep_signatures,
                revocation_signatures,
                policy_signatures,
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
        Ok((scripts, graph, linkage))
    }

    fn sweep(&mut self, state: &State, request: &SweepRequest) -> Result<(Scripts, Spend)> {
        Self::check_vault(state, request.network, &request.keys)?;
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
        Ok((request.build(self.crypto)?, linkage))
    }

    fn sign_policy(&mut self, graph: &PolicyGraph) -> Result<Vec<Signature>> {
        graph
            .cooperative_signing_jobs(self.crypto)?
            .into_iter()
            .map(|job| self.keys.sign(&job.sighash).map(Signature))
            .collect()
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
