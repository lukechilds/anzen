use super::{
    DEFAULT_FEE_RATE_SAT_VB, MONTHS_PER_ROLLOVER,
    crypto::EncryptedBlob,
    keys::DeviceKeys,
    policy::{SpendPath, VaultPolicy},
    storage::{VaultConfig, read_json, write_json, write_private},
    transactions::{create_vault_psbt, estimate_vault_vsize, sign_vault_psbt},
    types::VaultUtxo,
};
use anyhow::{Context, Result, bail};
use bitcoin::{
    Address, Amount, OutPoint, Psbt, ScriptBuf, Sequence, Transaction, TxIn, TxOut, Witness,
    absolute, transaction::Version,
};
use chrono::{DateTime, Datelike, TimeZone, Utc};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Component, Path, PathBuf},
    str::FromStr,
};

pub const DEFAULT_BATCH_DIR: &str = "ceremony/active";
pub const SCHEDULE_FILE: &str = "phone/schedule.json";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BatchTransaction {
    pub psbt_file: String,
    pub unsigned_txid: String,
    pub fee_sats: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MonthPair {
    pub month: String,
    pub unlock_timestamp: u32,
    pub chunk_vout: u32,
    pub chunk_value_sats: u64,
    pub hot_address: String,
    pub authorization: BatchTransaction,
    pub revocation: BatchTransaction,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BatchManifest {
    pub version: u8,
    pub created_at: i64,
    pub network: String,
    pub vault_descriptor: String,
    pub vault_address: String,
    #[serde(alias = "hard_limit_sats")]
    pub monthly_limit_sats: u64,
    pub fee_rate_sat_vb: u64,
    pub total_input_sats: u64,
    pub chunk_count: usize,
    pub rollover: BatchTransaction,
    pub split: Option<BatchTransaction>,
    pub remainder_vout: Option<u32>,
    pub remainder_value_sats: u64,
    pub months: Vec<MonthPair>,
    pub phone_approved: bool,
    pub hww_approved: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EncryptedTransaction {
    pub version: u8,
    pub month: String,
    pub kind: TransactionKind,
    pub txid: String,
    pub unlock_timestamp: Option<u32>,
    pub encrypted_transaction: EncryptedBlob,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EncryptedSplitTransaction {
    pub version: u8,
    pub txid: String,
    pub encrypted_transaction: EncryptedBlob,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum TransactionKind {
    Authorization,
    Revocation,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScheduleEntry {
    pub month: String,
    pub unlock_timestamp: u32,
    pub hot_address: String,
    pub authorization_file: String,
    pub authorization_txid: String,
    pub revocation_file: String,
    pub revocation_txid: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Schedule {
    pub version: u8,
    pub rollover_txid: String,
    pub split_file: Option<String>,
    pub split_txid: Option<String>,
    pub monthly_limit_sats: u64,
    pub entries: Vec<ScheduleEntry>,
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
        version: 2,
        kind: "monthly-policy".to_owned(),
        manifest,
        psbts,
    })
}

pub fn materialize_policy_package(package: &PolicyPackage, batch_dir: &Path) -> Result<()> {
    if package.version != 2 || package.kind != "monthly-policy" {
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
    monthly_limit_sats: u64,
    batch_dir: &Path,
    phone: &DeviceKeys,
    hot: &mut impl HotAddressProvider,
) -> Result<BatchManifest> {
    if utxos.is_empty() {
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
    let vault_script = policy.address.script_pubkey();
    let total_input_sats = checked_input_sum(utxos)?;
    let input_template = utxos
        .iter()
        .map(|utxo| vault_input(utxo.outpoint, Sequence::MAX))
        .collect::<Vec<_>>();

    // The rollover always consolidates the live vault into one output. Monthly chunks are created
    // by a separately presigned split transaction that remains off chain until the first monthly
    // authorization or revocation is attempted.
    let rollover_template = Transaction {
        version: Version::TWO,
        lock_time: absolute::LockTime::ZERO,
        input: input_template.clone(),
        output: vec![TxOut {
            value: Amount::from_sat(1),
            script_pubkey: vault_script.clone(),
        }],
    };
    let rollover_fee = estimate_vault_vsize(&rollover_template, &policy, SpendPath::Cooperative)?
        * DEFAULT_FEE_RATE_SAT_VB;
    let rollover_value = total_input_sats
        .checked_sub(rollover_fee)
        .context("vault balance cannot pay the rollover fee")?;
    if rollover_value < vault_script.minimal_non_dust().to_sat() {
        bail!("vault balance cannot create a non-dust rollover output");
    }

    let rollover_tx = Transaction {
        version: Version::TWO,
        lock_time: absolute::LockTime::ZERO,
        input: input_template,
        output: vec![TxOut {
            value: Amount::from_sat(rollover_value),
            script_pubkey: vault_script.clone(),
        }],
    };
    let rollover_txid = rollover_tx.compute_txid();
    let prevouts = utxos
        .iter()
        .map(|utxo| utxo.txout.clone())
        .collect::<Vec<_>>();
    let mut rollover_psbt = create_vault_psbt(rollover_tx.clone(), &prevouts, &policy)?;
    sign_vault_psbt(
        &mut rollover_psbt,
        &policy,
        SpendPath::Cooperative,
        &phone.vault_keypair,
    )?;
    let rollover_file = "rollover.psbt".to_owned();
    write_psbt(&batch_dir.join(&rollover_file), &rollover_psbt)?;

    let (chunk_count, split, split_tx, chunk_value, remainder_value_sats) =
        if monthly_limit_sats == 0 {
            (0, None, None, 0, 0)
        } else {
            let authorization_fee = authorization_fee(
                &policy,
                OutPoint::null(),
                monthly_limit_sats,
                500_000_001,
                vault_script.clone(),
            )?;
            let chunk_value = monthly_limit_sats
                .checked_add(authorization_fee)
                .context("monthly limit plus fee overflowed")?;
            let minimum_remainder = vault_script.minimal_non_dust().to_sat();
            let mut selected = None;
            for count in (1..=MONTHS_PER_ROLLOVER).rev() {
                let template = split_template(
                    OutPoint::new(rollover_txid, 0),
                    count,
                    chunk_value,
                    1,
                    vault_script.clone(),
                );
                let split_fee = estimate_vault_vsize(&template, &policy, SpendPath::Cooperative)?
                    * DEFAULT_FEE_RATE_SAT_VB;
                let required = split_fee
                    .checked_add(
                        chunk_value
                            .checked_mul(count as u64)
                            .context("monthly chunk total overflowed")?,
                    )
                    .and_then(|value| value.checked_add(minimum_remainder))
                    .context("monthly split requirement overflowed")?;
                if rollover_value >= required {
                    let remainder = rollover_value - split_fee - chunk_value * count as u64;
                    selected = Some((count, split_fee, remainder));
                    break;
                }
            }
            let (count, split_fee, remainder) = selected.context(
                "vault balance cannot fund even one exact monthly chunk, split fee, and remainder",
            )?;
            let split_tx = split_template(
                OutPoint::new(rollover_txid, 0),
                count,
                chunk_value,
                remainder,
                vault_script.clone(),
            );
            let mut split_psbt = create_vault_psbt(
                split_tx.clone(),
                std::slice::from_ref(&rollover_tx.output[0]),
                &policy,
            )?;
            sign_vault_psbt(
                &mut split_psbt,
                &policy,
                SpendPath::Cooperative,
                &phone.vault_keypair,
            )?;
            let split_file = "split.psbt".to_owned();
            write_psbt(&batch_dir.join(&split_file), &split_psbt)?;
            (
                count,
                Some(BatchTransaction {
                    psbt_file: split_file,
                    unsigned_txid: split_tx.compute_txid().to_string(),
                    fee_sats: split_fee,
                }),
                Some(split_tx),
                chunk_value,
                remainder,
            )
        };

    let month_starts = next_month_starts(now, chunk_count)?;
    let mut months = Vec::with_capacity(chunk_count);
    for (index, (month, unlock_timestamp)) in month_starts.into_iter().enumerate() {
        let hot_address = hot.next_receive_address()?;
        let split_tx = split_tx
            .as_ref()
            .context("active monthly policy is missing its split transaction")?;
        let chunk_outpoint = OutPoint::new(split_tx.compute_txid(), index as u32);
        let authorization_fee = authorization_fee(
            &policy,
            chunk_outpoint,
            monthly_limit_sats,
            unlock_timestamp,
            hot_address.script_pubkey(),
        )?;
        if chunk_value != monthly_limit_sats + authorization_fee {
            bail!("monthly authorization fee changed across equivalent output scripts");
        }
        let authorization_tx = authorization_template(
            chunk_outpoint,
            monthly_limit_sats,
            unlock_timestamp,
            hot_address.script_pubkey(),
        )?;
        let mut authorization_psbt = create_vault_psbt(
            authorization_tx.clone(),
            std::slice::from_ref(&split_tx.output[index]),
            &policy,
        )?;
        sign_vault_psbt(
            &mut authorization_psbt,
            &policy,
            SpendPath::Cooperative,
            &phone.vault_keypair,
        )?;

        let revocation_fee =
            revocation_fee(&policy, chunk_outpoint, chunk_value, vault_script.clone())?;
        let revocation_tx = revocation_template(
            chunk_outpoint,
            chunk_value,
            vault_script.clone(),
            revocation_fee,
        )?;
        let mut revocation_psbt = create_vault_psbt(
            revocation_tx.clone(),
            std::slice::from_ref(&split_tx.output[index]),
            &policy,
        )?;
        sign_vault_psbt(
            &mut revocation_psbt,
            &policy,
            SpendPath::Cooperative,
            &phone.vault_keypair,
        )?;

        let month_dir = format!("months/{month}");
        let authorization_file = format!("{month_dir}/authorization.psbt");
        let revocation_file = format!("{month_dir}/revocation.psbt");
        write_psbt(&batch_dir.join(&authorization_file), &authorization_psbt)?;
        write_psbt(&batch_dir.join(&revocation_file), &revocation_psbt)?;
        months.push(MonthPair {
            month,
            unlock_timestamp,
            chunk_vout: index as u32,
            chunk_value_sats: chunk_value,
            hot_address: hot_address.to_string(),
            authorization: BatchTransaction {
                psbt_file: authorization_file,
                unsigned_txid: authorization_tx.compute_txid().to_string(),
                fee_sats: authorization_fee,
            },
            revocation: BatchTransaction {
                psbt_file: revocation_file,
                unsigned_txid: revocation_tx.compute_txid().to_string(),
                fee_sats: revocation_fee,
            },
        });
    }

    let manifest = BatchManifest {
        version: 2,
        created_at: now.timestamp(),
        network: config.network.clone(),
        vault_descriptor: config.vault_descriptor.clone(),
        vault_address: config.vault_address.clone(),
        monthly_limit_sats,
        fee_rate_sat_vb: DEFAULT_FEE_RATE_SAT_VB,
        total_input_sats,
        chunk_count,
        rollover: BatchTransaction {
            psbt_file: rollover_file,
            unsigned_txid: rollover_txid.to_string(),
            fee_sats: rollover_fee,
        },
        split,
        remainder_vout: (chunk_count > 0).then_some(chunk_count as u32),
        remainder_value_sats,
        months,
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
    if manifest.version != 2 || manifest.network != config.network {
        bail!("unsupported ceremony manifest or network mismatch");
    }
    if manifest.vault_descriptor != config.vault_descriptor
        || manifest.vault_address != config.vault_address
    {
        bail!("ceremony policy does not match configured vault policy");
    }
    let disabled = manifest.monthly_limit_sats == 0;
    if (disabled && (manifest.chunk_count != 0 || !manifest.months.is_empty()))
        || (!disabled
            && (manifest.chunk_count == 0
                || manifest.chunk_count > MONTHS_PER_ROLLOVER
                || manifest.months.len() != manifest.chunk_count))
    {
        bail!("invalid ceremony chunk count");
    }
    if manifest.fee_rate_sat_vb != DEFAULT_FEE_RATE_SAT_VB {
        bail!("MVP ceremony must use the fixed 1 sat/vB fee rate");
    }
    let policy = VaultPolicy::from_descriptor_for_network(
        &config.vault_descriptor,
        config.bitcoin_network()?,
    )?;
    let vault_script = policy.address.script_pubkey();
    let rollover = read_psbt(&batch_dir.join(&manifest.rollover.psbt_file))?;
    let rollover_tx = &rollover.unsigned_tx;
    if rollover_tx.compute_txid().to_string() != manifest.rollover.unsigned_txid
        || rollover_tx.output.len() != 1
        || rollover_tx.input.len() != rollover.inputs.len()
    {
        bail!("rollover PSBT does not match its manifest");
    }
    if rollover_tx
        .output
        .iter()
        .any(|output| output.script_pubkey != vault_script)
    {
        bail!("rollover contains an output outside the static vault policy");
    }
    let rollover_input = psbt_input_sum(&rollover)?;
    let rollover_output = output_sum(rollover_tx)?;
    if rollover_input != manifest.total_input_sats
        || rollover_input.saturating_sub(rollover_output) != manifest.rollover.fee_sats
    {
        bail!("rollover amount or fee does not match its manifest");
    }
    let expected_rollover_fee = estimate_vault_vsize(rollover_tx, &policy, SpendPath::Cooperative)?
        * DEFAULT_FEE_RATE_SAT_VB;
    if manifest.rollover.fee_sats != expected_rollover_fee {
        bail!("rollover does not pay the approved fixed fee rate");
    }

    let split = match (&manifest.split, disabled) {
        (None, true) => {
            if manifest.remainder_vout.is_some() || manifest.remainder_value_sats != 0 {
                bail!("disabled monthly policy contains split remainder metadata");
            }
            None
        }
        (Some(_), true) => bail!("disabled monthly policy must not contain a split transaction"),
        (None, false) => bail!("active monthly policy is missing its split transaction"),
        (Some(split_manifest), false) => {
            let split = read_psbt(&batch_dir.join(&split_manifest.psbt_file))?;
            let split_tx = &split.unsigned_tx;
            let expected_outpoint = OutPoint::new(rollover_tx.compute_txid(), 0);
            if split_tx.compute_txid().to_string() != split_manifest.unsigned_txid
                || split_tx.input.len() != 1
                || split.inputs.len() != 1
                || split_tx.input[0].previous_output != expected_outpoint
                || split_tx.input[0].sequence != Sequence::MAX
                || split.inputs[0].witness_utxo.as_ref() != Some(&rollover_tx.output[0])
                || split_tx.output.len() != manifest.chunk_count + 1
                || split_tx
                    .output
                    .iter()
                    .any(|output| output.script_pubkey != vault_script)
            {
                bail!("split PSBT does not create the approved monthly outputs");
            }
            let fee = rollover_tx.output[0]
                .value
                .to_sat()
                .checked_sub(output_sum(split_tx)?)
                .context("split outputs exceed its rollover input")?;
            let expected_fee = estimate_vault_vsize(split_tx, &policy, SpendPath::Cooperative)?
                * DEFAULT_FEE_RATE_SAT_VB;
            if fee != split_manifest.fee_sats || fee != expected_fee {
                bail!("split transaction does not pay the approved fixed fee rate");
            }
            if manifest.remainder_vout != Some(manifest.chunk_count as u32)
                || split_tx.output[manifest.chunk_count].value.to_sat()
                    != manifest.remainder_value_sats
                || manifest.remainder_value_sats < vault_script.minimal_non_dust().to_sat()
            {
                bail!("split remainder does not match its manifest");
            }
            Some(split)
        }
    };

    for (index, month) in manifest.months.iter().enumerate() {
        let split = split
            .as_ref()
            .context("monthly entry is missing its split transaction")?;
        let split_tx = &split.unsigned_tx;
        if month.chunk_vout != index as u32
            || month.chunk_value_sats != split_tx.output[index].value.to_sat()
        {
            bail!("month {} does not match its split chunk", month.month);
        }
        let expected_outpoint = OutPoint::new(split_tx.compute_txid(), index as u32);
        let hot_script = Address::from_str(&month.hot_address)?
            .require_network(config.bitcoin_network()?)?
            .script_pubkey();
        let authorization = read_psbt(&batch_dir.join(&month.authorization.psbt_file))?;
        validate_child_common(
            &authorization,
            &split_tx.output[index],
            expected_outpoint,
            &month.authorization,
        )?;
        let auth_tx = &authorization.unsigned_tx;
        if auth_tx.lock_time.to_consensus_u32() != month.unlock_timestamp
            || auth_tx.input[0].sequence != Sequence::ENABLE_LOCKTIME_NO_RBF
            || auth_tx.output.len() != 1
            || auth_tx.output[0].value.to_sat() != manifest.monthly_limit_sats
            || auth_tx.output[0].script_pubkey != hot_script
            || Some(month.chunk_value_sats)
                != manifest
                    .monthly_limit_sats
                    .checked_add(month.authorization.fee_sats)
        {
            bail!(
                "monthly authorization {} violates the approved policy",
                month.month
            );
        }
        let revocation = read_psbt(&batch_dir.join(&month.revocation.psbt_file))?;
        validate_child_common(
            &revocation,
            &split_tx.output[index],
            expected_outpoint,
            &month.revocation,
        )?;
        let revoke_tx = &revocation.unsigned_tx;
        if revoke_tx.lock_time != absolute::LockTime::ZERO
            || revoke_tx.input[0].sequence != Sequence::MAX
            || revoke_tx.output.len() != 1
            || revoke_tx.output[0].script_pubkey != vault_script
        {
            bail!(
                "monthly revocation {} violates the approved policy",
                month.month
            );
        }
        let expected_authorization_fee =
            estimate_vault_vsize(auth_tx, &policy, SpendPath::Cooperative)?
                * DEFAULT_FEE_RATE_SAT_VB;
        let expected_revocation_fee =
            estimate_vault_vsize(revoke_tx, &policy, SpendPath::Cooperative)?
                * DEFAULT_FEE_RATE_SAT_VB;
        if month.authorization.fee_sats != expected_authorization_fee
            || month.revocation.fee_sats != expected_revocation_fee
        {
            bail!("monthly transaction does not pay the approved fixed fee rate");
        }
    }
    Ok(policy)
}

fn validate_child_common(
    psbt: &Psbt,
    expected_prevout: &TxOut,
    expected_outpoint: OutPoint,
    manifest_tx: &BatchTransaction,
) -> Result<()> {
    if psbt.unsigned_tx.compute_txid().to_string() != manifest_tx.unsigned_txid
        || psbt.unsigned_tx.input.len() != 1
        || psbt.inputs.len() != 1
        || psbt.unsigned_tx.input[0].previous_output != expected_outpoint
        || psbt.inputs[0].witness_utxo.as_ref() != Some(expected_prevout)
    {
        bail!("monthly PSBT does not spend its assigned split chunk");
    }
    let fee = expected_prevout
        .value
        .to_sat()
        .checked_sub(output_sum(&psbt.unsigned_tx)?)
        .context("monthly transaction outputs exceed its input")?;
    if fee != manifest_tx.fee_sats {
        bail!("monthly transaction fee does not match its manifest");
    }
    Ok(())
}

fn authorization_fee(
    policy: &VaultPolicy,
    outpoint: OutPoint,
    monthly_limit: u64,
    unlock_timestamp: u32,
    hot_script: ScriptBuf,
) -> Result<u64> {
    let template = authorization_template(outpoint, monthly_limit, unlock_timestamp, hot_script)?;
    Ok(estimate_vault_vsize(&template, policy, SpendPath::Cooperative)? * DEFAULT_FEE_RATE_SAT_VB)
}

fn authorization_template(
    outpoint: OutPoint,
    monthly_limit: u64,
    unlock_timestamp: u32,
    hot_script: ScriptBuf,
) -> Result<Transaction> {
    Ok(Transaction {
        version: Version::TWO,
        lock_time: absolute::LockTime::from_time(unlock_timestamp)
            .map_err(|_| anyhow::anyhow!("monthly unlock timestamp is below 500,000,000"))?,
        input: vec![vault_input(outpoint, Sequence::ENABLE_LOCKTIME_NO_RBF)],
        output: vec![TxOut {
            value: Amount::from_sat(monthly_limit),
            script_pubkey: hot_script,
        }],
    })
}

fn split_template(
    rollover_outpoint: OutPoint,
    chunk_count: usize,
    chunk_value: u64,
    remainder_value: u64,
    vault_script: ScriptBuf,
) -> Transaction {
    let mut outputs = (0..chunk_count)
        .map(|_| TxOut {
            value: Amount::from_sat(chunk_value),
            script_pubkey: vault_script.clone(),
        })
        .collect::<Vec<_>>();
    outputs.push(TxOut {
        value: Amount::from_sat(remainder_value),
        script_pubkey: vault_script,
    });
    Transaction {
        version: Version::TWO,
        lock_time: absolute::LockTime::ZERO,
        input: vec![vault_input(rollover_outpoint, Sequence::MAX)],
        output: outputs,
    }
}

fn revocation_fee(
    policy: &VaultPolicy,
    outpoint: OutPoint,
    chunk_value: u64,
    vault_script: ScriptBuf,
) -> Result<u64> {
    let template = revocation_template(outpoint, chunk_value, vault_script, 0)?;
    Ok(estimate_vault_vsize(&template, policy, SpendPath::Cooperative)? * DEFAULT_FEE_RATE_SAT_VB)
}

fn revocation_template(
    outpoint: OutPoint,
    chunk_value: u64,
    vault_script: ScriptBuf,
    fee: u64,
) -> Result<Transaction> {
    let value = chunk_value
        .checked_sub(fee)
        .context("chunk cannot fund the revocation fee")?;
    Ok(Transaction {
        version: Version::TWO,
        lock_time: absolute::LockTime::ZERO,
        input: vec![vault_input(outpoint, Sequence::MAX)],
        output: vec![TxOut {
            value: Amount::from_sat(value),
            script_pubkey: vault_script,
        }],
    })
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

fn psbt_input_sum(psbt: &Psbt) -> Result<u64> {
    psbt.inputs.iter().try_fold(0_u64, |sum, input| {
        let value = input
            .witness_utxo
            .as_ref()
            .context("vault PSBT input lacks witness UTXO")?
            .value
            .to_sat();
        sum.checked_add(value).context("PSBT input sum overflowed")
    })
}

fn output_sum(transaction: &Transaction) -> Result<u64> {
    transaction.output.iter().try_fold(0_u64, |sum, output| {
        sum.checked_add(output.value.to_sat())
            .context("transaction output sum overflowed")
    })
}

fn next_month_starts(now: DateTime<Utc>, count: usize) -> Result<Vec<(String, u32)>> {
    let mut year = now.year();
    let mut month = now.month();
    let mut result = Vec::with_capacity(count);
    for _ in 0..count {
        if month == 12 {
            year += 1;
            month = 1;
        } else {
            month += 1;
        }
        let date = Utc
            .with_ymd_and_hms(year, month, 1, 0, 0, 0)
            .single()
            .context("invalid calendar month")?;
        result.push((
            format!("{year:04}-{month:02}"),
            u32::try_from(date.timestamp()).context("monthly timestamp exceeds u32")?,
        ));
    }
    Ok(result)
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
    let mut transactions = Vec::with_capacity(2 + manifest.months.len() * 2);
    transactions.push(&manifest.rollover);
    if let Some(split) = &manifest.split {
        transactions.push(split);
    }
    for month in &manifest.months {
        transactions.push(&month.authorization);
        transactions.push(&month.revocation);
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

    fn prepare_from_utxos(
        data_dir: &Path,
        config: &VaultConfig,
        utxos: &[VaultUtxo],
        now: DateTime<Utc>,
        monthly_limit_sats: u64,
        batch_dir: &Path,
    ) -> Result<BatchManifest> {
        let phone = load_device_keys(data_dir, PHONE_DEVICE_FILE)?;
        let mut hot = HotWallet::open_or_create(data_dir)?;
        build_policy_proposal(
            config,
            utxos,
            now,
            monthly_limit_sats,
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

    #[test]
    fn two_btc_creates_twelve_exact_consecutive_months_and_one_remainder() {
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
        assert_eq!(manifest.chunk_count, 12);
        assert_eq!(manifest.months.first().unwrap().month, "2026-09");
        assert_eq!(manifest.months.last().unwrap().month, "2027-08");
        let rollover = read_psbt(&batch.join(&manifest.rollover.psbt_file)).unwrap();
        assert_eq!(rollover.unsigned_tx.output.len(), 1);
        let split = read_psbt(&batch.join(&manifest.split.as_ref().unwrap().psbt_file)).unwrap();
        assert_eq!(split.unsigned_tx.output.len(), 13);
        assert!(manifest.months.iter().all(|month| {
            month.chunk_value_sats == manifest.monthly_limit_sats + month.authorization.fee_sats
                && batch.join(&month.authorization.psbt_file).exists()
                && batch.join(&month.revocation.psbt_file).exists()
        }));
        assert_eq!(
            split.unsigned_tx.output[12].value.to_sat(),
            manifest.remainder_value_sats
        );
        validate_batch(&initialized.config, &manifest, &batch).unwrap();
    }

    #[test]
    fn insufficient_balance_reduces_the_number_of_funded_months() {
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
        assert_eq!(manifest.chunk_count, 3);
        assert_eq!(manifest.months.len(), 3);
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
            assert!(
                finalize_vault_psbt(read_psbt(&batch.join(&transaction.psbt_file)).unwrap())
                    .is_ok()
            );
        }
    }

    #[test]
    fn hww_rejects_a_tampered_monthly_limit_output() {
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
        let path = batch.join(&manifest.months[0].authorization.psbt_file);
        let mut psbt = read_psbt(&path).unwrap();
        psbt.unsigned_tx.output[0].value = Amount::from_sat(10_000_001);
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
        assert_eq!(manifest.chunk_count, 0);
        assert!(manifest.months.is_empty());
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
                .months
                .iter()
                .all(|month| month.hot_address.starts_with("bc1p"))
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
        let imported = dir.path().join("imported");
        materialize_policy_package(&package, &imported).unwrap();
        validate_batch(&initialized.config, &package.manifest, &imported).unwrap();
        assert_eq!(package.psbts.len(), 26);
    }
}
