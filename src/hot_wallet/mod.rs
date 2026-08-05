//! Phone/mobile-wallet implementation.
//!
//! Mobile applications can build on this module without importing hardware-wallet behavior.

mod rotation;
mod wallet;

pub use rotation::{activate_phone_rotation, create_phone_rotation};
pub use wallet::HotWallet;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MonthlyBroadcastResult {
    pub split_txid: Option<Txid>,
    pub split_was_broadcast: bool,
    pub transaction_txid: Txid,
}

use crate::core::{
    ceremony::{
        self, BatchManifest, EncryptedSplitTransaction, EncryptedTransaction, HotAddressProvider,
        PolicyPackage, SCHEDULE_FILE, Schedule, ScheduleEntry, TransactionKind,
    },
    chain::{Blockchain, ElectrumBackend, RegtestRpc},
    recovery::{self, CooperativeSweepPackage, PhoneRecoveryPackage, SweepPath, SweepResult},
};
use anyhow::{Context, Result, bail};
use bitcoin::{Address, Network, OutPoint, Psbt, Transaction, Txid, consensus, key::Secp256k1};
use chrono::{DateTime, Utc};
use std::{fs, path::Path, str::FromStr};

use crate::core::{
    keys::DeviceKeys,
    storage::{
        DeviceFile, InitializedDevice, PHONE_DEVICE_FILE, VaultConfig, load_config,
        load_device_keys, network_name, read_json, validate_supported_network, write_json,
    },
    transactions::finalize_vault_psbt,
};

/// Initialize the phone key material and its BDK wallet state.
pub fn initialize(data_dir: &Path, network: Network) -> Result<InitializedDevice> {
    validate_supported_network(network)?;
    let phone_path = data_dir.join(PHONE_DEVICE_FILE);
    if phone_path.exists() {
        anyhow::bail!("phone already initialized at {}", phone_path.display());
    }
    let phone = DeviceKeys::generate_for_network(&Secp256k1::new(), network)?;
    let mnemonic = phone.mnemonic.to_string();
    write_json(
        &phone_path,
        &DeviceFile {
            kind: "phone".to_owned(),
            network: network_name(network).to_owned(),
            mnemonic: mnemonic.clone(),
        },
    )?;
    HotWallet::open_or_create(data_dir)?;
    Ok(InitializedDevice {
        mnemonic,
        vault_pubkey: phone.vault_pubkey.to_string(),
    })
}

pub fn propose_policy(
    data_dir: &Path,
    backend: &dyn HotWalletBackend,
    now: DateTime<Utc>,
    monthly_limit_sats: u64,
    batch_dir: &Path,
) -> Result<BatchManifest> {
    let config = load_config(data_dir)?;
    ensure_backend_network(backend, &config)?;
    let utxos = backend.scan_vault(&config)?;
    let phone = load_device_keys(data_dir, PHONE_DEVICE_FILE)?;
    let mut wallet = HotWallet::open_or_create(data_dir)?;
    ceremony::build_policy_proposal(
        &config,
        &utxos,
        now,
        monthly_limit_sats,
        batch_dir,
        &phone,
        &mut wallet,
    )
}

pub fn activate_policy(
    data_dir: &Path,
    backend: &dyn HotWalletBackend,
    batch_dir: &Path,
) -> Result<Schedule> {
    let config = load_config(data_dir)?;
    ensure_backend_network(backend, &config)?;
    let manifest = ceremony::load_manifest(batch_dir)?;
    if !manifest.phone_approved || !manifest.hww_approved {
        bail!("both phone and HWW approval are required before finalization");
    }
    ceremony::validate_batch(&config, &manifest, batch_dir)?;
    let phone = load_device_keys(data_dir, PHONE_DEVICE_FILE)?;

    let rollover = finalize_vault_psbt(read_psbt(&batch_dir.join(&manifest.rollover.psbt_file))?)?;
    let (split_file, split_txid) = match &manifest.split {
        Some(split) => {
            let transaction = finalize_vault_psbt(read_psbt(&batch_dir.join(&split.psbt_file))?)?;
            let txid = transaction.compute_txid().to_string();
            let path = data_dir.join(format!("phone/transactions/split-{txid}.json"));
            write_encrypted_split(&path, &phone.seed, &transaction)?;
            (Some(relative_to(data_dir, &path)?), Some(txid))
        }
        None => (None, None),
    };
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
        version: 2,
        rollover_txid: rollover.compute_txid().to_string(),
        split_file,
        split_txid,
        monthly_limit_sats: manifest.monthly_limit_sats,
        entries,
    };
    write_json(&data_dir.join(SCHEDULE_FILE), &schedule)?;
    backend
        .broadcast(&rollover)
        .context("failed to broadcast rollover transaction")?;
    Ok(schedule)
}

pub fn broadcast_monthly(
    data_dir: &Path,
    backend: &dyn HotWalletBackend,
    month: &str,
    kind: TransactionKind,
) -> Result<MonthlyBroadcastResult> {
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
    let config = load_config(data_dir)?;
    ensure_backend_network(backend, &config)?;
    let phone = load_device_keys(data_dir, PHONE_DEVICE_FILE)?;
    let (split_txid, split_was_broadcast) =
        broadcast_split_if_needed(data_dir, backend, &schedule, &phone.seed)?;
    let artifact: EncryptedTransaction = read_json(&data_dir.join(file))?;
    if artifact.month != month || artifact.kind != kind {
        bail!("encrypted monthly transaction metadata does not match the requested action");
    }
    let purpose = transaction_purpose(month, kind, &artifact.txid);
    let plaintext =
        crate::core::crypto::decrypt(&phone.seed, &purpose, &artifact.encrypted_transaction)?;
    let transaction: Transaction =
        consensus::deserialize(&plaintext).context("decrypted monthly transaction was invalid")?;
    if transaction.compute_txid().to_string() != artifact.txid {
        bail!("decrypted monthly transaction ID does not match its metadata");
    }
    // TODO(production): revocations need a phone-available CPFP path. The fixed 1 sat/vB MVP fee
    // is deterministic on regtest and explicitly unsafe under dangerously enabled mainnet mode.
    let transaction_txid = backend.broadcast(&transaction).with_context(|| {
        let split_note = split_txid
            .filter(|_| split_was_broadcast)
            .map(|txid| format!("deferred split {txid} was broadcast; "))
            .unwrap_or_default();
        format!("{split_note}failed to broadcast {kind:?} for {month}")
    })?;
    Ok(MonthlyBroadcastResult {
        split_txid,
        split_was_broadcast,
        transaction_txid,
    })
}

fn broadcast_split_if_needed(
    data_dir: &Path,
    backend: &dyn HotWalletBackend,
    schedule: &Schedule,
    phone_seed: &[u8],
) -> Result<(Option<Txid>, bool)> {
    let (Some(file), Some(expected_txid)) = (&schedule.split_file, &schedule.split_txid) else {
        return Ok((None, false));
    };
    let artifact: EncryptedSplitTransaction = read_json(&data_dir.join(file))?;
    if artifact.version != 1 || artifact.txid != *expected_txid {
        bail!("encrypted split transaction metadata does not match the active schedule");
    }
    let purpose = split_purpose(&artifact.txid);
    let plaintext =
        crate::core::crypto::decrypt(phone_seed, &purpose, &artifact.encrypted_transaction)?;
    let transaction: Transaction =
        consensus::deserialize(&plaintext).context("decrypted split transaction was invalid")?;
    let txid = transaction.compute_txid();
    if txid.to_string() != artifact.txid {
        bail!("decrypted split transaction ID does not match its metadata");
    }
    match backend.broadcast(&transaction) {
        Ok(broadcast_txid) if broadcast_txid == txid => Ok((Some(txid), true)),
        Ok(_) => bail!("chain backend returned an unexpected split transaction ID"),
        Err(error) if duplicate_broadcast_error(&error) => Ok((Some(txid), false)),
        Err(error) => Err(error).context("failed to broadcast deferred monthly split transaction"),
    }
}

fn duplicate_broadcast_error(error: &anyhow::Error) -> bool {
    let message = format!("{error:#}").to_ascii_lowercase();
    [
        "txn-already-in-mempool",
        "already in block chain",
        "already known",
        "transaction already exists",
        "transaction outputs already in utxo set",
    ]
    .iter()
    .any(|needle| message.contains(needle))
}

pub fn load_schedule(data_dir: &Path) -> Result<Schedule> {
    read_json(&data_dir.join(SCHEDULE_FILE))
}

pub fn apply_soft_limit(
    data_dir: &Path,
    backend: &dyn HotWalletBackend,
    month: &str,
    soft_limit_sats: u64,
) -> Result<Option<Txid>> {
    let schedule = load_schedule(data_dir)?;
    let config = load_config(data_dir)?;
    ensure_backend_network(backend, &config)?;
    let entry = schedule
        .entries
        .iter()
        .find(|entry| entry.month == month)
        .with_context(|| format!("no monthly authorization exists for {month}"))?;
    let authorization_txid = entry.authorization_txid.parse()?;
    let mut wallet = HotWallet::open_or_create(data_dir)?;
    backend.sync_hot_wallet(&mut wallet)?;
    let transaction = wallet.build_soft_limit_return(
        OutPoint::new(authorization_txid, 0),
        schedule.monthly_limit_sats,
        soft_limit_sats,
        config
            .vault_address
            .parse::<Address<_>>()?
            .require_network(config.bitcoin_network()?)?
            .script_pubkey(),
    )?;
    transaction
        .map(|transaction| {
            backend
                .broadcast(&transaction)
                .context("failed to broadcast soft-limit cold-return transaction")
        })
        .transpose()
}

pub fn restore_phone(data_dir: &Path, package: &PhoneRecoveryPackage) -> Result<String> {
    if package.version != 2 || package.kind != "phone-recovery" {
        bail!("unsupported phone recovery package");
    }
    let phone_path = data_dir.join(PHONE_DEVICE_FILE);
    if phone_path.exists() {
        bail!(
            "phone key still exists at {}; refusing to overwrite it",
            phone_path.display()
        );
    }
    let config = load_config(data_dir)?;
    let phone = DeviceKeys::parse_for_network(
        &Secp256k1::new(),
        &package.phone_mnemonic,
        config.bitcoin_network()?,
    )?;
    if phone.vault_pubkey.to_string() != package.phone_vault_pubkey
        || package.phone_vault_pubkey != config.phone_vault_pubkey
        || package.vault_descriptor != config.vault_descriptor
        || package.vault_address != config.vault_address
    {
        bail!("phone recovery package does not match the configured vault policy");
    }
    write_json(
        &phone_path,
        &DeviceFile {
            kind: "phone".to_owned(),
            network: config.network,
            mnemonic: package.phone_mnemonic.clone(),
        },
    )?;
    Ok(package.phone_mnemonic.clone())
}

pub fn recover(
    data_dir: &Path,
    config: &VaultConfig,
    utxos: &[crate::core::types::VaultUtxo],
    tip_height: u64,
    destination: &Address,
) -> Result<(Transaction, SweepResult)> {
    let plan = recovery::prepare_sweep(
        config,
        utxos,
        tip_height,
        SweepPath::PhoneRecovery,
        destination,
    )?;
    let phone = load_device_keys(data_dir, PHONE_DEVICE_FILE)?;
    recovery::sign_recovery_sweep(plan, SweepPath::PhoneRecovery, &phone)
}

pub fn create_cooperative_sweep(
    data_dir: &Path,
    backend: &dyn HotWalletBackend,
    destination: &Address,
) -> Result<CooperativeSweepPackage> {
    let config = load_config(data_dir)?;
    ensure_backend_network(backend, &config)?;
    let utxos = backend.scan_vault(&config)?;
    let phone = load_device_keys(data_dir, PHONE_DEVICE_FILE)?;
    recovery::create_cooperative_sweep(&config, &utxos, destination, &phone)
}

pub fn broadcast_cooperative_sweep(
    data_dir: &Path,
    backend: &dyn HotWalletBackend,
    package: &CooperativeSweepPackage,
) -> Result<SweepResult> {
    let config = load_config(data_dir)?;
    ensure_backend_network(backend, &config)?;
    broadcast_cooperative_sweep_for_config(backend, &config, package)
}

fn broadcast_cooperative_sweep_for_config(
    backend: &dyn HotWalletBackend,
    config: &VaultConfig,
    package: &CooperativeSweepPackage,
) -> Result<SweepResult> {
    let (transaction, result) = recovery::finalize_cooperative_sweep(config, package)?;
    let txid = backend
        .broadcast(&transaction)
        .context("failed to broadcast cooperative vault sweep")?;
    if txid != result.txid {
        bail!("chain backend returned an unexpected cooperative sweep transaction ID");
    }
    Ok(result)
}

pub fn validate_policy_package(package: &PolicyPackage) -> Result<()> {
    if package.version != 2 || package.kind != "monthly-policy" {
        bail!("unsupported policy package");
    }
    Ok(())
}

/// Chain functionality needed by a mobile wallet in addition to the shared vault operations.
pub trait HotWalletBackend: Blockchain {
    fn sync_hot_wallet(&self, wallet: &mut HotWallet) -> Result<()>;
}

impl HotWalletBackend for RegtestRpc {
    fn sync_hot_wallet(&self, wallet: &mut HotWallet) -> Result<()> {
        wallet.sync_core(&self.client)
    }
}

impl HotWalletBackend for ElectrumBackend {
    fn sync_hot_wallet(&self, wallet: &mut HotWallet) -> Result<()> {
        wallet.sync_electrum(&self.client)
    }
}

impl HotAddressProvider for HotWallet {
    fn next_receive_address(&mut self) -> Result<bitcoin::Address> {
        HotWallet::next_receive_address(self)
    }
}

fn ensure_backend_network<B>(backend: &B, config: &VaultConfig) -> Result<()>
where
    B: Blockchain + ?Sized,
{
    let expected = config.bitcoin_network()?;
    if backend.network() != expected {
        bail!(
            "chain backend network {} does not match vault network {}",
            backend.network(),
            config.network
        );
    }
    Ok(())
}

fn read_psbt(path: &Path) -> Result<Psbt> {
    let text = fs::read_to_string(path)
        .with_context(|| format!("failed to read PSBT {}", path.display()))?;
    Psbt::from_str(text.trim()).with_context(|| format!("invalid PSBT in {}", path.display()))
}

fn encrypted_transaction_path(
    data_dir: &Path,
    month: &str,
    kind: TransactionKind,
) -> std::path::PathBuf {
    data_dir.join(format!(
        "phone/transactions/{month}-{}.json",
        transaction_kind_name(kind)
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
    let encrypted_transaction =
        crate::core::crypto::encrypt(phone_seed, &purpose, &consensus::serialize(transaction))?;
    write_json(
        path,
        &EncryptedTransaction {
            version: 1,
            month: month.to_owned(),
            kind,
            txid,
            unlock_timestamp,
            encrypted_transaction,
        },
    )
}

fn write_encrypted_split(path: &Path, phone_seed: &[u8], transaction: &Transaction) -> Result<()> {
    let txid = transaction.compute_txid().to_string();
    let purpose = split_purpose(&txid);
    let encrypted_transaction =
        crate::core::crypto::encrypt(phone_seed, &purpose, &consensus::serialize(transaction))?;
    write_json(
        path,
        &EncryptedSplitTransaction {
            version: 1,
            txid,
            encrypted_transaction,
        },
    )
}

fn split_purpose(txid: &str) -> String {
    format!("monthly/split/{txid}")
}

fn transaction_purpose(month: &str, kind: TransactionKind, txid: &str) -> String {
    format!("monthly/{month}/{}/{txid}", transaction_kind_name(kind))
}

fn transaction_kind_name(kind: TransactionKind) -> &'static str {
    match kind {
        TransactionKind::Authorization => "authorization",
        TransactionKind::Revocation => "revocation",
    }
}

fn relative_to(base: &Path, path: &Path) -> Result<String> {
    Ok(path
        .strip_prefix(base)
        .with_context(|| format!("{} is outside {}", path.display(), base.display()))?
        .to_string_lossy()
        .into_owned())
}
