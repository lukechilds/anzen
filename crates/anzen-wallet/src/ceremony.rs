//! The phone's view of an annual policy: proposal requests, approval verification and the
//! encrypted schedule it keeps afterwards.
//!
//! Transactions come only from `anzen_protocol::policy`. The phone proposes parameters, the
//! signer returns signatures, and the phone rebuilds the same graph, adds its own signatures and
//! verifies both before storing anything.

use crate::transactions::{
    attach_signatures, protocol_psbt, sign_controller_psbt_inputs, sign_vault_psbt_inputs,
};
use anyhow::{Context, Result, bail};
use anzen_common::{
    crypto::EncryptedBlob,
    keys::DeviceKeys,
    policy::{ControllerPath, ControllerPolicy, SpendPath, VaultPolicy},
    protocol::{self, SoftwareCrypto},
    storage::VaultConfig,
    types::VaultUtxo,
};
pub use anzen_protocol::policy::Limits as PolicyLimits;
use anzen_protocol::{
    policy::{self, PolicyGraph, PolicyInputs},
    script::Scripts,
    wire::{self, PolicyApproval, PolicyRequest, Signature},
};
use bitcoin::{
    Address, OutPoint, Psbt, hashes::Hash, secp256k1::XOnlyPublicKey, taproot::TapLeafHash,
};
use miniscript::psbt::PsbtExt;
use serde::{Deserialize, Serialize};
use std::str::FromStr;

pub const SCHEDULE_FILE: &str = "phone/schedule.json";

/// Supplies fresh phone receive addresses without coupling the protocol rules to a wallet SDK.
pub trait HotAddressProvider {
    fn next_receive_address(&mut self) -> Result<Address>;
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConnectorState {
    pub outpoint: OutPoint,
    pub value_sats: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EncryptedTransaction {
    pub version: u8,
    pub step: u8,
    pub kind: TransactionKind,
    pub txid: String,
    pub encrypted_psbt: EncryptedBlob,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum EmergencyTransactionKind {
    Trigger,
    Withdrawal,
    Cancellation,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EncryptedEmergencyTransaction {
    pub version: u8,
    pub kind: EmergencyTransactionKind,
    pub txid: String,
    pub encrypted_psbt: EncryptedBlob,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum TransactionKind {
    Authorization,
    Revocation,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScheduleEntry {
    pub step: u8,
    pub hot_address: String,
    pub authorization_file: String,
    pub authorization_txid: String,
    pub connector: ConnectorState,
    pub next_connector: Option<ConnectorState>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EmergencyAccessSchedule {
    pub amount_sats: u64,
    pub delay_seconds: u32,
    pub hot_address: String,
    pub trigger_file: String,
    pub trigger_txid: String,
    pub withdrawal_file: String,
    pub withdrawal_txid: String,
    pub trigger_connector: ConnectorState,
    pub withdrawal_connector: ConnectorState,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Schedule {
    pub version: u8,
    pub rollover_txid: String,
    pub controller_descriptor: String,
    pub controller_address: String,
    pub connector_value_sats: u64,
    pub monthly_limit_sats: u64,
    pub monthly_delay_seconds: u32,
    #[serde(default)]
    pub emergency_access_limit_sats: u64,
    pub entries: Vec<ScheduleEntry>,
    #[serde(default)]
    pub emergency_access: Option<EmergencyAccessSchedule>,
}

/// Build the parameters the signer reviews. Hot-wallet addresses are revealed only after the
/// protocol has decided how many allowance steps the vault can fund.
pub fn propose(
    config: &VaultConfig,
    vault_utxos: &[VaultUtxo],
    controller_utxos: &[VaultUtxo],
    limits: PolicyLimits,
    hot: &mut impl HotAddressProvider,
    previous_allowances_complete: bool,
) -> Result<PolicyRequest> {
    let keys = protocol::vault_keys(config)?;
    let mut crypto = SoftwareCrypto::new();
    let scripts = protocol::protocol(Scripts::new(&mut crypto, &keys))?;
    if vault_utxos
        .iter()
        .any(|utxo| utxo.txout.script_pubkey.as_bytes() != scripts.vault.script_pubkey)
    {
        bail!("policy proposal contains an input outside the configured vault script");
    }
    if controller_utxos
        .iter()
        .any(|utxo| utxo.txout.script_pubkey.as_bytes() != scripts.controller.script_pubkey)
    {
        bail!("policy proposal contains an input outside the fixed controller script");
    }
    let vault_inputs = protocol::inputs(vault_utxos);
    let controller_inputs = protocol::inputs(controller_utxos);
    let plan = protocol::protocol(policy::plan(&PolicyInputs {
        scripts: &scripts,
        vault: &vault_inputs,
        controllers: &controller_inputs,
        limits,
    }))?;
    let emergency_destination = (limits.emergency_access_limit_sats > 0)
        .then(|| {
            hot.next_receive_address()
                .map(|address| address.to_string())
        })
        .transpose()?;
    let allowance_destinations = (0..plan.allowance_count)
        .map(|_| {
            hot.next_receive_address()
                .map(|address| address.to_string())
        })
        .collect::<Result<Vec<_>>>()?;
    let request = PolicyRequest {
        version: wire::VERSION,
        network: protocol::config_network(config)?,
        keys,
        vault_inputs,
        controller_inputs,
        limits,
        emergency_destination,
        allowance_destinations,
        previous_allowances_complete,
    };
    protocol::protocol(request.build(&mut crypto))?;
    Ok(request)
}

/// Rebuild a request's graph for display.
pub fn graph(request: &PolicyRequest) -> Result<(Scripts, PolicyGraph)> {
    protocol::protocol(request.build(&mut SoftwareCrypto::new()))
}

/// A policy approval rebuilt by the phone, carrying both devices' verified signatures.
pub struct ApprovedPolicy {
    pub approval: PolicyApproval,
    pub graph: PolicyGraph,
    /// One PSBT per graph transaction in [`PolicyGraph::transactions`] order. Vault inputs are
    /// fully signed; only the rollover's connector inputs are signed, by the phone.
    pub psbts: Vec<Psbt>,
}

impl ApprovedPolicy {
    /// Rebuild the approved graph, verify every signer signature, add the phone's signatures
    /// and check that every vault input finalizes. Nothing is written or broadcast.
    pub fn verify(
        config: &VaultConfig,
        approval: &PolicyApproval,
        phone: &DeviceKeys,
    ) -> Result<Self> {
        config.validate_public_policy()?;
        let request = &approval.request;
        if approval.version != wire::VERSION {
            bail!("unsupported policy approval");
        }
        if request.network != protocol::config_network(config)?
            || request.keys != protocol::vault_keys(config)?
        {
            bail!("approved policy belongs to another vault");
        }
        if phone.vault_pubkey.to_string() != config.phone_vault_pubkey {
            bail!("phone key does not match the configured vault policy");
        }
        let mut crypto = SoftwareCrypto::new();
        let (scripts, graph) = protocol::protocol(request.build(&mut crypto))?;
        let jobs = protocol::protocol(graph.cooperative_signing_jobs(&mut crypto))?;
        if jobs.len() != approval.signatures.len() {
            bail!("policy approval does not sign every presigned transaction");
        }
        let network = config.bitcoin_network()?;
        let vault = VaultPolicy::from_descriptor_for_network(&config.vault_descriptor, network)?;
        let hww = XOnlyPublicKey::from_str(&config.hww_vault_pubkey)?;
        let controller = ControllerPolicy::new_for_network(phone.vault_pubkey, hww, network)?;
        let leaf = TapLeafHash::from_byte_array(scripts.vault.cooperative_leaf);
        if vault.leaf(SpendPath::Cooperative)?.leaf_hash != leaf {
            bail!("protocol and descriptor disagree on the cooperative leaf");
        }
        let secp = bitcoin::key::Secp256k1::verification_only();
        let mut psbts = Vec::new();
        for (index, transaction) in graph.transactions().into_iter().enumerate() {
            let mut psbt = protocol_psbt(
                &transaction.transaction,
                &transaction.prevouts,
                &transaction.spends,
                &vault,
                &controller,
            )?;
            let vault_inputs = transaction.vault_input_indexes().collect::<Vec<_>>();
            let (signed_inputs, signatures): (Vec<usize>, Vec<Signature>) = jobs
                .iter()
                .zip(&approval.signatures)
                .filter(|(job, _)| job.transaction == index)
                .map(|(job, signature)| (job.input, *signature))
                .unzip();
            attach_signatures(&mut psbt, &signed_inputs, hww, leaf, &signatures)
                .context("signer approval contains an invalid signature")?;
            sign_vault_psbt_inputs(
                &mut psbt,
                &vault,
                SpendPath::Cooperative,
                &phone.vault_keypair,
                &vault_inputs,
            )?;
            let mut finalize = vault_inputs.clone();
            if index == 0 {
                // The rollover consumes the previous epoch's connectors immediately.
                let controllers = transaction.controller_input_indexes().collect::<Vec<_>>();
                sign_controller_psbt_inputs(
                    &mut psbt,
                    &controller,
                    ControllerPath::Phone,
                    &phone.vault_keypair,
                    &controllers,
                )?;
                finalize.extend(controllers);
            }
            let mut trial = psbt.clone();
            for input in finalize {
                trial.finalize_inp_mut(&secp, input).map_err(|error| {
                    anyhow::anyhow!("policy input {input} cannot finalize: {error:?}")
                })?;
            }
            psbts.push(psbt);
        }
        Ok(Self {
            approval: approval.clone(),
            graph,
            psbts,
        })
    }

    pub fn rollover(&self) -> &Psbt {
        &self.psbts[0]
    }

    /// Authorization PSBT for each allowance step, in step order.
    pub fn authorizations(&self) -> &[Psbt] {
        &self.psbts[1..1 + self.graph.allowances.len()]
    }

    /// Emergency trigger and withdrawal PSBTs.
    pub fn emergency(&self) -> Option<(&Psbt, &Psbt)> {
        let start = 1 + self.graph.allowances.len();
        self.graph
            .emergency
            .as_ref()
            .map(|_| (&self.psbts[start], &self.psbts[start + 1]))
    }

    pub fn connector(outpoint: anzen_protocol::tx::OutPoint) -> ConnectorState {
        ConnectorState {
            outpoint: protocol::bitcoin_outpoint(outpoint),
            value_sats: anzen_protocol::consts::CONNECTOR_VALUE_SATS,
        }
    }
}

/// The monthly chain outpoints of a schedule. A proposal that spends none of them while the
/// schedule has allowances reports the chain finished to the signer.
pub fn monthly_chain_outpoints(schedule: &Schedule) -> Result<Vec<OutPoint>> {
    let mut outpoints = Vec::new();
    if let Some(first) = schedule.entries.first() {
        outpoints.push(OutPoint::new(schedule.rollover_txid.parse()?, 0));
        outpoints.push(first.connector.outpoint);
    }
    for entry in &schedule.entries {
        if let Some(next) = &entry.next_connector {
            outpoints.push(OutPoint::new(entry.authorization_txid.parse()?, 1));
            outpoints.push(next.outpoint);
        }
    }
    Ok(outpoints)
}
