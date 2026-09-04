use super::{
    CONNECTOR_VALUE_SATS, DEFAULT_FEE_RATE_SAT_VB, EMERGENCY_ACCESS_DELAY_SECONDS,
    MONTHLY_ALLOWANCE_DELAY_SECONDS, MONTHS_PER_ROLLOVER,
    crypto::EncryptedBlob,
    keys::DeviceKeys,
    policy::{ControllerPath, ControllerPolicy, SpendPath, VaultPolicy},
    storage::{VaultConfig, read_json, write_json, write_private},
    transactions::{
        create_policy_psbt, estimate_policy_vsize, sign_controller_psbt_inputs,
        sign_vault_psbt_inputs, validate_default_sighashes,
    },
    types::VaultUtxo,
};
use anyhow::{Context, Result, bail};
use bitcoin::{
    Address, Amount, OutPoint, Psbt, ScriptBuf, Sequence, Transaction, TxIn, TxOut, Witness,
    absolute, transaction::Version,
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Component, Path, PathBuf},
    str::FromStr,
};

pub const DEFAULT_BATCH_DIR: &str = "ceremony/active";
pub const SCHEDULE_FILE: &str = "phone/schedule.json";
pub const POLICY_PACKAGE_KIND: &str = "vault-policy";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PolicyLimits {
    pub monthly_limit_sats: u64,
    pub emergency_access_limit_sats: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BatchTransaction {
    pub psbt_file: String,
    pub unsigned_txid: String,
    pub fee_sats: u64,
    pub vault_input_indexes: Vec<u32>,
    pub controller_input_indexes: Vec<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConnectorState {
    pub outpoint: OutPoint,
    pub value_sats: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AllowanceStep {
    pub step: u8,
    pub delay_seconds: u32,
    pub delay_sequence: u32,
    pub chain_value_sats: u64,
    pub hot_address: String,
    pub connector: ConnectorState,
    pub next_connector: Option<ConnectorState>,
    pub authorization: BatchTransaction,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EmergencyAccessPolicy {
    pub amount_sats: u64,
    pub delay_seconds: u32,
    pub delay_sequence: u32,
    pub hot_address: String,
    pub staging_vout: u32,
    pub staging_value_sats: u64,
    pub vault_change_vout: u32,
    pub vault_change_value_sats: u64,
    pub trigger_connector: ConnectorState,
    pub withdrawal_connector: ConnectorState,
    pub trigger: BatchTransaction,
    pub withdrawal: BatchTransaction,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BatchManifest {
    pub version: u8,
    pub created_at: i64,
    pub network: String,
    pub vault_descriptor: String,
    pub vault_address: String,
    pub controller_descriptor: String,
    pub controller_address: String,
    pub connector_value_sats: u64,
    #[serde(alias = "hard_limit_sats")]
    pub monthly_limit_sats: u64,
    #[serde(default)]
    pub emergency_access_limit_sats: u64,
    pub fee_rate_sat_vb: u64,
    pub total_input_sats: u64,
    pub vault_input_sats: u64,
    pub controller_input_sats: u64,
    pub allowance_count: usize,
    pub rollover: BatchTransaction,
    pub allowance_vout: Option<u32>,
    pub allowance_value_sats: u64,
    pub remainder_vout: u32,
    pub remainder_value_sats: u64,
    pub monthly_connector_vout: Option<u32>,
    pub emergency_connector_vout: Option<u32>,
    pub allowances: Vec<AllowanceStep>,
    #[serde(default)]
    pub emergency_access: Option<EmergencyAccessPolicy>,
    pub phone_approved: bool,
    pub hww_approved: bool,
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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PolicyPackage {
    pub version: u8,
    pub kind: String,
    pub manifest: BatchManifest,
    pub psbts: BTreeMap<String, String>,
}

/// Supplies fresh phone receive addresses without coupling the protocol rules to a wallet SDK.
pub trait HotAddressProvider {
    fn next_receive_address(&mut self) -> Result<Address>;
}

pub fn default_batch_path(data_dir: &Path) -> PathBuf {
    data_dir.join(DEFAULT_BATCH_DIR)
}

pub fn package_from_batch(batch_dir: &Path) -> Result<PolicyPackage> {
    let manifest = load_manifest(batch_dir)?;
    let mut psbts = BTreeMap::new();
    for transaction in manifest_transactions(&manifest) {
        let text = fs::read_to_string(batch_dir.join(&transaction.psbt_file))
            .with_context(|| format!("failed to read packaged PSBT {}", transaction.psbt_file))?;
        psbts.insert(transaction.psbt_file.clone(), text.trim().to_owned());
    }
    Ok(PolicyPackage {
        version: 5,
        kind: POLICY_PACKAGE_KIND.to_owned(),
        manifest,
        psbts,
    })
}

pub fn is_supported_policy_package(package: &PolicyPackage) -> bool {
    package.version == 5 && package.kind == POLICY_PACKAGE_KIND
}

pub fn materialize_policy_package(package: &PolicyPackage, batch_dir: &Path) -> Result<()> {
    if !is_supported_policy_package(package) {
        bail!("unsupported policy package");
    }
    if batch_dir.exists() && batch_dir.read_dir()?.next().is_some() {
        bail!("policy workspace {} is not empty", batch_dir.display());
    }
    let expected = manifest_transactions(&package.manifest)
        .into_iter()
        .map(|transaction| transaction.psbt_file.clone())
        .collect::<BTreeSet<_>>();
    let actual = package.psbts.keys().cloned().collect::<BTreeSet<_>>();
    if actual != expected {
        bail!("policy package PSBT set does not match its manifest");
    }
    fs::create_dir_all(batch_dir)?;
    write_json(&batch_dir.join("manifest.json"), &package.manifest)?;
    for (relative, psbt) in &package.psbts {
        let relative_path = Path::new(relative);
        if relative_path.is_absolute()
            || relative_path
                .components()
                .any(|component| !matches!(component, Component::Normal(_)))
        {
            bail!("policy package contains an unsafe PSBT path");
        }
        write_private(
            &batch_dir.join(relative_path),
            format!("{psbt}\n").as_bytes(),
        )?;
    }
    Ok(())
}

pub fn build_policy_proposal(
    config: &VaultConfig,
    utxos: &[VaultUtxo],
    now: DateTime<Utc>,
    limits: PolicyLimits,
    batch_dir: &Path,
    phone: &DeviceKeys,
    hot: &mut impl HotAddressProvider,
) -> Result<BatchManifest> {
    build_policy_proposal_with_connectors(config, utxos, &[], now, limits, batch_dir, phone, hot)
}

#[allow(clippy::too_many_arguments)]
pub fn build_policy_proposal_with_connectors(
    config: &VaultConfig,
    vault_utxos: &[VaultUtxo],
    controller_utxos: &[VaultUtxo],
    now: DateTime<Utc>,
    limits: PolicyLimits,
    batch_dir: &Path,
    phone: &DeviceKeys,
    hot: &mut impl HotAddressProvider,
) -> Result<BatchManifest> {
    let PolicyLimits {
        monthly_limit_sats,
        emergency_access_limit_sats,
    } = limits;
    if vault_utxos.is_empty() {
        bail!("vault has no confirmed UTXOs to roll over");
    }
    if batch_dir.exists() && batch_dir.read_dir()?.next().is_some() {
        bail!("ceremony directory {} is not empty", batch_dir.display());
    }
    fs::create_dir_all(batch_dir)?;

    if phone.vault_pubkey.to_string() != config.phone_vault_pubkey {
        bail!("phone key does not match the configured vault policy");
    }
    let policy = VaultPolicy::from_descriptor_for_network(
        &config.vault_descriptor,
        config.bitcoin_network()?,
    )?;
    let controller = controller_policy(config)?;
    let vault_script = policy.address.script_pubkey();
    let controller_script = controller.address.script_pubkey();
    if vault_utxos
        .iter()
        .any(|utxo| utxo.txout.script_pubkey != vault_script)
    {
        bail!("policy proposal contains an input outside the configured vault script");
    }
    if controller_utxos
        .iter()
        .any(|utxo| utxo.txout.script_pubkey != controller_script)
    {
        bail!("policy proposal contains an input outside the fixed controller script");
    }
    let vault_input_sats = checked_input_sum(vault_utxos)?;
    let controller_input_sats = checked_input_sum(controller_utxos)?;
    let total_input_sats = vault_input_sats
        .checked_add(controller_input_sats)
        .context("policy input total overflowed")?;
    let mut input_template = vault_utxos
        .iter()
        .map(|utxo| vault_input(utxo.outpoint, Sequence::MAX))
        .collect::<Vec<_>>();
    input_template.extend(
        controller_utxos
            .iter()
            .map(|utxo| vault_input(utxo.outpoint, Sequence::ENABLE_RBF_NO_LOCKTIME)),
    );
    let vault_input_indexes = (0..vault_utxos.len()).collect::<Vec<_>>();
    let controller_input_indexes =
        (vault_utxos.len()..vault_utxos.len() + controller_utxos.len()).collect::<Vec<_>>();

    let emergency_hot_address = (emergency_access_limit_sats > 0)
        .then(|| hot.next_receive_address())
        .transpose()?;
    let emergency_delay_sequence = emergency_delay_sequence()?;
    let emergency_staging_value_sats = match &emergency_hot_address {
        Some(address) => {
            if emergency_access_limit_sats < address.script_pubkey().minimal_non_dust().to_sat() {
                bail!("emergency access amount would create a dust hot-wallet output");
            }
            let withdrawal_fee = emergency_withdrawal_fee(
                &policy,
                &controller,
                OutPoint::null(),
                emergency_access_limit_sats,
                emergency_delay_sequence,
                address.script_pubkey(),
            )?;
            emergency_access_limit_sats
                .checked_add(withdrawal_fee)
                .and_then(|value| value.checked_sub(CONNECTOR_VALUE_SATS))
                .context("emergency access amount plus withdrawal fee overflowed")?
        }
        None => 0,
    };
    let minimum_remainder_value = if emergency_access_limit_sats == 0 {
        vault_script.minimal_non_dust().to_sat()
    } else {
        let minimum_vault_change = vault_script.minimal_non_dust().to_sat();
        let trigger_fee = emergency_trigger_fee(
            &policy,
            &controller,
            OutPoint::null(),
            emergency_staging_value_sats,
            minimum_vault_change,
            vault_script.clone(),
        )?;
        emergency_staging_value_sats
            .checked_add(trigger_fee)
            .and_then(|value| value.checked_add(minimum_vault_change))
            .context("emergency access reserve overflowed")?
    };

    let monthly_delay_sequence = monthly_delay_sequence()?;
    let (continuing_authorization_fee, final_authorization_fee) = if monthly_limit_sats == 0 {
        (0, 0)
    } else {
        if monthly_limit_sats < vault_script.minimal_non_dust().to_sat() {
            bail!("monthly limit would create a dust hot-wallet output");
        }
        (
            authorization_fee(
                &policy,
                &controller,
                OutPoint::null(),
                monthly_limit_sats,
                monthly_delay_sequence,
                vault_script.clone(),
                true,
            )?,
            authorization_fee(
                &policy,
                &controller,
                OutPoint::null(),
                monthly_limit_sats,
                monthly_delay_sequence,
                vault_script.clone(),
                false,
            )?,
        )
    };

    // The annual rollover creates one allowance-chain output plus one cold remainder. Each
    // relative-delayed authorization releases one allowance and creates the next chain output.
    let mut selected_rollover = None;
    if monthly_limit_sats > 0 {
        for count in (1..=MONTHS_PER_ROLLOVER).rev() {
            let allowance_value = allowance_chain_value(
                count,
                monthly_limit_sats,
                continuing_authorization_fee,
                final_authorization_fee,
            )?;
            let template = rollover_template(
                input_template.clone(),
                Some(allowance_value),
                1,
                vault_script.clone(),
                controller_script.clone(),
                true,
                emergency_access_limit_sats > 0,
            );
            let fee = estimate_policy_vsize(
                &template,
                &policy,
                SpendPath::Cooperative,
                &vault_input_indexes,
                &controller,
                ControllerPath::Phone,
                &controller_input_indexes,
            )? * DEFAULT_FEE_RATE_SAT_VB;
            let connector_outputs = 1 + usize::from(emergency_access_limit_sats > 0);
            let required = fee
                .checked_add(allowance_value)
                .and_then(|value| value.checked_add(minimum_remainder_value))
                .and_then(|value| {
                    value.checked_add(CONNECTOR_VALUE_SATS * connector_outputs as u64)
                })
                .context("allowance rollover requirement overflowed")?;
            if total_input_sats >= required {
                let remainder = total_input_sats
                    - fee
                    - allowance_value
                    - CONNECTOR_VALUE_SATS * connector_outputs as u64;
                selected_rollover = Some((count, allowance_value, fee, remainder));
                break;
            }
        }
    }

    let (allowance_count, allowance_value_sats, rollover_fee, remainder_value_sats) =
        match selected_rollover {
            Some(selected) => selected,
            None => {
                let template = rollover_template(
                    input_template.clone(),
                    None,
                    1,
                    vault_script.clone(),
                    controller_script.clone(),
                    false,
                    emergency_access_limit_sats > 0,
                );
                let fee = estimate_policy_vsize(
                    &template,
                    &policy,
                    SpendPath::Cooperative,
                    &vault_input_indexes,
                    &controller,
                    ControllerPath::Phone,
                    &controller_input_indexes,
                )? * DEFAULT_FEE_RATE_SAT_VB;
                let connector_outputs = usize::from(emergency_access_limit_sats > 0);
                let remainder = total_input_sats
                    .checked_sub(fee)
                    .and_then(|value| {
                        value.checked_sub(CONNECTOR_VALUE_SATS * connector_outputs as u64)
                    })
                    .context("vault balance cannot pay the rollover fee")?;
                (0, 0, fee, remainder)
            }
        };
    if remainder_value_sats < minimum_remainder_value {
        if emergency_access_limit_sats > 0 {
            bail!("vault balance cannot fund the configured emergency access amount and fees");
        }
        bail!("vault balance cannot create a non-dust rollover remainder");
    }

    let rollover_tx = rollover_template(
        input_template,
        (allowance_count > 0).then_some(allowance_value_sats),
        remainder_value_sats,
        vault_script.clone(),
        controller_script.clone(),
        allowance_count > 0,
        emergency_access_limit_sats > 0,
    );
    let rollover_txid = rollover_tx.compute_txid();
    let mut prevouts = vault_utxos
        .iter()
        .map(|utxo| utxo.txout.clone())
        .collect::<Vec<_>>();
    prevouts.extend(controller_utxos.iter().map(|utxo| utxo.txout.clone()));
    let mut rollover_psbt = create_policy_psbt(
        rollover_tx.clone(),
        &prevouts,
        &vault_input_indexes,
        &controller_input_indexes,
        &policy,
        &controller,
    )?;
    sign_vault_psbt_inputs(
        &mut rollover_psbt,
        &policy,
        SpendPath::Cooperative,
        &phone.vault_keypair,
        &vault_input_indexes,
    )?;
    sign_controller_psbt_inputs(
        &mut rollover_psbt,
        &controller,
        ControllerPath::Phone,
        &phone.vault_keypair,
        &controller_input_indexes,
    )?;
    let rollover_file = "rollover.psbt".to_owned();
    write_psbt(&batch_dir.join(&rollover_file), &rollover_psbt)?;

    let remainder_vout = u32::from(allowance_count > 0);
    let monthly_connector_vout = (allowance_count > 0).then_some(remainder_vout + 1);
    let emergency_connector_vout = (emergency_access_limit_sats > 0)
        .then_some(remainder_vout + 1 + u32::from(allowance_count > 0));

    let mut allowances = Vec::with_capacity(allowance_count);
    let mut chain_outpoint = (allowance_count > 0).then(|| OutPoint::new(rollover_txid, 0));
    let mut chain_txout = (allowance_count > 0).then(|| rollover_tx.output[0].clone());
    let mut connector_state = monthly_connector_vout.map(|vout| ConnectorState {
        outpoint: OutPoint::new(rollover_txid, vout),
        value_sats: CONNECTOR_VALUE_SATS,
    });
    for index in 0..allowance_count {
        let step = u8::try_from(index + 1).context("allowance step exceeds u8")?;
        let has_next = index + 1 < allowance_count;
        let hot_address = hot.next_receive_address()?;
        let source_outpoint = chain_outpoint.context("allowance chain source is missing")?;
        let source_txout = chain_txout
            .take()
            .context("allowance chain output is missing")?;
        let source_connector = connector_state
            .take()
            .context("allowance connector is missing")?;
        let authorization_fee = authorization_fee(
            &policy,
            &controller,
            source_outpoint,
            monthly_limit_sats,
            monthly_delay_sequence,
            hot_address.script_pubkey(),
            has_next,
        )?;
        let next_chain_value = source_txout
            .value
            .to_sat()
            .checked_add(CONNECTOR_VALUE_SATS)
            .and_then(|value| {
                if has_next {
                    value.checked_sub(CONNECTOR_VALUE_SATS)
                } else {
                    Some(value)
                }
            })
            .and_then(|value| value.checked_sub(monthly_limit_sats))
            .and_then(|value| value.checked_sub(authorization_fee))
            .context("allowance chain cannot fund its authorization")?;
        if has_next && next_chain_value < vault_script.minimal_non_dust().to_sat() {
            bail!("allowance authorization would create a dust chain output");
        }
        if !has_next && next_chain_value != 0 {
            bail!("final allowance does not exhaust its chain output");
        }
        let authorization_tx = authorization_template(
            source_outpoint,
            source_connector.outpoint,
            monthly_limit_sats,
            monthly_delay_sequence,
            hot_address.script_pubkey(),
            has_next.then(|| (next_chain_value, vault_script.clone())),
            controller_script.clone(),
        );
        let next_connector = has_next.then(|| ConnectorState {
            outpoint: OutPoint::new(authorization_tx.compute_txid(), 2),
            value_sats: CONNECTOR_VALUE_SATS,
        });
        let connector_txout = TxOut {
            value: Amount::from_sat(CONNECTOR_VALUE_SATS),
            script_pubkey: controller_script.clone(),
        };
        let mut authorization_psbt = create_policy_psbt(
            authorization_tx.clone(),
            &[source_txout.clone(), connector_txout],
            &[0],
            &[1],
            &policy,
            &controller,
        )?;
        sign_vault_psbt_inputs(
            &mut authorization_psbt,
            &policy,
            SpendPath::Cooperative,
            &phone.vault_keypair,
            &[0],
        )?;
        let chain_value_sats = source_txout.value.to_sat();

        let step_dir = format!("allowances/{step:02}");
        let authorization_file = format!("{step_dir}/authorization.psbt");
        write_psbt(&batch_dir.join(&authorization_file), &authorization_psbt)?;
        allowances.push(AllowanceStep {
            step,
            delay_seconds: MONTHLY_ALLOWANCE_DELAY_SECONDS,
            delay_sequence: monthly_delay_sequence.to_consensus_u32(),
            chain_value_sats,
            hot_address: hot_address.to_string(),
            connector: source_connector,
            next_connector: next_connector.clone(),
            authorization: BatchTransaction {
                psbt_file: authorization_file,
                unsigned_txid: authorization_tx.compute_txid().to_string(),
                fee_sats: authorization_fee,
                vault_input_indexes: vec![0],
                controller_input_indexes: vec![1],
            },
        });
        if has_next {
            chain_outpoint = Some(OutPoint::new(authorization_tx.compute_txid(), 1));
            chain_txout = Some(authorization_tx.output[1].clone());
            connector_state = next_connector;
        } else {
            chain_outpoint = None;
        }
    }

    let emergency_access = match emergency_hot_address {
        Some(hot_address) => {
            let source_outpoint = OutPoint::new(rollover_txid, remainder_vout);
            let source_txout = rollover_tx.output[remainder_vout as usize].clone();
            Some(build_emergency_access(
                &policy,
                &controller,
                source_outpoint,
                &source_txout,
                ConnectorState {
                    outpoint: OutPoint::new(
                        rollover_txid,
                        emergency_connector_vout.context("emergency connector vout is missing")?,
                    ),
                    value_sats: CONNECTOR_VALUE_SATS,
                },
                emergency_access_limit_sats,
                emergency_delay_sequence,
                &hot_address,
                vault_script.clone(),
                batch_dir,
                phone,
            )?)
        }
        None => None,
    };

    let manifest = BatchManifest {
        version: 5,
        created_at: now.timestamp(),
        network: config.network.clone(),
        vault_descriptor: config.vault_descriptor.clone(),
        vault_address: config.vault_address.clone(),
        controller_descriptor: controller.descriptor_string(),
        controller_address: controller.address.to_string(),
        connector_value_sats: CONNECTOR_VALUE_SATS,
        monthly_limit_sats,
        emergency_access_limit_sats,
        fee_rate_sat_vb: DEFAULT_FEE_RATE_SAT_VB,
        total_input_sats,
        vault_input_sats,
        controller_input_sats,
        allowance_count,
        rollover: BatchTransaction {
            psbt_file: rollover_file,
            unsigned_txid: rollover_txid.to_string(),
            fee_sats: rollover_fee,
            vault_input_indexes: vault_input_indexes
                .iter()
                .map(|index| *index as u32)
                .collect(),
            controller_input_indexes: controller_input_indexes
                .iter()
                .map(|index| *index as u32)
                .collect(),
        },
        allowance_vout: (allowance_count > 0).then_some(0),
        allowance_value_sats,
        remainder_vout,
        remainder_value_sats,
        monthly_connector_vout,
        emergency_connector_vout,
        allowances,
        emergency_access,
        phone_approved: true,
        hww_approved: false,
    };
    write_json(&batch_dir.join("manifest.json"), &manifest)?;
    Ok(manifest)
}

pub fn load_manifest(batch_dir: &Path) -> Result<BatchManifest> {
    read_json(&batch_dir.join("manifest.json"))
}

pub fn validate_batch(
    config: &VaultConfig,
    manifest: &BatchManifest,
    batch_dir: &Path,
) -> Result<VaultPolicy> {
    if manifest.version != 5 || manifest.network != config.network {
        bail!("unsupported ceremony manifest or network mismatch");
    }
    for transaction in manifest_transactions(manifest) {
        validate_default_sighashes(&read_psbt(&batch_dir.join(&transaction.psbt_file))?)?;
    }
    if manifest.vault_descriptor != config.vault_descriptor
        || manifest.vault_address != config.vault_address
    {
        bail!("ceremony policy does not match configured vault policy");
    }
    if manifest.fee_rate_sat_vb != DEFAULT_FEE_RATE_SAT_VB {
        bail!("MVP ceremony must use the fixed 1 sat/vB fee rate");
    }
    if (manifest.monthly_limit_sats == 0 && manifest.allowance_count != 0)
        || manifest.allowance_count > MONTHS_PER_ROLLOVER
        || manifest.allowances.len() != manifest.allowance_count
    {
        bail!("invalid ceremony allowance count");
    }

    let policy = VaultPolicy::from_descriptor_for_network(
        &config.vault_descriptor,
        config.bitcoin_network()?,
    )?;
    let controller = controller_policy(config)?;
    if manifest.controller_descriptor != controller.descriptor_string()
        || manifest.controller_address != controller.address.to_string()
        || manifest.connector_value_sats != CONNECTOR_VALUE_SATS
    {
        bail!("ceremony controller does not match the fixed vault-key controller policy");
    }
    let vault_script = policy.address.script_pubkey();
    let controller_script = controller.address.script_pubkey();

    let rollover = read_psbt(&batch_dir.join(&manifest.rollover.psbt_file))?;
    let rollover_tx = &rollover.unsigned_tx;
    let vault_indexes = transaction_indexes(&manifest.rollover.vault_input_indexes)?;
    let controller_indexes = transaction_indexes(&manifest.rollover.controller_input_indexes)?;
    validate_input_partition(rollover_tx.input.len(), &vault_indexes, &controller_indexes)?;
    let expected_output_count = usize::from(manifest.allowance_count > 0)
        + 1
        + usize::from(manifest.allowance_count > 0)
        + usize::from(manifest.emergency_access_limit_sats > 0);
    if rollover_tx.compute_txid().to_string() != manifest.rollover.unsigned_txid
        || rollover_tx.version != Version::TWO
        || rollover_tx.lock_time != absolute::LockTime::ZERO
        || rollover_tx.input.is_empty()
        || rollover_tx.input.len() != rollover.inputs.len()
        || rollover_tx.output.len() != expected_output_count
        || vault_indexes
            .iter()
            .any(|index| rollover_tx.input[*index].sequence != Sequence::MAX)
        || controller_indexes
            .iter()
            .any(|index| rollover_tx.input[*index].sequence != Sequence::ENABLE_RBF_NO_LOCKTIME)
    {
        bail!("rollover PSBT does not match its manifest");
    }
    let calculated_vault_inputs =
        psbt_role_input_sum(&rollover, &vault_indexes, &vault_script, "rollover vault")?;
    let calculated_controller_inputs = psbt_role_input_sum(
        &rollover,
        &controller_indexes,
        &controller_script,
        "rollover controller",
    )?;
    let calculated_total = calculated_vault_inputs
        .checked_add(calculated_controller_inputs)
        .context("rollover input total overflowed")?;
    let fee = calculated_total
        .checked_sub(output_sum(rollover_tx)?)
        .context("rollover outputs exceed inputs")?;
    if calculated_vault_inputs != manifest.vault_input_sats
        || calculated_controller_inputs != manifest.controller_input_sats
        || calculated_total != manifest.total_input_sats
        || fee != manifest.rollover.fee_sats
    {
        bail!("rollover amount or fee does not match its manifest");
    }
    let expected_rollover_fee = estimate_policy_vsize(
        rollover_tx,
        &policy,
        SpendPath::Cooperative,
        &vault_indexes,
        &controller,
        ControllerPath::Phone,
        &controller_indexes,
    )? * DEFAULT_FEE_RATE_SAT_VB;
    if manifest.rollover.fee_sats != expected_rollover_fee {
        bail!("rollover does not pay the approved fixed fee rate");
    }

    let has_allowances = manifest.allowance_count > 0;
    let expected_allowance_vout = has_allowances.then_some(0);
    let expected_remainder_vout = u32::from(has_allowances);
    let expected_monthly_connector_vout = has_allowances.then_some(expected_remainder_vout + 1);
    let expected_emergency_connector_vout = (manifest.emergency_access_limit_sats > 0)
        .then_some(expected_remainder_vout + 1 + u32::from(has_allowances));
    if manifest.allowance_vout != expected_allowance_vout
        || manifest.remainder_vout != expected_remainder_vout
        || manifest.monthly_connector_vout != expected_monthly_connector_vout
        || manifest.emergency_connector_vout != expected_emergency_connector_vout
        || (!has_allowances && manifest.allowance_value_sats != 0)
    {
        bail!("rollover output indexes do not match the approved policy");
    }
    if has_allowances
        && (rollover_tx.output[0].script_pubkey != vault_script
            || rollover_tx.output[0].value.to_sat() != manifest.allowance_value_sats)
    {
        bail!("rollover allowance-chain output does not match its manifest");
    }
    let remainder = &rollover_tx.output[manifest.remainder_vout as usize];
    if remainder.script_pubkey != vault_script
        || remainder.value.to_sat() != manifest.remainder_value_sats
        || remainder.value.to_sat() < vault_script.minimal_non_dust().to_sat()
    {
        bail!("rollover remainder does not match its manifest");
    }
    for vout in [
        manifest.monthly_connector_vout,
        manifest.emergency_connector_vout,
    ]
    .into_iter()
    .flatten()
    {
        validate_connector_output(
            rollover_tx
                .output
                .get(vout as usize)
                .context("rollover connector vout is out of range")?,
            &controller_script,
        )?;
    }

    validate_allowances(config, manifest, batch_dir, &policy, &controller, &rollover)?;
    validate_connector_emergency(config, manifest, batch_dir, &policy, &controller, &rollover)?;
    Ok(policy)
}

fn validate_allowances(
    config: &VaultConfig,
    manifest: &BatchManifest,
    batch_dir: &Path,
    policy: &VaultPolicy,
    controller: &ControllerPolicy,
    rollover: &Psbt,
) -> Result<()> {
    let has_allowances = manifest.allowance_count > 0;
    let vault_script = policy.address.script_pubkey();
    let controller_script = controller.address.script_pubkey();
    let mut vault_outpoint = has_allowances.then(|| {
        OutPoint::new(
            rollover.unsigned_tx.compute_txid(),
            manifest.allowance_vout.unwrap(),
        )
    });
    let mut vault_prevout = has_allowances.then(|| rollover.unsigned_tx.output[0].clone());
    let mut connector = manifest.monthly_connector_vout.map(|vout| ConnectorState {
        outpoint: OutPoint::new(rollover.unsigned_tx.compute_txid(), vout),
        value_sats: CONNECTOR_VALUE_SATS,
    });
    let expected_delay = monthly_delay_sequence()?;

    for (index, allowance) in manifest.allowances.iter().enumerate() {
        let step = u8::try_from(index + 1).context("allowance step exceeds u8")?;
        let has_next = index + 1 < manifest.allowance_count;
        let source_outpoint = vault_outpoint.context("allowance vault outpoint is missing")?;
        let source_prevout = vault_prevout
            .take()
            .context("allowance vault output is missing")?;
        let source_connector = connector.take().context("allowance connector is missing")?;
        if allowance.step != step
            || allowance.delay_seconds != MONTHLY_ALLOWANCE_DELAY_SECONDS
            || allowance.delay_sequence != expected_delay.to_consensus_u32()
            || allowance.chain_value_sats != source_prevout.value.to_sat()
            || allowance.connector != source_connector
        {
            bail!("allowance step {step} metadata violates the approved policy");
        }
        let hot_script = Address::from_str(&allowance.hot_address)?
            .require_network(config.bitcoin_network()?)?
            .script_pubkey();
        let psbt = read_psbt(&batch_dir.join(&allowance.authorization.psbt_file))?;
        validate_connector_child(
            &psbt,
            &source_prevout,
            source_outpoint,
            source_connector.outpoint,
            &controller_script,
            &allowance.authorization,
        )?;
        let tx = &psbt.unsigned_tx;
        if tx.version != Version::TWO
            || tx.lock_time != absolute::LockTime::ZERO
            || tx.input[0].sequence != expected_delay
            || tx.input[1].sequence != Sequence::ENABLE_RBF_NO_LOCKTIME
            || tx.output.len() != if has_next { 3 } else { 1 }
            || tx.output[0].script_pubkey != hot_script
            || tx.output[0].value.to_sat() != manifest.monthly_limit_sats
        {
            bail!("allowance authorization step {step} violates the approved policy");
        }
        if has_next {
            let next_value = source_prevout
                .value
                .to_sat()
                .checked_sub(manifest.monthly_limit_sats)
                .and_then(|value| value.checked_sub(allowance.authorization.fee_sats))
                .context("allowance authorization exceeds its vault input")?;
            if tx.output[1].script_pubkey != vault_script
                || tx.output[1].value.to_sat() != next_value
                || next_value < vault_script.minimal_non_dust().to_sat()
            {
                bail!("allowance step {step} does not create the approved next vault hop");
            }
            validate_connector_output(&tx.output[2], &controller_script)?;
            let expected_next_connector = ConnectorState {
                outpoint: OutPoint::new(tx.compute_txid(), 2),
                value_sats: CONNECTOR_VALUE_SATS,
            };
            if allowance.next_connector.as_ref() != Some(&expected_next_connector) {
                bail!("allowance step {step} connector metadata is discontinuous");
            }
            vault_outpoint = Some(OutPoint::new(tx.compute_txid(), 1));
            vault_prevout = Some(tx.output[1].clone());
            connector = Some(expected_next_connector);
        } else if source_prevout
            .value
            .to_sat()
            .checked_add(CONNECTOR_VALUE_SATS)
            != manifest
                .monthly_limit_sats
                .checked_add(allowance.authorization.fee_sats)
            || allowance.next_connector.is_some()
        {
            bail!("final allowance does not consume its connector without creating another");
        }
        let expected_fee = estimate_policy_vsize(
            tx,
            policy,
            SpendPath::Cooperative,
            &[0],
            controller,
            ControllerPath::Phone,
            &[1],
        )? * DEFAULT_FEE_RATE_SAT_VB;
        if allowance.authorization.fee_sats != expected_fee {
            bail!("allowance transaction does not pay the approved fixed fee rate");
        }
    }
    Ok(())
}

fn validate_connector_emergency(
    config: &VaultConfig,
    manifest: &BatchManifest,
    batch_dir: &Path,
    policy: &VaultPolicy,
    controller: &ControllerPolicy,
    rollover: &Psbt,
) -> Result<()> {
    let emergency = match (
        manifest.emergency_access_limit_sats,
        &manifest.emergency_access,
    ) {
        (0, None) => return Ok(()),
        (0, Some(_)) => bail!("disabled emergency access contains presigned transactions"),
        (_, None) => bail!("configured emergency access is missing its presigned transactions"),
        (_, Some(emergency)) => emergency,
    };
    let expected_delay = emergency_delay_sequence()?;
    if emergency.amount_sats != manifest.emergency_access_limit_sats
        || emergency.delay_seconds != EMERGENCY_ACCESS_DELAY_SECONDS
        || emergency.delay_sequence != expected_delay.to_consensus_u32()
        || emergency.staging_vout != 0
        || emergency.vault_change_vout != 1
    {
        bail!("emergency access metadata violates the approved policy");
    }
    let hot_script = Address::from_str(&emergency.hot_address)?
        .require_network(config.bitcoin_network()?)?
        .script_pubkey();
    let vault_script = policy.address.script_pubkey();
    let controller_script = controller.address.script_pubkey();
    let source_outpoint =
        OutPoint::new(rollover.unsigned_tx.compute_txid(), manifest.remainder_vout);
    let source_prevout = &rollover.unsigned_tx.output[manifest.remainder_vout as usize];
    let trigger_connector = ConnectorState {
        outpoint: OutPoint::new(
            rollover.unsigned_tx.compute_txid(),
            manifest
                .emergency_connector_vout
                .context("emergency connector output is missing")?,
        ),
        value_sats: CONNECTOR_VALUE_SATS,
    };
    if emergency.trigger_connector != trigger_connector {
        bail!("emergency trigger connector metadata violates the approved policy");
    }
    let trigger = read_psbt(&batch_dir.join(&emergency.trigger.psbt_file))?;
    validate_connector_child(
        &trigger,
        source_prevout,
        source_outpoint,
        trigger_connector.outpoint,
        &controller_script,
        &emergency.trigger,
    )?;
    let trigger_tx = &trigger.unsigned_tx;
    if trigger_tx.version != Version::TWO
        || trigger_tx.lock_time != absolute::LockTime::ZERO
        || trigger_tx.input[0].sequence != Sequence::MAX
        || trigger_tx.input[1].sequence != Sequence::ENABLE_RBF_NO_LOCKTIME
        || trigger_tx.output.len() != 3
        || trigger_tx.output[0].script_pubkey != vault_script
        || trigger_tx.output[0].value.to_sat() != emergency.staging_value_sats
        || trigger_tx.output[1].script_pubkey != vault_script
        || trigger_tx.output[1].value.to_sat() != emergency.vault_change_value_sats
        || emergency.vault_change_value_sats < vault_script.minimal_non_dust().to_sat()
    {
        bail!("emergency access trigger violates the approved policy");
    }
    validate_connector_output(&trigger_tx.output[2], &controller_script)?;
    let withdrawal_connector = ConnectorState {
        outpoint: OutPoint::new(trigger_tx.compute_txid(), 2),
        value_sats: CONNECTOR_VALUE_SATS,
    };
    if emergency.withdrawal_connector != withdrawal_connector {
        bail!("emergency withdrawal connector metadata violates the approved policy");
    }
    let trigger_fee = estimate_policy_vsize(
        trigger_tx,
        policy,
        SpendPath::Cooperative,
        &[0],
        controller,
        ControllerPath::Phone,
        &[1],
    )? * DEFAULT_FEE_RATE_SAT_VB;
    if emergency.trigger.fee_sats != trigger_fee {
        bail!("emergency trigger does not pay the approved fixed fee rate");
    }

    let staging_outpoint = OutPoint::new(trigger_tx.compute_txid(), emergency.staging_vout);
    let staging_prevout = &trigger_tx.output[emergency.staging_vout as usize];
    let withdrawal = read_psbt(&batch_dir.join(&emergency.withdrawal.psbt_file))?;
    validate_connector_child(
        &withdrawal,
        staging_prevout,
        staging_outpoint,
        withdrawal_connector.outpoint,
        &controller_script,
        &emergency.withdrawal,
    )?;
    let withdrawal_tx = &withdrawal.unsigned_tx;
    if withdrawal_tx.version != Version::TWO
        || withdrawal_tx.lock_time != absolute::LockTime::ZERO
        || withdrawal_tx.input[0].sequence != expected_delay
        || withdrawal_tx.input[1].sequence != Sequence::ENABLE_RBF_NO_LOCKTIME
        || withdrawal_tx.output.len() != 1
        || withdrawal_tx.output[0].script_pubkey != hot_script
        || withdrawal_tx.output[0].value.to_sat() != manifest.emergency_access_limit_sats
        || emergency
            .staging_value_sats
            .checked_add(CONNECTOR_VALUE_SATS)
            != manifest
                .emergency_access_limit_sats
                .checked_add(emergency.withdrawal.fee_sats)
    {
        bail!("emergency access withdrawal violates the approved policy");
    }
    let withdrawal_fee = estimate_policy_vsize(
        withdrawal_tx,
        policy,
        SpendPath::Cooperative,
        &[0],
        controller,
        ControllerPath::Phone,
        &[1],
    )? * DEFAULT_FEE_RATE_SAT_VB;
    if emergency.withdrawal.fee_sats != withdrawal_fee {
        bail!("emergency withdrawal does not pay the approved fixed fee rate");
    }
    Ok(())
}

fn validate_connector_child(
    psbt: &Psbt,
    vault_prevout: &TxOut,
    vault_outpoint: OutPoint,
    connector_outpoint: OutPoint,
    controller_script: &ScriptBuf,
    manifest_tx: &BatchTransaction,
) -> Result<()> {
    if manifest_tx.vault_input_indexes != [0]
        || manifest_tx.controller_input_indexes != [1]
        || psbt.unsigned_tx.compute_txid().to_string() != manifest_tx.unsigned_txid
        || psbt.unsigned_tx.input.len() != 2
        || psbt.inputs.len() != 2
        || psbt.unsigned_tx.input[0].previous_output != vault_outpoint
        || psbt.inputs[0].witness_utxo.as_ref() != Some(vault_prevout)
        || psbt.unsigned_tx.input[1].previous_output != connector_outpoint
        || !psbt.inputs[1].tap_script_sigs.is_empty()
    {
        bail!("policy PSBT does not spend its assigned vault and unsigned connector outputs");
    }
    let connector_prevout = psbt.inputs[1]
        .witness_utxo
        .as_ref()
        .context("policy connector input lacks witness UTXO")?;
    if connector_prevout.script_pubkey != *controller_script
        || connector_prevout.value.to_sat() != CONNECTOR_VALUE_SATS
    {
        bail!("policy PSBT connector input violates the fixed controller policy");
    }
    let input = vault_prevout
        .value
        .to_sat()
        .checked_add(CONNECTOR_VALUE_SATS)
        .context("policy transaction input sum overflowed")?;
    let fee = input
        .checked_sub(output_sum(&psbt.unsigned_tx)?)
        .context("policy transaction outputs exceed inputs")?;
    if fee != manifest_tx.fee_sats {
        bail!("policy transaction fee does not match its manifest");
    }
    Ok(())
}

fn validate_connector_output(output: &TxOut, controller_script: &ScriptBuf) -> Result<()> {
    if output.script_pubkey != *controller_script || output.value.to_sat() != CONNECTOR_VALUE_SATS {
        bail!("connector output violates the fixed controller script or value");
    }
    Ok(())
}

fn transaction_indexes(indexes: &[u32]) -> Result<Vec<usize>> {
    indexes
        .iter()
        .map(|index| usize::try_from(*index).context("policy input index exceeds usize"))
        .collect()
}

fn validate_input_partition(
    input_count: usize,
    vault_indexes: &[usize],
    controller_indexes: &[usize],
) -> Result<()> {
    let vault = vault_indexes.iter().copied().collect::<BTreeSet<_>>();
    let controller = controller_indexes.iter().copied().collect::<BTreeSet<_>>();
    if vault.len() != vault_indexes.len()
        || controller.len() != controller_indexes.len()
        || !vault.is_disjoint(&controller)
        || vault.len() + controller.len() != input_count
        || vault
            .iter()
            .chain(&controller)
            .any(|index| *index >= input_count)
    {
        bail!("policy input roles do not form an exact input partition");
    }
    Ok(())
}

fn psbt_role_input_sum(
    psbt: &Psbt,
    indexes: &[usize],
    expected_script: &ScriptBuf,
    label: &str,
) -> Result<u64> {
    indexes.iter().try_fold(0_u64, |sum, index| {
        let prevout = psbt.inputs[*index]
            .witness_utxo
            .as_ref()
            .with_context(|| format!("{label} input lacks witness UTXO"))?;
        if prevout.script_pubkey != *expected_script {
            bail!("{label} input uses an unexpected script");
        }
        sum.checked_add(prevout.value.to_sat())
            .with_context(|| format!("{label} input sum overflowed"))
    })
}

fn controller_policy(config: &VaultConfig) -> Result<ControllerPolicy> {
    let phone = bitcoin::secp256k1::XOnlyPublicKey::from_str(&config.phone_vault_pubkey)
        .context("invalid configured phone vault key")?;
    let hww = bitcoin::secp256k1::XOnlyPublicKey::from_str(&config.hww_vault_pubkey)
        .context("invalid configured HWW vault key")?;
    ControllerPolicy::new_for_network(phone, hww, config.bitcoin_network()?)
}

#[allow(clippy::too_many_arguments)]
fn build_emergency_access(
    policy: &VaultPolicy,
    controller: &ControllerPolicy,
    source_outpoint: OutPoint,
    source_txout: &TxOut,
    trigger_connector: ConnectorState,
    amount_sats: u64,
    delay_sequence: Sequence,
    hot_address: &Address,
    vault_script: ScriptBuf,
    batch_dir: &Path,
    phone: &DeviceKeys,
) -> Result<EmergencyAccessPolicy> {
    let controller_script = controller.address.script_pubkey();
    let withdrawal_fee = emergency_withdrawal_fee(
        policy,
        controller,
        OutPoint::null(),
        amount_sats,
        delay_sequence,
        hot_address.script_pubkey(),
    )?;
    let staging_value_sats = amount_sats
        .checked_add(withdrawal_fee)
        .and_then(|value| value.checked_sub(CONNECTOR_VALUE_SATS))
        .context("emergency access amount and fee cannot be funded with the connector")?;
    if staging_value_sats < vault_script.minimal_non_dust().to_sat() {
        bail!("emergency staging output would be dust");
    }
    let trigger_fee = emergency_trigger_fee(
        policy,
        controller,
        source_outpoint,
        staging_value_sats,
        vault_script.minimal_non_dust().to_sat(),
        vault_script.clone(),
    )?;
    let vault_change_value_sats = source_txout
        .value
        .to_sat()
        .checked_sub(staging_value_sats)
        .and_then(|value| value.checked_sub(trigger_fee))
        .context("vault remainder cannot fund the configured emergency access amount and fees")?;
    if vault_change_value_sats < vault_script.minimal_non_dust().to_sat() {
        bail!("emergency access trigger would create dust vault change");
    }
    let trigger_tx = emergency_trigger_template(
        source_outpoint,
        trigger_connector.outpoint,
        staging_value_sats,
        vault_change_value_sats,
        vault_script.clone(),
        controller_script.clone(),
    );
    let connector_prevout = TxOut {
        value: Amount::from_sat(CONNECTOR_VALUE_SATS),
        script_pubkey: controller_script.clone(),
    };
    let mut trigger_psbt = create_policy_psbt(
        trigger_tx.clone(),
        &[source_txout.clone(), connector_prevout.clone()],
        &[0],
        &[1],
        policy,
        controller,
    )?;
    sign_vault_psbt_inputs(
        &mut trigger_psbt,
        policy,
        SpendPath::Cooperative,
        &phone.vault_keypair,
        &[0],
    )?;

    let staging_outpoint = OutPoint::new(trigger_tx.compute_txid(), 0);
    let staging_txout = &trigger_tx.output[0];
    let withdrawal_connector = ConnectorState {
        outpoint: OutPoint::new(trigger_tx.compute_txid(), 2),
        value_sats: CONNECTOR_VALUE_SATS,
    };
    let withdrawal_tx = emergency_withdrawal_template(
        staging_outpoint,
        withdrawal_connector.outpoint,
        amount_sats,
        delay_sequence,
        hot_address.script_pubkey(),
    );
    let mut withdrawal_psbt = create_policy_psbt(
        withdrawal_tx.clone(),
        &[staging_txout.clone(), connector_prevout],
        &[0],
        &[1],
        policy,
        controller,
    )?;
    sign_vault_psbt_inputs(
        &mut withdrawal_psbt,
        policy,
        SpendPath::Cooperative,
        &phone.vault_keypair,
        &[0],
    )?;

    let trigger_file = "emergency/trigger.psbt".to_owned();
    let withdrawal_file = "emergency/withdrawal.psbt".to_owned();
    write_psbt(&batch_dir.join(&trigger_file), &trigger_psbt)?;
    write_psbt(&batch_dir.join(&withdrawal_file), &withdrawal_psbt)?;

    Ok(EmergencyAccessPolicy {
        amount_sats,
        delay_seconds: EMERGENCY_ACCESS_DELAY_SECONDS,
        delay_sequence: delay_sequence.to_consensus_u32(),
        hot_address: hot_address.to_string(),
        staging_vout: 0,
        staging_value_sats,
        vault_change_vout: 1,
        vault_change_value_sats,
        trigger_connector,
        withdrawal_connector,
        trigger: BatchTransaction {
            psbt_file: trigger_file,
            unsigned_txid: trigger_tx.compute_txid().to_string(),
            fee_sats: trigger_fee,
            vault_input_indexes: vec![0],
            controller_input_indexes: vec![1],
        },
        withdrawal: BatchTransaction {
            psbt_file: withdrawal_file,
            unsigned_txid: withdrawal_tx.compute_txid().to_string(),
            fee_sats: withdrawal_fee,
            vault_input_indexes: vec![0],
            controller_input_indexes: vec![1],
        },
    })
}

fn emergency_trigger_fee(
    policy: &VaultPolicy,
    controller: &ControllerPolicy,
    outpoint: OutPoint,
    staging_value_sats: u64,
    vault_change_value_sats: u64,
    vault_script: ScriptBuf,
) -> Result<u64> {
    let template = emergency_trigger_template(
        outpoint,
        OutPoint::null(),
        staging_value_sats,
        vault_change_value_sats,
        vault_script,
        controller.address.script_pubkey(),
    );
    Ok(estimate_policy_vsize(
        &template,
        policy,
        SpendPath::Cooperative,
        &[0],
        controller,
        ControllerPath::Phone,
        &[1],
    )? * DEFAULT_FEE_RATE_SAT_VB)
}

fn emergency_trigger_template(
    vault_outpoint: OutPoint,
    connector_outpoint: OutPoint,
    staging_value_sats: u64,
    vault_change_value_sats: u64,
    vault_script: ScriptBuf,
    controller_script: ScriptBuf,
) -> Transaction {
    Transaction {
        version: Version::TWO,
        lock_time: absolute::LockTime::ZERO,
        input: vec![
            vault_input(vault_outpoint, Sequence::MAX),
            vault_input(connector_outpoint, Sequence::ENABLE_RBF_NO_LOCKTIME),
        ],
        output: vec![
            TxOut {
                value: Amount::from_sat(staging_value_sats),
                script_pubkey: vault_script.clone(),
            },
            TxOut {
                value: Amount::from_sat(vault_change_value_sats),
                script_pubkey: vault_script,
            },
            TxOut {
                value: Amount::from_sat(CONNECTOR_VALUE_SATS),
                script_pubkey: controller_script,
            },
        ],
    }
}

fn emergency_withdrawal_fee(
    policy: &VaultPolicy,
    controller: &ControllerPolicy,
    outpoint: OutPoint,
    amount_sats: u64,
    delay_sequence: Sequence,
    hot_script: ScriptBuf,
) -> Result<u64> {
    let template = emergency_withdrawal_template(
        outpoint,
        OutPoint::null(),
        amount_sats,
        delay_sequence,
        hot_script,
    );
    Ok(estimate_policy_vsize(
        &template,
        policy,
        SpendPath::Cooperative,
        &[0],
        controller,
        ControllerPath::Phone,
        &[1],
    )? * DEFAULT_FEE_RATE_SAT_VB)
}

fn emergency_withdrawal_template(
    vault_outpoint: OutPoint,
    connector_outpoint: OutPoint,
    amount_sats: u64,
    delay_sequence: Sequence,
    hot_script: ScriptBuf,
) -> Transaction {
    Transaction {
        version: Version::TWO,
        lock_time: absolute::LockTime::ZERO,
        input: vec![
            vault_input(vault_outpoint, delay_sequence),
            vault_input(connector_outpoint, Sequence::ENABLE_RBF_NO_LOCKTIME),
        ],
        output: vec![TxOut {
            value: Amount::from_sat(amount_sats),
            script_pubkey: hot_script,
        }],
    }
}

fn authorization_fee(
    policy: &VaultPolicy,
    controller: &ControllerPolicy,
    outpoint: OutPoint,
    monthly_limit: u64,
    delay_sequence: Sequence,
    hot_script: ScriptBuf,
    has_next: bool,
) -> Result<u64> {
    let template = authorization_template(
        outpoint,
        OutPoint::null(),
        monthly_limit,
        delay_sequence,
        hot_script,
        has_next.then(|| (1, policy.address.script_pubkey())),
        controller.address.script_pubkey(),
    );
    Ok(estimate_policy_vsize(
        &template,
        policy,
        SpendPath::Cooperative,
        &[0],
        controller,
        ControllerPath::Phone,
        &[1],
    )? * DEFAULT_FEE_RATE_SAT_VB)
}

fn authorization_template(
    vault_outpoint: OutPoint,
    connector_outpoint: OutPoint,
    monthly_limit: u64,
    delay_sequence: Sequence,
    hot_script: ScriptBuf,
    next_chain: Option<(u64, ScriptBuf)>,
    controller_script: ScriptBuf,
) -> Transaction {
    let mut output = vec![TxOut {
        value: Amount::from_sat(monthly_limit),
        script_pubkey: hot_script,
    }];
    if let Some((value, script_pubkey)) = next_chain {
        output.push(TxOut {
            value: Amount::from_sat(value),
            script_pubkey,
        });
        output.push(TxOut {
            value: Amount::from_sat(CONNECTOR_VALUE_SATS),
            script_pubkey: controller_script,
        });
    }
    Transaction {
        version: Version::TWO,
        lock_time: absolute::LockTime::ZERO,
        input: vec![
            vault_input(vault_outpoint, delay_sequence),
            vault_input(connector_outpoint, Sequence::ENABLE_RBF_NO_LOCKTIME),
        ],
        output,
    }
}

fn rollover_template(
    inputs: Vec<TxIn>,
    allowance_value: Option<u64>,
    remainder_value: u64,
    vault_script: ScriptBuf,
    controller_script: ScriptBuf,
    monthly_connector: bool,
    emergency_connector: bool,
) -> Transaction {
    let mut outputs = Vec::with_capacity(
        usize::from(allowance_value.is_some())
            + 1
            + usize::from(monthly_connector)
            + usize::from(emergency_connector),
    );
    if let Some(value) = allowance_value {
        outputs.push(TxOut {
            value: Amount::from_sat(value),
            script_pubkey: vault_script.clone(),
        });
    }
    outputs.push(TxOut {
        value: Amount::from_sat(remainder_value),
        script_pubkey: vault_script,
    });
    for _ in 0..usize::from(monthly_connector) + usize::from(emergency_connector) {
        outputs.push(TxOut {
            value: Amount::from_sat(CONNECTOR_VALUE_SATS),
            script_pubkey: controller_script.clone(),
        });
    }
    Transaction {
        version: Version::TWO,
        lock_time: absolute::LockTime::ZERO,
        input: inputs,
        output: outputs,
    }
}

fn emergency_delay_sequence() -> Result<Sequence> {
    Sequence::from_seconds_ceil(EMERGENCY_ACCESS_DELAY_SECONDS)
        .context("emergency access delay cannot be represented by BIP68")
}

fn monthly_delay_sequence() -> Result<Sequence> {
    Sequence::from_seconds_ceil(MONTHLY_ALLOWANCE_DELAY_SECONDS)
        .context("monthly allowance delay cannot be represented by BIP68")
}

fn allowance_chain_value(
    count: usize,
    monthly_limit_sats: u64,
    continuing_authorization_fee_sats: u64,
    final_authorization_fee_sats: u64,
) -> Result<u64> {
    if count == 0 {
        return Ok(0);
    }
    let allowances = monthly_limit_sats
        .checked_mul(u64::try_from(count).context("allowance count exceeds u64")?)
        .context("allowance chain value overflowed")?;
    let continuing_fees = continuing_authorization_fee_sats
        .checked_mul(u64::try_from(count - 1).context("allowance count exceeds u64")?)
        .context("allowance chain fees overflowed")?;
    allowances
        .checked_add(continuing_fees)
        .and_then(|value| value.checked_add(final_authorization_fee_sats))
        .and_then(|value| value.checked_sub(CONNECTOR_VALUE_SATS))
        .context("allowance chain value overflowed")
}

fn vault_input(outpoint: OutPoint, sequence: Sequence) -> TxIn {
    TxIn {
        previous_output: outpoint,
        script_sig: ScriptBuf::new(),
        sequence,
        witness: Witness::new(),
    }
}

fn checked_input_sum(utxos: &[VaultUtxo]) -> Result<u64> {
    utxos.iter().try_fold(0_u64, |sum, utxo| {
        sum.checked_add(utxo.txout.value.to_sat())
            .context("vault input total overflowed")
    })
}

fn output_sum(transaction: &Transaction) -> Result<u64> {
    transaction.output.iter().try_fold(0_u64, |sum, output| {
        sum.checked_add(output.value.to_sat())
            .context("transaction output sum overflowed")
    })
}

pub fn write_psbt(path: &Path, psbt: &Psbt) -> Result<()> {
    write_private(path, format!("{psbt}\n").as_bytes())
}

pub fn read_psbt(path: &Path) -> Result<Psbt> {
    let text = fs::read_to_string(path)
        .with_context(|| format!("failed to read PSBT {}", path.display()))?;
    Psbt::from_str(text.trim()).with_context(|| format!("invalid PSBT in {}", path.display()))
}

pub fn manifest_transactions(manifest: &BatchManifest) -> Vec<&BatchTransaction> {
    let mut transactions = Vec::with_capacity(3 + manifest.allowances.len());
    transactions.push(&manifest.rollover);
    for allowance in &manifest.allowances {
        transactions.push(&allowance.authorization);
    }
    if let Some(emergency) = &manifest.emergency_access {
        transactions.push(&emergency.trigger);
        transactions.push(&emergency.withdrawal);
    }
    transactions
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        cold_wallet,
        core::{
            storage::{PHONE_DEVICE_FILE, load_device_keys},
            transactions::finalize_vault_psbt,
        },
        hot_wallet::HotWallet,
        test_support::{initialize, initialize_for_network},
    };
    use bitcoin::{Network, Txid, hashes::Hash};
    use chrono::TimeZone;

    fn prepare_from_utxos(
        data_dir: &Path,
        config: &VaultConfig,
        utxos: &[VaultUtxo],
        now: DateTime<Utc>,
        monthly_limit_sats: u64,
        batch_dir: &Path,
    ) -> Result<BatchManifest> {
        prepare_policy_from_utxos(
            data_dir,
            config,
            utxos,
            now,
            monthly_limit_sats,
            0,
            batch_dir,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn prepare_policy_from_utxos(
        data_dir: &Path,
        config: &VaultConfig,
        utxos: &[VaultUtxo],
        now: DateTime<Utc>,
        monthly_limit_sats: u64,
        emergency_access_limit_sats: u64,
        batch_dir: &Path,
    ) -> Result<BatchManifest> {
        let phone = load_device_keys(data_dir, PHONE_DEVICE_FILE)?;
        let mut hot = HotWallet::open_or_create(data_dir)?;
        build_policy_proposal(
            config,
            utxos,
            now,
            PolicyLimits {
                monthly_limit_sats,
                emergency_access_limit_sats,
            },
            batch_dir,
            &phone,
            &mut hot,
        )
    }

    fn fake_utxo(config: &VaultConfig, sats: u64) -> VaultUtxo {
        VaultUtxo {
            outpoint: OutPoint::new(Txid::all_zeros(), 0),
            txout: TxOut {
                value: Amount::from_sat(sats),
                script_pubkey: Address::from_str(&config.vault_address)
                    .unwrap()
                    .require_network(config.bitcoin_network().unwrap())
                    .unwrap()
                    .script_pubkey(),
            },
            confirmation_height: 1,
        }
    }

    fn fake_connector_utxo(config: &VaultConfig, txid_byte: u8) -> VaultUtxo {
        let controller = controller_policy(config).unwrap();
        VaultUtxo {
            outpoint: OutPoint::new(Txid::from_byte_array([txid_byte; 32]), 0),
            txout: TxOut {
                value: Amount::from_sat(CONNECTOR_VALUE_SATS),
                script_pubkey: controller.address.script_pubkey(),
            },
            confirmation_height: 1,
        }
    }

    #[test]
    fn two_btc_creates_twelve_step_relative_allowance_chain_and_one_remainder() {
        let dir = tempfile::tempdir().unwrap();
        let initialized = initialize(dir.path()).unwrap();
        let batch = dir.path().join("batch");
        let now = Utc.with_ymd_and_hms(2026, 8, 3, 12, 0, 0).unwrap();
        let manifest = prepare_from_utxos(
            dir.path(),
            &initialized.config,
            &[fake_utxo(&initialized.config, 200_000_000)],
            now,
            10_000_000,
            &batch,
        )
        .unwrap();
        assert_eq!(manifest.allowance_count, 12);
        assert_eq!(manifest.allowances.first().unwrap().step, 1);
        assert_eq!(manifest.allowances.last().unwrap().step, 12);
        let rollover = read_psbt(&batch.join(&manifest.rollover.psbt_file)).unwrap();
        assert_eq!(rollover.unsigned_tx.output.len(), 3);
        assert!(!batch.join("split.psbt").exists());
        assert_eq!(manifest.allowance_vout, Some(0));
        assert_eq!(
            rollover.unsigned_tx.output[0].value.to_sat(),
            manifest.allowance_value_sats
        );

        let delay = monthly_delay_sequence().unwrap();
        let mut expected_outpoint = OutPoint::new(rollover.unsigned_tx.compute_txid(), 0);
        let mut expected_connector = OutPoint::new(
            rollover.unsigned_tx.compute_txid(),
            manifest.monthly_connector_vout.unwrap(),
        );
        let mut expected_value = manifest.allowance_value_sats;
        for (index, allowance) in manifest.allowances.iter().enumerate() {
            let authorization = read_psbt(&batch.join(&allowance.authorization.psbt_file)).unwrap();
            assert_eq!(allowance.step as usize, index + 1);
            assert_eq!(allowance.delay_seconds, MONTHLY_ALLOWANCE_DELAY_SECONDS);
            assert_eq!(allowance.delay_sequence, delay.to_consensus_u32());
            assert_eq!(allowance.chain_value_sats, expected_value);
            assert_eq!(
                authorization.unsigned_tx.lock_time,
                absolute::LockTime::ZERO
            );
            assert_eq!(authorization.unsigned_tx.input[0].sequence, delay);
            assert_eq!(
                authorization.unsigned_tx.input[0].previous_output,
                expected_outpoint
            );
            assert_eq!(authorization.unsigned_tx.input.len(), 2);
            assert_eq!(
                authorization.unsigned_tx.input[1].previous_output,
                expected_connector
            );
            assert!(authorization.inputs[1].tap_script_sigs.is_empty());
            assert_eq!(
                authorization.unsigned_tx.output[0].value.to_sat(),
                10_000_000
            );
            if index + 1 < manifest.allowance_count {
                assert_eq!(authorization.unsigned_tx.output.len(), 3);
                expected_value = authorization.unsigned_tx.output[1].value.to_sat();
                expected_outpoint = OutPoint::new(authorization.unsigned_tx.compute_txid(), 1);
                expected_connector = OutPoint::new(authorization.unsigned_tx.compute_txid(), 2);
                assert_eq!(
                    authorization.unsigned_tx.output[2].value.to_sat(),
                    CONNECTOR_VALUE_SATS
                );
            } else {
                assert_eq!(authorization.unsigned_tx.output.len(), 1);
                assert_eq!(
                    expected_value + CONNECTOR_VALUE_SATS,
                    manifest.monthly_limit_sats + allowance.authorization.fee_sats
                );
            }
        }
        assert_eq!(
            rollover.unsigned_tx.output[manifest.remainder_vout as usize]
                .value
                .to_sat(),
            manifest.remainder_value_sats
        );
        validate_batch(&initialized.config, &manifest, &batch).unwrap();
    }

    #[test]
    fn insufficient_balance_reduces_the_number_of_funded_allowance_steps() {
        let dir = tempfile::tempdir().unwrap();
        let initialized = initialize(dir.path()).unwrap();
        let manifest = prepare_from_utxos(
            dir.path(),
            &initialized.config,
            &[fake_utxo(&initialized.config, 35_000_000)],
            Utc.with_ymd_and_hms(2026, 8, 3, 12, 0, 0).unwrap(),
            10_000_000,
            &dir.path().join("batch"),
        )
        .unwrap();
        assert_eq!(manifest.allowance_count, 3);
        assert_eq!(manifest.allowances.len(), 3);
    }

    #[test]
    fn emergency_access_uses_two_presigned_transactions_and_unsigned_connectors() {
        let dir = tempfile::tempdir().unwrap();
        let initialized = initialize(dir.path()).unwrap();
        let batch = dir.path().join("batch");
        let manifest = prepare_policy_from_utxos(
            dir.path(),
            &initialized.config,
            &[fake_utxo(&initialized.config, 200_000_000)],
            Utc.with_ymd_and_hms(2026, 8, 3, 12, 0, 0).unwrap(),
            10_000_000,
            50_000_000,
            &batch,
        )
        .unwrap();
        assert_eq!(manifest.allowance_count, 12);
        assert_eq!(manifest_transactions(&manifest).len(), 15);
        let emergency = manifest.emergency_access.as_ref().unwrap();
        assert_eq!(emergency.amount_sats, 50_000_000);
        assert_eq!(emergency.delay_seconds, EMERGENCY_ACCESS_DELAY_SECONDS);

        let rollover = read_psbt(&batch.join(&manifest.rollover.psbt_file)).unwrap();
        let trigger = read_psbt(&batch.join(&emergency.trigger.psbt_file)).unwrap();
        assert_eq!(
            trigger.unsigned_tx.input[0].previous_output,
            OutPoint::new(rollover.unsigned_tx.compute_txid(), manifest.remainder_vout)
        );
        assert_eq!(trigger.unsigned_tx.input.len(), 2);
        assert_eq!(
            trigger.unsigned_tx.input[1].previous_output,
            emergency.trigger_connector.outpoint
        );
        assert!(trigger.inputs[1].tap_script_sigs.is_empty());
        assert_eq!(trigger.unsigned_tx.output.len(), 3);
        assert!(trigger.unsigned_tx.output[..2].iter().all(|output| {
            output.script_pubkey
                == Address::from_str(&manifest.vault_address)
                    .unwrap()
                    .require_network(Network::Regtest)
                    .unwrap()
                    .script_pubkey()
        }));

        let staged = OutPoint::new(trigger.unsigned_tx.compute_txid(), 0);
        let withdrawal = read_psbt(&batch.join(&emergency.withdrawal.psbt_file)).unwrap();
        assert_eq!(withdrawal.unsigned_tx.input[0].previous_output, staged);
        assert_eq!(
            withdrawal.unsigned_tx.input[1].previous_output,
            emergency.withdrawal_connector.outpoint
        );
        assert_eq!(
            withdrawal.unsigned_tx.input[0].sequence,
            emergency_delay_sequence().unwrap()
        );
        assert_eq!(withdrawal.unsigned_tx.output[0].value.to_sat(), 50_000_000);
        validate_batch(&initialized.config, &manifest, &batch).unwrap();

        let approved = cold_wallet::approve_policy(dir.path(), &batch).unwrap();
        let phone = load_device_keys(dir.path(), PHONE_DEVICE_FILE).unwrap();
        let controller = controller_policy(&initialized.config).unwrap();
        for transaction in manifest_transactions(&approved) {
            let mut psbt = read_psbt(&batch.join(&transaction.psbt_file)).unwrap();
            if !transaction.controller_input_indexes.is_empty() {
                assert!(finalize_vault_psbt(psbt.clone()).is_err());
                let indexes = transaction
                    .controller_input_indexes
                    .iter()
                    .map(|index| *index as usize)
                    .collect::<Vec<_>>();
                sign_controller_psbt_inputs(
                    &mut psbt,
                    &controller,
                    ControllerPath::Phone,
                    &phone.vault_keypair,
                    &indexes,
                )
                .unwrap();
            }
            assert!(finalize_vault_psbt(psbt).is_ok());
        }
    }

    #[test]
    fn emergency_access_can_be_enabled_without_monthly_spending() {
        let dir = tempfile::tempdir().unwrap();
        let initialized = initialize(dir.path()).unwrap();
        let batch = dir.path().join("batch");
        let manifest = prepare_policy_from_utxos(
            dir.path(),
            &initialized.config,
            &[fake_utxo(&initialized.config, 200_000_000)],
            Utc.with_ymd_and_hms(2026, 8, 3, 12, 0, 0).unwrap(),
            0,
            50_000_000,
            &batch,
        )
        .unwrap();
        assert_eq!(manifest.allowance_count, 0);
        assert_eq!(manifest_transactions(&manifest).len(), 3);
        let emergency = manifest.emergency_access.as_ref().unwrap();
        let trigger = read_psbt(&batch.join(&emergency.trigger.psbt_file)).unwrap();
        assert_eq!(
            trigger.unsigned_tx.input[0].previous_output,
            OutPoint::new(Txid::from_str(&manifest.rollover.unsigned_txid).unwrap(), 0)
        );
        validate_batch(&initialized.config, &manifest, &batch).unwrap();
    }

    #[test]
    fn annual_refresh_consumes_old_connectors_and_creates_only_new_policy_state() {
        let dir = tempfile::tempdir().unwrap();
        let initialized = initialize(dir.path()).unwrap();
        let batch = dir.path().join("batch");
        let phone = load_device_keys(dir.path(), PHONE_DEVICE_FILE).unwrap();
        let mut hot = HotWallet::open_or_create(dir.path()).unwrap();
        let old_connectors = vec![
            fake_connector_utxo(&initialized.config, 1),
            fake_connector_utxo(&initialized.config, 2),
        ];
        let manifest = build_policy_proposal_with_connectors(
            &initialized.config,
            &[fake_utxo(&initialized.config, 200_000_000)],
            &old_connectors,
            Utc.with_ymd_and_hms(2026, 8, 3, 12, 0, 0).unwrap(),
            PolicyLimits {
                monthly_limit_sats: 10_000_000,
                emergency_access_limit_sats: 50_000_000,
            },
            &batch,
            &phone,
            &mut hot,
        )
        .unwrap();

        assert_eq!(manifest.vault_input_sats, 200_000_000);
        assert_eq!(manifest.controller_input_sats, 2 * CONNECTOR_VALUE_SATS);
        assert_eq!(manifest.rollover.vault_input_indexes, [0]);
        assert_eq!(manifest.rollover.controller_input_indexes, [1, 2]);
        let rollover = read_psbt(&batch.join(&manifest.rollover.psbt_file)).unwrap();
        assert_eq!(rollover.unsigned_tx.input.len(), 3);
        assert_eq!(
            rollover.unsigned_tx.input[1].previous_output,
            old_connectors[0].outpoint
        );
        assert_eq!(
            rollover.unsigned_tx.input[2].previous_output,
            old_connectors[1].outpoint
        );
        assert!(
            rollover.inputs[1..]
                .iter()
                .all(|input| input.tap_script_sigs.len() == 1)
        );
        let controller_script = controller_policy(&initialized.config)
            .unwrap()
            .address
            .script_pubkey();
        assert_eq!(
            rollover
                .unsigned_tx
                .output
                .iter()
                .filter(|output| output.script_pubkey == controller_script)
                .count(),
            2
        );

        let approved = cold_wallet::approve_policy(dir.path(), &batch).unwrap();
        assert!(approved.hww_approved);
        assert!(finalize_vault_psbt(read_psbt(&batch.join("rollover.psbt")).unwrap()).is_ok());
    }

    #[test]
    fn emergency_reserve_reduces_allowance_steps_before_reducing_its_amount() {
        let dir = tempfile::tempdir().unwrap();
        let initialized = initialize(dir.path()).unwrap();
        let manifest = prepare_policy_from_utxos(
            dir.path(),
            &initialized.config,
            &[fake_utxo(&initialized.config, 130_000_000)],
            Utc.with_ymd_and_hms(2026, 8, 3, 12, 0, 0).unwrap(),
            10_000_000,
            20_000_000,
            &dir.path().join("batch"),
        )
        .unwrap();
        assert!(manifest.allowance_count < MONTHS_PER_ROLLOVER);
        assert_eq!(
            manifest.emergency_access.as_ref().unwrap().amount_sats,
            20_000_000
        );
    }

    #[test]
    fn policy_rollover_continues_when_no_allowance_step_can_be_funded() {
        let dir = tempfile::tempdir().unwrap();
        let initialized = initialize(dir.path()).unwrap();
        let batch = dir.path().join("batch");
        let manifest = prepare_policy_from_utxos(
            dir.path(),
            &initialized.config,
            &[fake_utxo(&initialized.config, 25_000_000)],
            Utc.with_ymd_and_hms(2026, 8, 3, 12, 0, 0).unwrap(),
            10_000_000,
            20_000_000,
            &batch,
        )
        .unwrap();
        assert_eq!(manifest.monthly_limit_sats, 10_000_000);
        assert_eq!(manifest.allowance_count, 0);
        assert!(manifest.allowances.is_empty());
        assert_eq!(
            manifest.emergency_access.as_ref().unwrap().amount_sats,
            20_000_000
        );
        validate_batch(&initialized.config, &manifest, &batch).unwrap();
    }

    #[test]
    fn hww_rejects_a_tampered_emergency_delay() {
        let dir = tempfile::tempdir().unwrap();
        let initialized = initialize(dir.path()).unwrap();
        let batch = dir.path().join("batch");
        let manifest = prepare_policy_from_utxos(
            dir.path(),
            &initialized.config,
            &[fake_utxo(&initialized.config, 200_000_000)],
            Utc.with_ymd_and_hms(2026, 8, 3, 12, 0, 0).unwrap(),
            10_000_000,
            20_000_000,
            &batch,
        )
        .unwrap();
        let path = batch.join(
            &manifest
                .emergency_access
                .as_ref()
                .unwrap()
                .withdrawal
                .psbt_file,
        );
        let mut psbt = read_psbt(&path).unwrap();
        psbt.unsigned_tx.input[0].sequence = Sequence::from_height(1);
        write_psbt(&path, &psbt).unwrap();
        assert!(cold_wallet::approve_policy(dir.path(), &batch).is_err());
    }

    #[test]
    fn hww_validates_and_signs_the_complete_batch_once() {
        let dir = tempfile::tempdir().unwrap();
        let initialized = initialize(dir.path()).unwrap();
        let batch = dir.path().join("batch");
        prepare_from_utxos(
            dir.path(),
            &initialized.config,
            &[fake_utxo(&initialized.config, 200_000_000)],
            Utc.with_ymd_and_hms(2026, 8, 3, 12, 0, 0).unwrap(),
            10_000_000,
            &batch,
        )
        .unwrap();
        let approved = cold_wallet::approve_policy(dir.path(), &batch).unwrap();
        assert!(approved.phone_approved);
        assert!(approved.hww_approved);
        for transaction in manifest_transactions(&approved) {
            let psbt = read_psbt(&batch.join(&transaction.psbt_file)).unwrap();
            if transaction.psbt_file == approved.rollover.psbt_file {
                assert!(finalize_vault_psbt(psbt).is_ok());
            } else {
                assert!(
                    finalize_vault_psbt(psbt).is_err(),
                    "presigned policy actions must remain incomplete until a device signs their controller input"
                );
            }
        }
    }

    #[test]
    fn hww_rejects_unsupported_sighash_before_signing_any_batch_transaction() {
        let dir = tempfile::tempdir().unwrap();
        let initialized = initialize(dir.path()).unwrap();
        let batch = dir.path().join("batch");
        let manifest = prepare_from_utxos(
            dir.path(),
            &initialized.config,
            &[fake_utxo(&initialized.config, 200_000_000)],
            Utc.with_ymd_and_hms(2026, 8, 3, 12, 0, 0).unwrap(),
            10_000_000,
            &batch,
        )
        .unwrap();
        let path = batch.join(&manifest.allowances.last().unwrap().authorization.psbt_file);
        let mut psbt = read_psbt(&path).unwrap();
        psbt.inputs[0].sighash_type = Some(bitcoin::sighash::TapSighashType::All.into());
        write_psbt(&path, &psbt).unwrap();
        let before = manifest_transactions(&manifest)
            .iter()
            .map(|transaction| {
                let path = batch.join(&transaction.psbt_file);
                (path.clone(), fs::read(path).unwrap())
            })
            .collect::<Vec<_>>();
        let error = cold_wallet::approve_policy(dir.path(), &batch).unwrap_err();
        assert!(error.to_string().contains("SIGHASH_DEFAULT"));
        assert!(!load_manifest(&batch).unwrap().hww_approved);
        for (path, contents) in before {
            assert_eq!(fs::read(path).unwrap(), contents);
        }
    }

    #[test]
    fn hww_rejects_a_tampered_allowance_limit_output() {
        let dir = tempfile::tempdir().unwrap();
        let initialized = initialize(dir.path()).unwrap();
        let batch = dir.path().join("batch");
        let manifest = prepare_from_utxos(
            dir.path(),
            &initialized.config,
            &[fake_utxo(&initialized.config, 200_000_000)],
            Utc.with_ymd_and_hms(2026, 8, 3, 12, 0, 0).unwrap(),
            10_000_000,
            &batch,
        )
        .unwrap();
        let path = batch.join(&manifest.allowances[0].authorization.psbt_file);
        let mut psbt = read_psbt(&path).unwrap();
        psbt.unsigned_tx.output[0].value = Amount::from_sat(10_000_001);
        write_psbt(&path, &psbt).unwrap();
        assert!(cold_wallet::approve_policy(dir.path(), &batch).is_err());
    }

    #[test]
    fn hww_rejects_a_tampered_allowance_relative_delay() {
        let dir = tempfile::tempdir().unwrap();
        let initialized = initialize(dir.path()).unwrap();
        let batch = dir.path().join("batch");
        let manifest = prepare_from_utxos(
            dir.path(),
            &initialized.config,
            &[fake_utxo(&initialized.config, 200_000_000)],
            Utc.with_ymd_and_hms(2026, 8, 3, 12, 0, 0).unwrap(),
            10_000_000,
            &batch,
        )
        .unwrap();
        let path = batch.join(&manifest.allowances[0].authorization.psbt_file);
        let mut psbt = read_psbt(&path).unwrap();
        psbt.unsigned_tx.input[0].sequence = Sequence::from_height(1);
        write_psbt(&path, &psbt).unwrap();
        assert!(cold_wallet::approve_policy(dir.path(), &batch).is_err());
    }

    #[test]
    fn hww_rejects_a_tampered_allowance_chain_output() {
        let dir = tempfile::tempdir().unwrap();
        let initialized = initialize(dir.path()).unwrap();
        let batch = dir.path().join("batch");
        let manifest = prepare_from_utxos(
            dir.path(),
            &initialized.config,
            &[fake_utxo(&initialized.config, 200_000_000)],
            Utc.with_ymd_and_hms(2026, 8, 3, 12, 0, 0).unwrap(),
            10_000_000,
            &batch,
        )
        .unwrap();
        let path = batch.join(&manifest.rollover.psbt_file);
        let mut psbt = read_psbt(&path).unwrap();
        psbt.unsigned_tx.output[0].value += Amount::from_sat(1);
        write_psbt(&path, &psbt).unwrap();
        assert!(cold_wallet::approve_policy(dir.path(), &batch).is_err());
    }

    #[test]
    fn hww_rejects_a_tampered_allowance_connector_outpoint() {
        let dir = tempfile::tempdir().unwrap();
        let initialized = initialize(dir.path()).unwrap();
        let batch = dir.path().join("batch");
        let manifest = prepare_from_utxos(
            dir.path(),
            &initialized.config,
            &[fake_utxo(&initialized.config, 200_000_000)],
            Utc.with_ymd_and_hms(2026, 8, 3, 12, 0, 0).unwrap(),
            10_000_000,
            &batch,
        )
        .unwrap();
        let path = batch.join(&manifest.allowances[0].authorization.psbt_file);
        let mut psbt = read_psbt(&path).unwrap();
        psbt.unsigned_tx.input[1].previous_output = OutPoint::null();
        write_psbt(&path, &psbt).unwrap();
        assert!(cold_wallet::approve_policy(dir.path(), &batch).is_err());
    }

    #[test]
    fn hww_rejects_a_tampered_next_connector_output() {
        let dir = tempfile::tempdir().unwrap();
        let initialized = initialize(dir.path()).unwrap();
        let batch = dir.path().join("batch");
        let manifest = prepare_from_utxos(
            dir.path(),
            &initialized.config,
            &[fake_utxo(&initialized.config, 200_000_000)],
            Utc.with_ymd_and_hms(2026, 8, 3, 12, 0, 0).unwrap(),
            10_000_000,
            &batch,
        )
        .unwrap();
        let path = batch.join(&manifest.allowances[0].authorization.psbt_file);
        let mut psbt = read_psbt(&path).unwrap();
        psbt.unsigned_tx.output[2].value += Amount::from_sat(1);
        write_psbt(&path, &psbt).unwrap();
        assert!(cold_wallet::approve_policy(dir.path(), &batch).is_err());
    }

    #[test]
    fn zero_monthly_limit_creates_only_a_cold_rollover() {
        let dir = tempfile::tempdir().unwrap();
        let initialized = initialize(dir.path()).unwrap();
        let batch = dir.path().join("batch");
        let manifest = prepare_from_utxos(
            dir.path(),
            &initialized.config,
            &[fake_utxo(&initialized.config, 200_000_000)],
            Utc.with_ymd_and_hms(2026, 8, 3, 12, 0, 0).unwrap(),
            0,
            &batch,
        )
        .unwrap();
        assert_eq!(manifest.monthly_limit_sats, 0);
        assert_eq!(manifest.allowance_count, 0);
        assert!(manifest.allowances.is_empty());
        validate_batch(&initialized.config, &manifest, &batch).unwrap();
    }

    #[test]
    fn mainnet_policy_batch_uses_mainnet_manifest_and_hot_addresses() {
        let dir = tempfile::tempdir().unwrap();
        let initialized = initialize_for_network(dir.path(), Network::Bitcoin).unwrap();
        let batch = dir.path().join("mainnet-batch");
        let utxo = fake_utxo(&initialized.config, 200_000_000);
        let manifest = prepare_from_utxos(
            dir.path(),
            &initialized.config,
            &[utxo],
            Utc.with_ymd_and_hms(2026, 8, 4, 0, 0, 0).unwrap(),
            10_000_000,
            &batch,
        )
        .unwrap();
        assert_eq!(manifest.network, "mainnet");
        assert!(
            manifest
                .allowances
                .iter()
                .all(|allowance| allowance.hot_address.starts_with("bc1p"))
        );
        let approved = cold_wallet::approve_policy(dir.path(), &batch).unwrap();
        assert!(approved.hww_approved);
    }

    #[test]
    fn policy_package_round_trips_all_psbts() {
        let dir = tempfile::tempdir().unwrap();
        let initialized = initialize(dir.path()).unwrap();
        let batch = dir.path().join("batch");
        prepare_from_utxos(
            dir.path(),
            &initialized.config,
            &[fake_utxo(&initialized.config, 200_000_000)],
            Utc.with_ymd_and_hms(2026, 8, 3, 12, 0, 0).unwrap(),
            10_000_000,
            &batch,
        )
        .unwrap();
        let package = package_from_batch(&batch).unwrap();
        assert_eq!(package.kind, POLICY_PACKAGE_KIND);
        let imported = dir.path().join("imported");
        materialize_policy_package(&package, &imported).unwrap();
        validate_batch(&initialized.config, &package.manifest, &imported).unwrap();
        assert_eq!(package.version, 5);
        assert_eq!(package.psbts.len(), 13);

        let mut legacy = package.clone();
        legacy.version = 3;
        let legacy_imported = dir.path().join("legacy-imported");
        assert!(materialize_policy_package(&legacy, &legacy_imported).is_err());
    }
}
