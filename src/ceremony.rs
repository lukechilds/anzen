use crate::{
    DEFAULT_FEE_RATE_SAT_VB, MONTHS_PER_ROLLOVER,
    crypto::{self, EncryptedBlob},
    hot::HotWallet,
    keys::DeviceKeys,
    policy::{SpendPath, VaultPolicy},
    rpc::{RegtestRpc, VaultUtxo},
    state::{
        HWW_DEVICE_FILE, PHONE_DEVICE_FILE, VaultConfig, load_config, load_device, read_json,
        write_json, write_private,
    },
    transactions::{
        create_vault_psbt, estimate_vault_vsize, finalize_vault_psbt, sign_vault_psbt,
        verify_vault_psbt_signature,
    },
};
use anyhow::{Context, Result, bail};
use bitcoin::{
    Address, Amount, Network, OutPoint, Psbt, ScriptBuf, Sequence, Transaction, TxIn, TxOut, Txid,
    Witness, absolute, consensus, key::Secp256k1, transaction::Version,
};
use bitcoincore_rpc::RpcApi;
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum TransactionKind {
    Authorization,
    Revocation,
}

impl TransactionKind {
    fn file_stem(self) -> &'static str {
        match self {
            Self::Authorization => "authorization",
            Self::Revocation => "revocation",
        }
    }
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
        version: 1,
        kind: "monthly-policy".to_owned(),
        manifest,
        psbts,
    })
}

pub fn materialize_policy_package(package: &PolicyPackage, batch_dir: &Path) -> Result<()> {
    if package.version != 1 || package.kind != "monthly-policy" {
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

pub fn prepare(
    data_dir: &Path,
    rpc: &RegtestRpc,
    now: DateTime<Utc>,
    monthly_limit_sats: u64,
    batch_dir: &Path,
) -> Result<BatchManifest> {
    let config = load_config(data_dir)?;
    let utxos = rpc.scan_vault(&config)?;
    prepare_from_utxos(
        data_dir,
        &config,
        &utxos,
        now,
        monthly_limit_sats,
        batch_dir,
    )
}

pub fn prepare_from_utxos(
    data_dir: &Path,
    config: &VaultConfig,
    utxos: &[VaultUtxo],
    now: DateTime<Utc>,
    monthly_limit_sats: u64,
    batch_dir: &Path,
) -> Result<BatchManifest> {
    let secp = Secp256k1::new();
    let phone_file = load_device(data_dir, PHONE_DEVICE_FILE)?;
    let phone = DeviceKeys::parse(&secp, &phone_file.mnemonic)?;
    let mut hot = HotWallet::open_or_create(data_dir)?;
    prepare_from_utxos_for_phone(
        config,
        utxos,
        now,
        monthly_limit_sats,
        batch_dir,
        &phone,
        &mut hot,
    )
}

pub(crate) fn prepare_from_utxos_for_phone(
    config: &VaultConfig,
    utxos: &[VaultUtxo],
    now: DateTime<Utc>,
    monthly_limit_sats: u64,
    batch_dir: &Path,
    phone: &DeviceKeys,
    hot: &mut HotWallet,
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
    let policy = VaultPolicy::from_descriptor(&config.vault_descriptor)?;
    let vault_script = policy.address.script_pubkey();
    let total_input_sats = checked_input_sum(utxos)?;
    let input_template = utxos
        .iter()
        .map(|utxo| vault_input(utxo.outpoint, Sequence::MAX))
        .collect::<Vec<_>>();

    let mut selected = None;
    let candidate_counts: Vec<usize> = if monthly_limit_sats == 0 {
        vec![0]
    } else {
        (1..=MONTHS_PER_ROLLOVER).rev().collect()
    };
    for count in candidate_counts {
        let output_count = count.max(1);
        let template = Transaction {
            version: Version::TWO,
            lock_time: absolute::LockTime::ZERO,
            input: input_template.clone(),
            output: (0..output_count)
                .map(|_| TxOut {
                    value: Amount::from_sat(1),
                    script_pubkey: vault_script.clone(),
                })
                .collect(),
        };
        let rollover_fee = estimate_vault_vsize(&template, &policy, SpendPath::Cooperative)?
            * DEFAULT_FEE_RATE_SAT_VB;
        if total_input_sats <= rollover_fee {
            continue;
        }
        let distributable = total_input_sats - rollover_fee;
        if monthly_limit_sats == 0 {
            selected = Some((0, rollover_fee, distributable));
            break;
        }
        let smallest_chunk = distributable / count as u64;
        if smallest_chunk <= monthly_limit_sats {
            continue;
        }
        let child_template = authorization_template(
            OutPoint::null(),
            smallest_chunk,
            monthly_limit_sats,
            500_000_001,
            vault_script.clone(),
            vault_script.clone(),
            0,
        )?;
        let authorization_fee =
            estimate_vault_vsize(&child_template, &policy, SpendPath::Cooperative)?
                * DEFAULT_FEE_RATE_SAT_VB;
        let minimum_remainder = vault_script.minimal_non_dust().to_sat();
        if smallest_chunk
            >= monthly_limit_sats
                .saturating_add(authorization_fee)
                .saturating_add(minimum_remainder)
        {
            selected = Some((count, rollover_fee, distributable));
            break;
        }
    }
    let (chunk_count, rollover_fee, distributable) = selected.context(
        "vault balance cannot fund even one monthly authorization plus fees and cold change",
    )?;

    let output_count = chunk_count.max(1);
    let mut chunk_values = vec![distributable / output_count as u64; output_count];
    for value in chunk_values
        .iter_mut()
        .take((distributable % output_count as u64) as usize)
    {
        *value += 1;
    }
    let rollover_tx = Transaction {
        version: Version::TWO,
        lock_time: absolute::LockTime::ZERO,
        input: input_template,
        output: chunk_values
            .iter()
            .map(|value| TxOut {
                value: Amount::from_sat(*value),
                script_pubkey: vault_script.clone(),
            })
            .collect(),
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

    let month_starts = next_month_starts(now, chunk_count)?;
    let mut months = Vec::with_capacity(chunk_count);
    for (index, ((month, unlock_timestamp), chunk_value)) in month_starts
        .into_iter()
        .zip(chunk_values.iter().copied())
        .enumerate()
    {
        let hot_address = hot.next_receive_address()?;
        let chunk_outpoint = OutPoint::new(rollover_txid, index as u32);
        let authorization_fee = authorization_fee(
            &policy,
            chunk_outpoint,
            chunk_value,
            monthly_limit_sats,
            unlock_timestamp,
            hot_address.script_pubkey(),
            vault_script.clone(),
        )?;
        let authorization_tx = authorization_template(
            chunk_outpoint,
            chunk_value,
            monthly_limit_sats,
            unlock_timestamp,
            hot_address.script_pubkey(),
            vault_script.clone(),
            authorization_fee,
        )?;
        let mut authorization_psbt = create_vault_psbt(
            authorization_tx.clone(),
            std::slice::from_ref(&rollover_tx.output[index]),
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
            std::slice::from_ref(&rollover_tx.output[index]),
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
        version: 1,
        created_at: now.timestamp(),
        network: "regtest".to_owned(),
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

pub fn approve_hww(data_dir: &Path, batch_dir: &Path) -> Result<BatchManifest> {
    let config = load_config(data_dir)?;
    approve_hww_for_config(data_dir, &config, batch_dir)
}

pub(crate) fn approve_hww_for_config(
    data_dir: &Path,
    config: &VaultConfig,
    batch_dir: &Path,
) -> Result<BatchManifest> {
    let mut manifest = load_manifest(batch_dir)?;
    let policy = validate_batch(config, &manifest, batch_dir)?;
    let secp = Secp256k1::new();
    let phone_pubkey = bitcoin::secp256k1::XOnlyPublicKey::from_str(&config.phone_vault_pubkey)?;
    let hww_file = load_device(data_dir, HWW_DEVICE_FILE)?;
    let hww = DeviceKeys::parse(&secp, &hww_file.mnemonic)?;
    if hww.vault_pubkey.to_string() != config.hww_vault_pubkey {
        bail!("HWW key does not match the configured vault policy");
    }

    for transaction in manifest_transactions(&manifest) {
        let path = batch_dir.join(&transaction.psbt_file);
        let mut psbt = read_psbt(&path)?;
        verify_vault_psbt_signature(&psbt, &policy, SpendPath::Cooperative, phone_pubkey)?;
        sign_vault_psbt(
            &mut psbt,
            &policy,
            SpendPath::Cooperative,
            &hww.vault_keypair,
        )?;
        write_psbt(&path, &psbt)?;
    }
    manifest.hww_approved = true;
    write_json(&batch_dir.join("manifest.json"), &manifest)?;
    Ok(manifest)
}

pub fn finalize_and_broadcast(
    data_dir: &Path,
    rpc: &RegtestRpc,
    batch_dir: &Path,
) -> Result<Schedule> {
    let config = load_config(data_dir)?;
    let manifest = load_manifest(batch_dir)?;
    if !manifest.phone_approved || !manifest.hww_approved {
        bail!("both phone and HWW approval are required before finalization");
    }
    let _policy = validate_batch(&config, &manifest, batch_dir)?;
    let phone_file = load_device(data_dir, PHONE_DEVICE_FILE)?;
    let phone = DeviceKeys::parse(&Secp256k1::new(), &phone_file.mnemonic)?;

    let rollover_psbt = read_psbt(&batch_dir.join(&manifest.rollover.psbt_file))?;
    let rollover_tx = finalize_vault_psbt(rollover_psbt)?;
    let mut entries = Vec::with_capacity(manifest.months.len());
    for month in &manifest.months {
        let authorization =
            finalize_vault_psbt(read_psbt(&batch_dir.join(&month.authorization.psbt_file))?)?;
        let revocation =
            finalize_vault_psbt(read_psbt(&batch_dir.join(&month.revocation.psbt_file))?)?;
        let authorization_path =
            encrypted_transaction_path(data_dir, &month.month, TransactionKind::Authorization);
        let revocation_path =
            encrypted_transaction_path(data_dir, &month.month, TransactionKind::Revocation);
        write_encrypted_transaction(
            &authorization_path,
            &phone.seed,
            &month.month,
            TransactionKind::Authorization,
            Some(month.unlock_timestamp),
            &authorization,
        )?;
        write_encrypted_transaction(
            &revocation_path,
            &phone.seed,
            &month.month,
            TransactionKind::Revocation,
            None,
            &revocation,
        )?;
        entries.push(ScheduleEntry {
            month: month.month.clone(),
            unlock_timestamp: month.unlock_timestamp,
            hot_address: month.hot_address.clone(),
            authorization_file: relative_to(data_dir, &authorization_path)?,
            authorization_txid: authorization.compute_txid().to_string(),
            revocation_file: relative_to(data_dir, &revocation_path)?,
            revocation_txid: revocation.compute_txid().to_string(),
        });
    }
    let schedule = Schedule {
        version: 1,
        rollover_txid: rollover_tx.compute_txid().to_string(),
        monthly_limit_sats: manifest.monthly_limit_sats,
        entries,
    };
    write_json(&data_dir.join(SCHEDULE_FILE), &schedule)?;
    rpc.client
        .send_raw_transaction(&rollover_tx)
        .context("failed to broadcast rollover transaction")?;
    Ok(schedule)
}

pub fn load_schedule(data_dir: &Path) -> Result<Schedule> {
    read_json(&data_dir.join(SCHEDULE_FILE))
}

pub fn broadcast_monthly(
    data_dir: &Path,
    rpc: &RegtestRpc,
    month: &str,
    kind: TransactionKind,
) -> Result<Txid> {
    let schedule = load_schedule(data_dir)?;
    let entry = schedule
        .entries
        .iter()
        .find(|entry| entry.month == month)
        .with_context(|| format!("no monthly authorization exists for {month}"))?;
    let file = match kind {
        TransactionKind::Authorization => &entry.authorization_file,
        TransactionKind::Revocation => &entry.revocation_file,
    };
    let phone_file = load_device(data_dir, PHONE_DEVICE_FILE)?;
    let phone = DeviceKeys::parse(&Secp256k1::new(), &phone_file.mnemonic)?;
    let artifact: EncryptedTransaction = read_json(&data_dir.join(file))?;
    if artifact.month != month || artifact.kind != kind {
        bail!("encrypted monthly transaction metadata does not match the requested action");
    }
    let purpose = transaction_purpose(month, kind, &artifact.txid);
    let plaintext = crypto::decrypt(&phone.seed, &purpose, &artifact.encrypted_transaction)?;
    let transaction: Transaction =
        consensus::deserialize(&plaintext).context("decrypted monthly transaction was invalid")?;
    if transaction.compute_txid().to_string() != artifact.txid {
        bail!("decrypted monthly transaction ID does not match its metadata");
    }
    // TODO(production): revocations need a phone-available CPFP path; the MVP relies on 1 sat/vB
    // and deterministic regtest mining as agreed in the design.
    rpc.client
        .send_raw_transaction(&transaction)
        .with_context(|| format!("failed to broadcast {kind:?} for {month}"))
}

pub fn apply_soft_limit(
    data_dir: &Path,
    rpc: &RegtestRpc,
    month: &str,
    soft_limit_sats: u64,
) -> Result<Option<Txid>> {
    let schedule = load_schedule(data_dir)?;
    let config = load_config(data_dir)?;
    let entry = schedule
        .entries
        .iter()
        .find(|entry| entry.month == month)
        .with_context(|| format!("no monthly authorization exists for {month}"))?;
    let authorization_txid = Txid::from_str(&entry.authorization_txid)?;
    let mut hot = HotWallet::open_or_create(data_dir)?;
    hot.sync(&rpc.client)?;
    let transaction = hot.build_soft_limit_return(
        OutPoint::new(authorization_txid, 0),
        schedule.monthly_limit_sats,
        soft_limit_sats,
        Address::from_str(&config.vault_address)?
            .require_network(Network::Regtest)?
            .script_pubkey(),
    )?;
    match transaction {
        Some(transaction) => Ok(Some(
            rpc.client
                .send_raw_transaction(&transaction)
                .context("failed to broadcast soft-limit cold-return transaction")?,
        )),
        None => Ok(None),
    }
}

pub fn validate_batch(
    config: &VaultConfig,
    manifest: &BatchManifest,
    batch_dir: &Path,
) -> Result<VaultPolicy> {
    if manifest.version != 1 || manifest.network != "regtest" {
        bail!("unsupported or non-regtest ceremony manifest");
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
    let policy = VaultPolicy::from_descriptor(&config.vault_descriptor)?;
    let vault_script = policy.address.script_pubkey();
    let rollover = read_psbt(&batch_dir.join(&manifest.rollover.psbt_file))?;
    let rollover_tx = &rollover.unsigned_tx;
    if rollover_tx.compute_txid().to_string() != manifest.rollover.unsigned_txid
        || rollover_tx.output.len() != manifest.chunk_count.max(1)
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
    let rollover_output = output_sum(rollover_tx);
    if rollover_input != manifest.total_input_sats
        || rollover_input.saturating_sub(rollover_output) != manifest.rollover.fee_sats
    {
        bail!("rollover amount or fee does not match its manifest");
    }
    let min_chunk = rollover_tx
        .output
        .iter()
        .map(|output| output.value.to_sat())
        .min()
        .context("rollover has no chunks")?;
    let max_chunk = rollover_tx
        .output
        .iter()
        .map(|output| output.value.to_sat())
        .max()
        .context("rollover has no chunks")?;
    if max_chunk - min_chunk > 1 {
        bail!("rollover chunks are not equal within one satoshi");
    }

    for (index, month) in manifest.months.iter().enumerate() {
        if month.chunk_vout != index as u32
            || month.chunk_value_sats != rollover_tx.output[index].value.to_sat()
        {
            bail!("month {} does not match its rollover chunk", month.month);
        }
        let expected_outpoint = OutPoint::new(rollover_tx.compute_txid(), index as u32);
        let hot_script = Address::from_str(&month.hot_address)?
            .require_network(Network::Regtest)?
            .script_pubkey();
        let authorization = read_psbt(&batch_dir.join(&month.authorization.psbt_file))?;
        validate_child_common(
            &authorization,
            &rollover_tx.output[index],
            expected_outpoint,
            &month.authorization,
        )?;
        let auth_tx = &authorization.unsigned_tx;
        if auth_tx.lock_time.to_consensus_u32() != month.unlock_timestamp
            || auth_tx.input[0].sequence != Sequence::ENABLE_LOCKTIME_NO_RBF
            || auth_tx.output.len() != 2
            || auth_tx.output[0].value.to_sat() != manifest.monthly_limit_sats
            || auth_tx.output[0].script_pubkey != hot_script
            || auth_tx.output[1].script_pubkey != vault_script
        {
            bail!(
                "monthly authorization {} violates the approved policy",
                month.month
            );
        }
        let revocation = read_psbt(&batch_dir.join(&month.revocation.psbt_file))?;
        validate_child_common(
            &revocation,
            &rollover_tx.output[index],
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
        bail!("monthly PSBT does not spend its assigned rollover chunk");
    }
    let fee = expected_prevout
        .value
        .to_sat()
        .checked_sub(output_sum(&psbt.unsigned_tx))
        .context("monthly transaction outputs exceed its input")?;
    if fee != manifest_tx.fee_sats {
        bail!("monthly transaction fee does not match its manifest");
    }
    Ok(())
}

fn authorization_fee(
    policy: &VaultPolicy,
    outpoint: OutPoint,
    chunk_value: u64,
    monthly_limit: u64,
    unlock_timestamp: u32,
    hot_script: ScriptBuf,
    vault_script: ScriptBuf,
) -> Result<u64> {
    let template = authorization_template(
        outpoint,
        chunk_value,
        monthly_limit,
        unlock_timestamp,
        hot_script,
        vault_script,
        0,
    )?;
    Ok(estimate_vault_vsize(&template, policy, SpendPath::Cooperative)? * DEFAULT_FEE_RATE_SAT_VB)
}

fn authorization_template(
    outpoint: OutPoint,
    chunk_value: u64,
    monthly_limit: u64,
    unlock_timestamp: u32,
    hot_script: ScriptBuf,
    vault_script: ScriptBuf,
    fee: u64,
) -> Result<Transaction> {
    let remainder = chunk_value
        .checked_sub(monthly_limit)
        .and_then(|value| value.checked_sub(fee))
        .context("chunk cannot fund the monthly limit and authorization fee")?;
    Ok(Transaction {
        version: Version::TWO,
        lock_time: absolute::LockTime::from_time(unlock_timestamp)
            .map_err(|_| anyhow::anyhow!("monthly unlock timestamp is below 500,000,000"))?,
        input: vec![vault_input(outpoint, Sequence::ENABLE_LOCKTIME_NO_RBF)],
        output: vec![
            TxOut {
                value: Amount::from_sat(monthly_limit),
                script_pubkey: hot_script,
            },
            TxOut {
                value: Amount::from_sat(remainder),
                script_pubkey: vault_script,
            },
        ],
    })
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

fn output_sum(transaction: &Transaction) -> u64 {
    transaction
        .output
        .iter()
        .map(|output| output.value.to_sat())
        .sum()
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

fn write_psbt(path: &Path, psbt: &Psbt) -> Result<()> {
    write_private(path, format!("{psbt}\n").as_bytes())
}

fn read_psbt(path: &Path) -> Result<Psbt> {
    let text = fs::read_to_string(path)
        .with_context(|| format!("failed to read PSBT {}", path.display()))?;
    Psbt::from_str(text.trim()).with_context(|| format!("invalid PSBT in {}", path.display()))
}

fn manifest_transactions(manifest: &BatchManifest) -> Vec<&BatchTransaction> {
    let mut transactions = Vec::with_capacity(1 + manifest.months.len() * 2);
    transactions.push(&manifest.rollover);
    for month in &manifest.months {
        transactions.push(&month.authorization);
        transactions.push(&month.revocation);
    }
    transactions
}

fn encrypted_transaction_path(data_dir: &Path, month: &str, kind: TransactionKind) -> PathBuf {
    data_dir.join(format!(
        "phone/transactions/{month}-{}.json",
        kind.file_stem()
    ))
}

fn write_encrypted_transaction(
    path: &Path,
    phone_seed: &[u8],
    month: &str,
    kind: TransactionKind,
    unlock_timestamp: Option<u32>,
    transaction: &Transaction,
) -> Result<()> {
    let txid = transaction.compute_txid().to_string();
    let purpose = transaction_purpose(month, kind, &txid);
    let blob = crypto::encrypt(phone_seed, &purpose, &consensus::serialize(transaction))?;
    write_json(
        path,
        &EncryptedTransaction {
            version: 1,
            month: month.to_owned(),
            kind,
            txid,
            unlock_timestamp,
            encrypted_transaction: blob,
        },
    )
}

fn transaction_purpose(month: &str, kind: TransactionKind, txid: &str) -> String {
    format!("monthly/{month}/{}/{txid}", kind.file_stem())
}

fn relative_to(base: &Path, path: &Path) -> Result<String> {
    Ok(path
        .strip_prefix(base)
        .with_context(|| format!("{} is outside {}", path.display(), base.display()))?
        .to_string_lossy()
        .into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::initialize;
    use bitcoin::hashes::Hash;

    fn fake_utxo(config: &VaultConfig, sats: u64) -> VaultUtxo {
        VaultUtxo {
            outpoint: OutPoint::new(Txid::all_zeros(), 0),
            txout: TxOut {
                value: Amount::from_sat(sats),
                script_pubkey: Address::from_str(&config.vault_address)
                    .unwrap()
                    .require_network(Network::Regtest)
                    .unwrap()
                    .script_pubkey(),
            },
            confirmation_height: 1,
        }
    }

    #[test]
    fn two_btc_creates_twelve_equal_consecutive_months() {
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
        let min = manifest
            .months
            .iter()
            .map(|month| month.chunk_value_sats)
            .min()
            .unwrap();
        let max = manifest
            .months
            .iter()
            .map(|month| month.chunk_value_sats)
            .max()
            .unwrap();
        assert!(max - min <= 1);
        assert!(manifest.months.iter().all(|month| {
            month.chunk_value_sats > manifest.monthly_limit_sats
                && batch.join(&month.authorization.psbt_file).exists()
                && batch.join(&month.revocation.psbt_file).exists()
        }));
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
        let approved = approve_hww(dir.path(), &batch).unwrap();
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
        assert!(approve_hww(dir.path(), &batch).is_err());
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
        assert_eq!(package.psbts.len(), 25);
    }
}
