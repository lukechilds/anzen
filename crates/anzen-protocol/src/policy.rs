//! The annual policy graph: one rollover, a sequential monthly allowance chain and an optional
//! cancellable emergency withdrawal.
//!
//! [`plan`] decides every amount from the vault balance and limits alone, so the phone learns how
//! many hot-wallet addresses to reveal before it reveals any. [`build`] then produces the complete
//! graph. The phone and the signer call the same functions with the same inputs, so validation is
//! reconstruction: there is no separate checker that could drift from the builder.

use crate::{
    Error, Result,
    consts::{
        CONNECTOR_VALUE_SATS, EMERGENCY_DELAY_SEQUENCE, MAX_INPUTS, MONTHLY_DELAY_SEQUENCE,
        MONTHS_PER_ROLLOVER, SEQUENCE_FINAL, SEQUENCE_RBF,
    },
    crypto::Crypto,
    script::{P2TR_SCRIPT_LEN, Scripts, is_p2tr},
    sighash::SighashCache,
    tx::{OutPoint, SpendPath, Transaction, TxIn, TxOut, Txid, minimal_non_dust},
};
use alloc::{collections::BTreeSet, vec, vec::Vec};
use serde::{Deserialize, Serialize};

/// An unspent output and its value. The owning script is implied by where it is listed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Input {
    pub outpoint: OutPoint,
    pub value_sats: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Limits {
    pub monthly_limit_sats: u64,
    pub emergency_access_limit_sats: u64,
}

/// Everything that determines a policy graph apart from its hot-wallet destinations.
#[derive(Debug, Clone, Copy)]
pub struct PolicyInputs<'a> {
    pub scripts: &'a Scripts,
    /// Live vault outputs, spent through the cooperative path.
    pub vault: &'a [Input],
    /// Live connectors from a previous epoch, consumed so none of its actions survive.
    pub controllers: &'a [Input],
    pub limits: Limits,
}

/// Amounts chosen by [`plan`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PolicyPlan {
    pub allowance_count: usize,
    pub allowance_value_sats: u64,
    pub rollover_fee_sats: u64,
    pub remainder_value_sats: u64,
    pub vault_input_sats: u64,
    pub controller_input_sats: u64,
    pub total_input_sats: u64,
}

impl PolicyPlan {
    pub fn has_emergency_access(&self, limits: Limits) -> bool {
        limits.emergency_access_limit_sats > 0
    }
}

/// Hot-wallet scriptPubKeys, revealed in this order: emergency first, then each allowance step.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Destinations {
    pub emergency: Option<Vec<u8>>,
    pub allowances: Vec<Vec<u8>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum TransactionKind {
    Rollover,
    Allowance,
    EmergencyTrigger,
    EmergencyWithdrawal,
}

/// One transaction of a graph with everything needed to sign or finalize it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GraphTransaction {
    pub kind: TransactionKind,
    pub transaction: Transaction,
    pub txid: Txid,
    pub prevouts: Vec<TxOut>,
    pub spends: Vec<SpendPath>,
    pub fee_sats: u64,
}

impl GraphTransaction {
    fn new<C: Crypto + ?Sized>(
        crypto: &mut C,
        kind: TransactionKind,
        transaction: Transaction,
        prevouts: Vec<TxOut>,
        spends: Vec<SpendPath>,
    ) -> Result<Self> {
        let input_sats = prevouts.iter().try_fold(0_u64, |sum, prevout| {
            sum.checked_add(prevout.value).ok_or(Error::AmountOverflow)
        })?;
        let fee_sats = input_sats
            .checked_sub(transaction.output_sum()?)
            .ok_or(Error::InsufficientFee)?;
        Ok(Self {
            kind,
            txid: transaction.txid(crypto),
            transaction,
            prevouts,
            spends,
            fee_sats,
        })
    }

    pub fn vault_input_indexes(&self) -> impl Iterator<Item = usize> + '_ {
        self.spends
            .iter()
            .enumerate()
            .filter(|(_, spend)| !spend.is_controller())
            .map(|(index, _)| index)
    }

    pub fn controller_input_indexes(&self) -> impl Iterator<Item = usize> + '_ {
        self.spends
            .iter()
            .enumerate()
            .filter(|(_, spend)| spend.is_controller())
            .map(|(index, _)| index)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AllowanceStep {
    pub step: u8,
    pub chain_value_sats: u64,
    pub hot_script: Vec<u8>,
    pub connector: OutPoint,
    pub next_connector: Option<OutPoint>,
    pub authorization: GraphTransaction,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EmergencyAccess {
    pub amount_sats: u64,
    pub hot_script: Vec<u8>,
    pub staging_vout: u32,
    pub staging_value_sats: u64,
    pub vault_change_vout: u32,
    pub vault_change_value_sats: u64,
    pub trigger_connector: OutPoint,
    pub withdrawal_connector: OutPoint,
    pub trigger: GraphTransaction,
    pub withdrawal: GraphTransaction,
}

/// A complete annual policy.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PolicyGraph {
    pub limits: Limits,
    pub plan: PolicyPlan,
    pub rollover: GraphTransaction,
    pub allowance_vout: Option<u32>,
    pub remainder_vout: u32,
    pub monthly_connector_vout: Option<u32>,
    pub emergency_connector_vout: Option<u32>,
    pub allowances: Vec<AllowanceStep>,
    pub emergency: Option<EmergencyAccess>,
}

/// One vault-input signature the cooperative key path needs from both devices together.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SigningJob {
    pub transaction: usize,
    pub input: usize,
    pub sighash: [u8; 32],
}

impl PolicyGraph {
    /// Rollover, allowance authorizations in step order, then emergency trigger and withdrawal.
    pub fn transactions(&self) -> Vec<&GraphTransaction> {
        let mut transactions = Vec::with_capacity(3 + self.allowances.len());
        transactions.push(&self.rollover);
        transactions.extend(self.allowances.iter().map(|step| &step.authorization));
        if let Some(emergency) = &self.emergency {
            transactions.push(&emergency.trigger);
            transactions.push(&emergency.withdrawal);
        }
        transactions
    }

    /// Every cooperative vault-input signature message, in [`Self::transactions`] order and then
    /// input order. Connector inputs are never part of the annual approval.
    pub fn cooperative_signing_jobs<C: Crypto + ?Sized>(
        &self,
        crypto: &mut C,
    ) -> Result<Vec<SigningJob>> {
        let mut jobs = Vec::new();
        for (index, transaction) in self.transactions().into_iter().enumerate() {
            let cache = SighashCache::new(crypto, &transaction.transaction, &transaction.prevouts)?;
            for input in transaction.vault_input_indexes() {
                jobs.push(SigningJob {
                    transaction: index,
                    input,
                    sighash: cache.key_path(crypto, input),
                });
            }
        }
        Ok(jobs)
    }

    /// The outpoints whose consumption makes this graph's presigned actions impossible.
    pub fn anchor(&self) -> EpochAnchor {
        let rollover_txid = self.rollover.txid;
        let mut monthly = Vec::new();
        if let (Some(chain), Some(connector)) = (self.allowance_vout, self.monthly_connector_vout) {
            monthly.push(OutPoint::new(rollover_txid, chain));
            monthly.push(OutPoint::new(rollover_txid, connector));
            for step in self
                .allowances
                .iter()
                .filter(|step| step.next_connector.is_some())
            {
                monthly.push(OutPoint::new(step.authorization.txid, 1));
                monthly.push(OutPoint::new(step.authorization.txid, 2));
            }
        }
        let mut emergency = Vec::new();
        if let (Some(access), Some(connector)) = (&self.emergency, self.emergency_connector_vout) {
            emergency.push(OutPoint::new(rollover_txid, self.remainder_vout));
            emergency.push(OutPoint::new(rollover_txid, connector));
            for vout in 0..3 {
                emergency.push(OutPoint::new(access.trigger.txid, vout));
            }
        }
        EpochAnchor {
            rollover_inputs: self
                .rollover
                .transaction
                .input
                .iter()
                .map(|input| input.previous_output)
                .collect(),
            monthly_allowances: self.allowances.len() as u8,
            monthly_chain: monthly,
            emergency_chain: emergency,
        }
    }
}

/// What a later rollover or rotation must consume so an earlier approved graph cannot run
/// alongside it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EpochAnchor {
    pub rollover_inputs: Vec<OutPoint>,
    pub monthly_allowances: u8,
    pub monthly_chain: Vec<OutPoint>,
    pub emergency_chain: Vec<OutPoint>,
}

/// How a new epoch-ending transaction relates to the previously approved graph.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Linkage {
    /// No earlier graph was approved for these keys.
    First,
    /// Spends an input of the earlier rollover, so at most one of the two can ever confirm.
    ConflictsWithPrevious,
    /// Consumes a live output of every earlier action chain.
    ConsumesPrevious,
    /// Consumes the earlier emergency chain; the phone reports the monthly chain finished.
    MonthlyReportedComplete,
}

impl EpochAnchor {
    /// `spent` is every outpoint the new transaction set consumes.
    pub fn linkage(
        &self,
        spent: &BTreeSet<OutPoint>,
        monthly_reported_complete: bool,
    ) -> Result<Linkage> {
        if self
            .rollover_inputs
            .iter()
            .any(|outpoint| spent.contains(outpoint))
        {
            return Ok(Linkage::ConflictsWithPrevious);
        }
        let consumes =
            |chain: &[OutPoint]| chain.is_empty() || chain.iter().any(|o| spent.contains(o));
        if !consumes(&self.emergency_chain) {
            return Err(Error::UnlinkedPreviousPolicy);
        }
        if consumes(&self.monthly_chain) {
            return Ok(Linkage::ConsumesPrevious);
        }
        if monthly_reported_complete {
            return Ok(Linkage::MonthlyReportedComplete);
        }
        Err(Error::UnlinkedPreviousPolicy)
    }
}

/// Choose allowance count and all amounts without revealing hot-wallet addresses.
pub fn plan(inputs: &PolicyInputs<'_>) -> Result<PolicyPlan> {
    let Limits {
        monthly_limit_sats,
        emergency_access_limit_sats,
    } = inputs.limits;
    if inputs.vault.is_empty() {
        return Err(Error::NoVaultInputs);
    }
    validate_inputs(inputs)?;
    let vault_script = &inputs.scripts.vault.script_pubkey;
    let vault_dust = minimal_non_dust(vault_script);
    let hot_placeholder = placeholder_p2tr();
    let vault_input_sats = input_sum(inputs.vault)?;
    let controller_input_sats = input_sum(inputs.controllers)?;
    let total_input_sats = vault_input_sats
        .checked_add(controller_input_sats)
        .ok_or(Error::AmountOverflow)?;

    let has_emergency = emergency_access_limit_sats > 0;
    let minimum_remainder_value = if has_emergency {
        let staging = emergency_staging_value(emergency_access_limit_sats, &hot_placeholder)?;
        let trigger_fee =
            emergency_trigger_template(inputs.scripts, OutPoint::null(), staging, vault_dust)
                .fixed_fee(&[SpendPath::Cooperative, SpendPath::ControllerPhone])?;
        staging
            .checked_add(trigger_fee)
            .and_then(|value| value.checked_add(vault_dust))
            .ok_or(Error::AmountOverflow)?
    } else {
        vault_dust
    };

    let (continuing_fee, final_fee) = if monthly_limit_sats == 0 {
        (0, 0)
    } else {
        if monthly_limit_sats < vault_dust {
            return Err(Error::DustMonthlyLimit);
        }
        (
            authorization_fee(inputs.scripts, monthly_limit_sats, &hot_placeholder, true)?,
            authorization_fee(inputs.scripts, monthly_limit_sats, &hot_placeholder, false)?,
        )
    };

    let rollover_spends = rollover_spends(inputs);
    let emergency_connectors = u64::from(has_emergency);
    if monthly_limit_sats > 0 {
        for count in (1..=MONTHS_PER_ROLLOVER).rev() {
            let allowance_value =
                allowance_chain_value(count, monthly_limit_sats, continuing_fee, final_fee)?;
            let fee = rollover_template(inputs, Some(allowance_value), 1, true, has_emergency)
                .fixed_fee(&rollover_spends)?;
            let connectors = CONNECTOR_VALUE_SATS * (1 + emergency_connectors);
            let required = fee
                .checked_add(allowance_value)
                .and_then(|value| value.checked_add(minimum_remainder_value))
                .and_then(|value| value.checked_add(connectors))
                .ok_or(Error::AmountOverflow)?;
            if total_input_sats >= required {
                return Ok(PolicyPlan {
                    allowance_count: count,
                    allowance_value_sats: allowance_value,
                    rollover_fee_sats: fee,
                    remainder_value_sats: total_input_sats - fee - allowance_value - connectors,
                    vault_input_sats,
                    controller_input_sats,
                    total_input_sats,
                });
            }
        }
    }

    let fee =
        rollover_template(inputs, None, 1, false, has_emergency).fixed_fee(&rollover_spends)?;
    let remainder = total_input_sats
        .checked_sub(fee)
        .and_then(|value| value.checked_sub(CONNECTOR_VALUE_SATS * emergency_connectors))
        .ok_or(Error::InsufficientRolloverFee)?;
    if remainder < minimum_remainder_value {
        return Err(if has_emergency {
            Error::InsufficientEmergencyFunds
        } else {
            Error::InsufficientRemainder
        });
    }
    Ok(PolicyPlan {
        allowance_count: 0,
        allowance_value_sats: 0,
        rollover_fee_sats: fee,
        remainder_value_sats: remainder,
        vault_input_sats,
        controller_input_sats,
        total_input_sats,
    })
}

/// Build the complete graph for the destinations revealed after [`plan`].
pub fn build<C: Crypto + ?Sized>(
    crypto: &mut C,
    inputs: &PolicyInputs<'_>,
    destinations: &Destinations,
) -> Result<PolicyGraph> {
    let plan = plan(inputs)?;
    let limits = inputs.limits;
    let has_emergency = limits.emergency_access_limit_sats > 0;
    if destinations.allowances.len() != plan.allowance_count {
        return Err(Error::AllowanceCountMismatch);
    }
    if destinations.emergency.is_some() != has_emergency {
        return Err(Error::EmergencyDestinationMismatch);
    }
    if destinations
        .allowances
        .iter()
        .chain(&destinations.emergency)
        .any(|script| !is_p2tr(script))
    {
        return Err(Error::UnsupportedDestination);
    }
    let scripts = inputs.scripts;
    let vault_script = scripts.vault.script_pubkey.clone();
    let controller_script = scripts.controller.script_pubkey.clone();
    let vault_dust = minimal_non_dust(&vault_script);
    let has_allowances = plan.allowance_count > 0;

    let rollover_tx = rollover_template(
        inputs,
        has_allowances.then_some(plan.allowance_value_sats),
        plan.remainder_value_sats,
        has_allowances,
        has_emergency,
    );
    let mut prevouts = inputs
        .vault
        .iter()
        .map(|input| TxOut {
            value: input.value_sats,
            script_pubkey: vault_script.clone(),
        })
        .collect::<Vec<_>>();
    prevouts.extend(inputs.controllers.iter().map(|input| TxOut {
        value: input.value_sats,
        script_pubkey: controller_script.clone(),
    }));
    let rollover = GraphTransaction::new(
        crypto,
        TransactionKind::Rollover,
        rollover_tx,
        prevouts,
        rollover_spends(inputs),
    )?;
    let rollover_txid = rollover.txid;
    let remainder_vout = u32::from(has_allowances);
    let monthly_connector_vout = has_allowances.then_some(remainder_vout + 1);
    let emergency_connector_vout =
        has_emergency.then_some(remainder_vout + 1 + u32::from(has_allowances));

    let mut allowances = Vec::with_capacity(plan.allowance_count);
    let mut chain = has_allowances.then(|| {
        (
            OutPoint::new(rollover_txid, 0),
            rollover.transaction.output[0].clone(),
            OutPoint::new(rollover_txid, remainder_vout + 1),
        )
    });
    for (index, hot_script) in destinations.allowances.iter().enumerate() {
        let (source, source_txout, connector) =
            chain.take().ok_or(Error::InsufficientAllowanceFunds)?;
        let has_next = index + 1 < plan.allowance_count;
        let fee = authorization_fee(scripts, limits.monthly_limit_sats, hot_script, has_next)?;
        let next_chain_value = source_txout
            .value
            .checked_add(CONNECTOR_VALUE_SATS)
            .and_then(|value| {
                if has_next {
                    value.checked_sub(CONNECTOR_VALUE_SATS)
                } else {
                    Some(value)
                }
            })
            .and_then(|value| value.checked_sub(limits.monthly_limit_sats))
            .and_then(|value| value.checked_sub(fee))
            .ok_or(Error::InsufficientAllowanceFunds)?;
        if has_next && next_chain_value < vault_dust {
            return Err(Error::DustAllowanceChain);
        }
        if !has_next && next_chain_value != 0 {
            return Err(Error::FinalAllowanceNotExhausted);
        }
        let transaction = authorization_template(
            scripts,
            source,
            connector,
            limits.monthly_limit_sats,
            hot_script,
            has_next.then_some(next_chain_value),
        );
        let authorization = GraphTransaction::new(
            crypto,
            TransactionKind::Allowance,
            transaction,
            vec![source_txout.clone(), connector_txout(scripts)],
            vec![SpendPath::Cooperative, SpendPath::ControllerPhone],
        )?;
        let next_connector = has_next.then(|| OutPoint::new(authorization.txid, 2));
        if has_next {
            chain = Some((
                OutPoint::new(authorization.txid, 1),
                authorization.transaction.output[1].clone(),
                OutPoint::new(authorization.txid, 2),
            ));
        }
        allowances.push(AllowanceStep {
            step: (index + 1) as u8,
            chain_value_sats: source_txout.value,
            hot_script: hot_script.clone(),
            connector,
            next_connector,
            authorization,
        });
    }

    let emergency = match &destinations.emergency {
        Some(hot_script) => Some(build_emergency(
            crypto,
            scripts,
            OutPoint::new(rollover_txid, remainder_vout),
            rollover.transaction.output[remainder_vout as usize].clone(),
            OutPoint::new(
                rollover_txid,
                emergency_connector_vout.expect("emergency connector exists with emergency access"),
            ),
            limits.emergency_access_limit_sats,
            hot_script,
        )?),
        None => None,
    };

    Ok(PolicyGraph {
        limits,
        plan,
        allowance_vout: has_allowances.then_some(0),
        remainder_vout,
        monthly_connector_vout,
        emergency_connector_vout,
        allowances,
        emergency,
        rollover,
    })
}

fn build_emergency<C: Crypto + ?Sized>(
    crypto: &mut C,
    scripts: &Scripts,
    source: OutPoint,
    source_txout: TxOut,
    trigger_connector: OutPoint,
    amount_sats: u64,
    hot_script: &[u8],
) -> Result<EmergencyAccess> {
    let vault_dust = minimal_non_dust(&scripts.vault.script_pubkey);
    let staging_value_sats = emergency_staging_value(amount_sats, hot_script)?;
    if staging_value_sats < vault_dust {
        return Err(Error::DustEmergencyStaging);
    }
    let trigger_fee = emergency_trigger_template(scripts, source, staging_value_sats, vault_dust)
        .fixed_fee(&[SpendPath::Cooperative, SpendPath::ControllerPhone])?;
    let vault_change_value_sats = source_txout
        .value
        .checked_sub(staging_value_sats)
        .and_then(|value| value.checked_sub(trigger_fee))
        .ok_or(Error::InsufficientEmergencyFunds)?;
    if vault_change_value_sats < vault_dust {
        return Err(Error::DustEmergencyChange);
    }
    let mut trigger_tx =
        emergency_trigger_template(scripts, source, staging_value_sats, vault_change_value_sats);
    trigger_tx.input[1].previous_output = trigger_connector;
    let trigger = GraphTransaction::new(
        crypto,
        TransactionKind::EmergencyTrigger,
        trigger_tx,
        vec![source_txout, connector_txout(scripts)],
        vec![SpendPath::Cooperative, SpendPath::ControllerPhone],
    )?;
    let withdrawal_connector = OutPoint::new(trigger.txid, 2);
    let withdrawal_tx = Transaction {
        input: vec![
            TxIn {
                previous_output: OutPoint::new(trigger.txid, 0),
                sequence: EMERGENCY_DELAY_SEQUENCE,
            },
            TxIn {
                previous_output: withdrawal_connector,
                sequence: SEQUENCE_RBF,
            },
        ],
        output: vec![TxOut {
            value: amount_sats,
            script_pubkey: hot_script.to_vec(),
        }],
        lock_time: 0,
    };
    let withdrawal = GraphTransaction::new(
        crypto,
        TransactionKind::EmergencyWithdrawal,
        withdrawal_tx,
        vec![
            trigger.transaction.output[0].clone(),
            connector_txout(scripts),
        ],
        vec![SpendPath::Cooperative, SpendPath::ControllerPhone],
    )?;
    Ok(EmergencyAccess {
        amount_sats,
        hot_script: hot_script.to_vec(),
        staging_vout: 0,
        staging_value_sats,
        vault_change_vout: 1,
        vault_change_value_sats,
        trigger_connector,
        withdrawal_connector,
        trigger,
        withdrawal,
    })
}

fn validate_inputs(inputs: &PolicyInputs<'_>) -> Result<()> {
    let count = inputs.vault.len() + inputs.controllers.len();
    if count == 0 || count > MAX_INPUTS {
        return Err(Error::TooManyInputs);
    }
    let mut seen = BTreeSet::new();
    if inputs
        .vault
        .iter()
        .chain(inputs.controllers)
        .any(|input| !seen.insert(input.outpoint))
    {
        return Err(Error::DuplicateInput);
    }
    Ok(())
}

fn input_sum(inputs: &[Input]) -> Result<u64> {
    inputs.iter().try_fold(0_u64, |sum, input| {
        sum.checked_add(input.value_sats)
            .ok_or(Error::AmountOverflow)
    })
}

fn rollover_spends(inputs: &PolicyInputs<'_>) -> Vec<SpendPath> {
    let mut spends = vec![SpendPath::Cooperative; inputs.vault.len()];
    spends.extend(core::iter::repeat_n(
        SpendPath::ControllerPhone,
        inputs.controllers.len(),
    ));
    spends
}

fn rollover_template(
    inputs: &PolicyInputs<'_>,
    allowance_value: Option<u64>,
    remainder_value: u64,
    monthly_connector: bool,
    emergency_connector: bool,
) -> Transaction {
    let vault_script = &inputs.scripts.vault.script_pubkey;
    let controller_script = &inputs.scripts.controller.script_pubkey;
    let mut input = inputs
        .vault
        .iter()
        .map(|vault| TxIn {
            previous_output: vault.outpoint,
            sequence: SEQUENCE_FINAL,
        })
        .collect::<Vec<_>>();
    input.extend(inputs.controllers.iter().map(|controller| TxIn {
        previous_output: controller.outpoint,
        sequence: SEQUENCE_RBF,
    }));
    let mut output = Vec::with_capacity(4);
    if let Some(value) = allowance_value {
        output.push(TxOut {
            value,
            script_pubkey: vault_script.clone(),
        });
    }
    output.push(TxOut {
        value: remainder_value,
        script_pubkey: vault_script.clone(),
    });
    for _ in 0..usize::from(monthly_connector) + usize::from(emergency_connector) {
        output.push(TxOut {
            value: CONNECTOR_VALUE_SATS,
            script_pubkey: controller_script.clone(),
        });
    }
    Transaction {
        input,
        output,
        lock_time: 0,
    }
}

fn authorization_template(
    scripts: &Scripts,
    source: OutPoint,
    connector: OutPoint,
    monthly_limit_sats: u64,
    hot_script: &[u8],
    next_chain_value: Option<u64>,
) -> Transaction {
    let mut output = vec![TxOut {
        value: monthly_limit_sats,
        script_pubkey: hot_script.to_vec(),
    }];
    if let Some(value) = next_chain_value {
        output.push(TxOut {
            value,
            script_pubkey: scripts.vault.script_pubkey.clone(),
        });
        output.push(connector_txout(scripts));
    }
    Transaction {
        input: vec![
            TxIn {
                previous_output: source,
                sequence: MONTHLY_DELAY_SEQUENCE,
            },
            TxIn {
                previous_output: connector,
                sequence: SEQUENCE_RBF,
            },
        ],
        output,
        lock_time: 0,
    }
}

fn authorization_fee(
    scripts: &Scripts,
    monthly_limit_sats: u64,
    hot_script: &[u8],
    has_next: bool,
) -> Result<u64> {
    authorization_template(
        scripts,
        OutPoint::null(),
        OutPoint::null(),
        monthly_limit_sats,
        hot_script,
        has_next.then_some(1),
    )
    .fixed_fee(&[SpendPath::Cooperative, SpendPath::ControllerPhone])
}

fn emergency_trigger_template(
    scripts: &Scripts,
    source: OutPoint,
    staging_value_sats: u64,
    vault_change_value_sats: u64,
) -> Transaction {
    Transaction {
        input: vec![
            TxIn {
                previous_output: source,
                sequence: SEQUENCE_FINAL,
            },
            TxIn {
                previous_output: OutPoint::null(),
                sequence: SEQUENCE_RBF,
            },
        ],
        output: vec![
            TxOut {
                value: staging_value_sats,
                script_pubkey: scripts.vault.script_pubkey.clone(),
            },
            TxOut {
                value: vault_change_value_sats,
                script_pubkey: scripts.vault.script_pubkey.clone(),
            },
            connector_txout(scripts),
        ],
        lock_time: 0,
    }
}

/// The staged amount after the withdrawal pays its fee and absorbs the connector value.
fn emergency_staging_value(amount_sats: u64, hot_script: &[u8]) -> Result<u64> {
    if amount_sats < minimal_non_dust(hot_script) {
        return Err(Error::DustEmergencyAmount);
    }
    let withdrawal_fee = Transaction {
        input: vec![
            TxIn {
                previous_output: OutPoint::null(),
                sequence: EMERGENCY_DELAY_SEQUENCE,
            },
            TxIn {
                previous_output: OutPoint::null(),
                sequence: SEQUENCE_RBF,
            },
        ],
        output: vec![TxOut {
            value: amount_sats,
            script_pubkey: hot_script.to_vec(),
        }],
        lock_time: 0,
    }
    .fixed_fee(&[SpendPath::Cooperative, SpendPath::ControllerPhone])?;
    amount_sats
        .checked_add(withdrawal_fee)
        .and_then(|value| value.checked_sub(CONNECTOR_VALUE_SATS))
        .ok_or(Error::AmountOverflow)
}

fn allowance_chain_value(
    count: usize,
    monthly_limit_sats: u64,
    continuing_fee: u64,
    final_fee: u64,
) -> Result<u64> {
    if count == 0 {
        return Ok(0);
    }
    let allowances = monthly_limit_sats
        .checked_mul(count as u64)
        .ok_or(Error::AmountOverflow)?;
    let continuing = continuing_fee
        .checked_mul((count - 1) as u64)
        .ok_or(Error::AmountOverflow)?;
    allowances
        .checked_add(continuing)
        .and_then(|value| value.checked_add(final_fee))
        .and_then(|value| value.checked_sub(CONNECTOR_VALUE_SATS))
        .ok_or(Error::AmountOverflow)
}

fn connector_txout(scripts: &Scripts) -> TxOut {
    TxOut {
        value: CONNECTOR_VALUE_SATS,
        script_pubkey: scripts.controller.script_pubkey.clone(),
    }
}

/// A P2TR-sized script used only to size templates before destinations are revealed.
fn placeholder_p2tr() -> Vec<u8> {
    let mut script = vec![0_u8; P2TR_SCRIPT_LEN];
    script[0] = 0x51;
    script[1] = 32;
    script
}
