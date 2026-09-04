//! Phone/mobile-wallet implementation.
//!
//! Mobile applications can build on this module without importing hardware-wallet behavior.

mod rotation;
mod wallet;

pub use rotation::{
    VanityPhoneRotation, activate_phone_rotation, create_phone_rotation,
    create_vanity_phone_rotation,
};
pub use wallet::HotWallet;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MonthlyBroadcastResult {
    pub transaction_txid: Txid,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EmergencyBroadcastResult {
    pub transaction_txid: Txid,
}

use crate::core::{
    ceremony::{
        self, BatchManifest, EmergencyAccessSchedule, EmergencyTransactionKind,
        EncryptedEmergencyTransaction, EncryptedTransaction, HotAddressProvider, PolicyLimits,
        PolicyPackage, SCHEDULE_FILE, Schedule, ScheduleEntry, TransactionKind,
    },
    chain::{BitcoinCoreBackend, Blockchain, ElectrumBackend},
    policy::{ControllerPath, ControllerPolicy, VaultAddressTemplate},
    recovery::{self, CooperativeSweepPackage, PhoneRecoveryPackage, SweepPath, SweepResult},
};
use anyhow::{Context, Result, bail};
use bitcoin::{Address, Amount, Network, OutPoint, Psbt, Transaction, TxOut, Txid, key::Secp256k1};
use chrono::{DateTime, Utc};
use std::{
    fs,
    path::Path,
    str::FromStr,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc,
    },
    thread,
};

use crate::core::{
    DEFAULT_FEE_RATE_SAT_VB,
    keys::DeviceKeys,
    storage::{
        DeviceFile, HWW_DEVICE_FILE, HWW_PUBLIC_FILE, InitializedDevice, PHONE_DEVICE_FILE,
        VaultConfig, load_config, load_device_keys, load_public_device, network_name, read_json,
        validate_supported_network, write_json,
    },
    transactions::{
        build_controller_revocation_psbt, finalize_vault_psbt, sign_controller_psbt_inputs,
    },
    types::VaultUtxo,
};

/// Initialize the phone key material and its BDK wallet state.
pub fn initialize(data_dir: &Path, network: Network) -> Result<InitializedDevice> {
    validate_supported_network(network)?;
    let phone = DeviceKeys::generate_for_network(&Secp256k1::new(), network)?;
    persist_phone(data_dir, network, &phone)
}

pub const VANITY_SUFFIX: &str = "vault";

#[derive(Debug)]
pub struct VanityInitialization {
    pub device: InitializedDevice,
    pub vault_address: String,
    pub attempts: u64,
    pub worker_count: usize,
}

#[derive(Debug)]
struct VanityPhoneKey {
    phone: DeviceKeys,
    vault_address: String,
    attempts: u64,
    worker_count: usize,
}

/// Grind the phone vault-key derivation index across all available CPU threads.
pub fn initialize_vanity<F>(
    data_dir: &Path,
    network: Network,
    report_progress: F,
) -> Result<VanityInitialization>
where
    F: Fn(u64) + Sync,
{
    let worker_count = available_worker_count();
    initialize_vanity_with_suffix(
        data_dir,
        network,
        VANITY_SUFFIX,
        worker_count,
        report_progress,
    )
}

pub fn vanity_address_prefix(network: Network) -> Result<&'static str> {
    match network {
        Network::Bitcoin => Ok("bc1pvault"),
        Network::Regtest => Ok("bcrt1pvault"),
        other => bail!("vanity initialization is unsupported on {other}"),
    }
}

fn initialize_vanity_with_suffix<F>(
    data_dir: &Path,
    network: Network,
    suffix: &str,
    worker_count: usize,
    report_progress: F,
) -> Result<VanityInitialization>
where
    F: Fn(u64) + Sync,
{
    validate_supported_network(network)?;
    ensure_phone_is_uninitialized(data_dir)?;
    if worker_count == 0 {
        bail!("vanity search requires at least one worker thread");
    }
    if !data_dir.join(HWW_DEVICE_FILE).exists() {
        bail!("initialize the HWW before using phone init --vanity");
    }
    let public_hww = load_public_device(data_dir, HWW_PUBLIC_FILE)
        .context("initialize the HWW before using phone init --vanity")?;
    if public_hww.version != 1 || public_hww.kind != "hww-public-key" {
        bail!("unsupported HWW public metadata");
    }
    if public_hww.bitcoin_network()? != network {
        bail!("phone and HWW must use the same network");
    }
    let hww_pubkey = public_hww.parsed_vault_pubkey()?;
    let vanity =
        grind_vanity_phone_key(network, hww_pubkey, suffix, worker_count, report_progress)?;
    let device = persist_phone(data_dir, network, &vanity.phone)?;
    Ok(VanityInitialization {
        device,
        vault_address: vanity.vault_address,
        attempts: vanity.attempts,
        worker_count: vanity.worker_count,
    })
}

fn available_worker_count() -> usize {
    thread::available_parallelism()
        .map(std::num::NonZeroUsize::get)
        .unwrap_or(1)
}

fn grind_vanity_phone_key<F>(
    network: Network,
    hww_pubkey: bitcoin::secp256k1::XOnlyPublicKey,
    suffix: &str,
    worker_count: usize,
    report_progress: F,
) -> Result<VanityPhoneKey>
where
    F: Fn(u64) + Sync,
{
    validate_supported_network(network)?;
    if worker_count == 0 {
        bail!("vanity search requires at least one worker thread");
    }
    let target = VanityTarget::new(suffix)?;
    let base_phone = DeviceKeys::generate_for_network(&Secp256k1::new(), network)?;
    let parent = base_phone.vault_parent_xpriv(&Secp256k1::new())?;
    let template = Arc::new(VaultAddressTemplate::new(hww_pubkey)?);
    let stopped = Arc::new(AtomicBool::new(false));
    let attempts = Arc::new(AtomicU64::new(0));
    let last_report = Arc::new(AtomicU64::new(0));
    let (sender, receiver) = mpsc::channel::<Result<u32>>();

    let winning_index = thread::scope(|scope| -> Result<u32> {
        for worker in 0..worker_count {
            let sender = sender.clone();
            let template = Arc::clone(&template);
            let stopped = Arc::clone(&stopped);
            let attempts = Arc::clone(&attempts);
            let last_report = Arc::clone(&last_report);
            let report_progress = &report_progress;
            scope.spawn(move || {
                let result = (|| -> Result<()> {
                    let secp = Secp256k1::new();
                    let mut index = worker as u64;
                    let stride = worker_count as u64;
                    let mut pending_attempts = 0_u64;
                    while index < (1_u64 << 31) && !stopped.load(Ordering::Relaxed) {
                        let (_, _, phone_pubkey) =
                            DeviceKeys::derive_vault_key_from_parent(&secp, &parent, index as u32)?;
                        pending_attempts += 1;
                        let output_key = template.output_key(&secp, phone_pubkey);
                        if target.matches(&output_key.to_x_only_public_key().serialize()) {
                            attempts.fetch_add(pending_attempts, Ordering::Relaxed);
                            if !stopped.swap(true, Ordering::Relaxed) {
                                let _ = sender.send(Ok(index as u32));
                            }
                            return Ok(());
                        }
                        if pending_attempts == 4_096 {
                            let total = attempts.fetch_add(pending_attempts, Ordering::Relaxed)
                                + pending_attempts;
                            pending_attempts = 0;
                            maybe_report_progress(total, &last_report, report_progress);
                        }
                        index += stride;
                    }
                    if pending_attempts != 0 {
                        attempts.fetch_add(pending_attempts, Ordering::Relaxed);
                    }
                    Ok(())
                })();
                if let Err(error) = result {
                    if !stopped.swap(true, Ordering::Relaxed) {
                        let _ = sender.send(Err(error));
                    }
                }
            });
        }
        drop(sender);
        receiver
            .recv()
            .context("vanity search ended without finding a matching key")?
    })?;

    let phone = base_phone.with_vault_key_index(&Secp256k1::new(), winning_index)?;
    let vault_address =
        template.address(&Secp256k1::verification_only(), phone.vault_pubkey, network);
    let expected_prefix = match network {
        Network::Bitcoin => format!("bc1p{suffix}"),
        Network::Regtest => format!("bcrt1p{suffix}"),
        other => bail!("vanity initialization is unsupported on {other}"),
    };
    if !vault_address.to_string().starts_with(&expected_prefix) {
        bail!("vanity search result did not reproduce the requested address prefix");
    }
    Ok(VanityPhoneKey {
        phone,
        vault_address: vault_address.to_string(),
        attempts: attempts.load(Ordering::Relaxed),
        worker_count,
    })
}

fn persist_phone(
    data_dir: &Path,
    network: Network,
    phone: &DeviceKeys,
) -> Result<InitializedDevice> {
    ensure_phone_is_uninitialized(data_dir)?;
    let phone_path = data_dir.join(PHONE_DEVICE_FILE);
    let mnemonic = phone.mnemonic.to_string();
    write_json(
        &phone_path,
        &DeviceFile {
            kind: "phone".to_owned(),
            network: network_name(network).to_owned(),
            mnemonic: mnemonic.clone(),
            vault_key_index: phone.vault_key_index,
        },
    )?;
    HotWallet::open_or_create(data_dir)?;
    Ok(InitializedDevice {
        mnemonic,
        vault_pubkey: phone.vault_pubkey.to_string(),
        vault_key_index: phone.vault_key_index,
    })
}

fn ensure_phone_is_uninitialized(data_dir: &Path) -> Result<()> {
    let phone_path = data_dir.join(PHONE_DEVICE_FILE);
    if phone_path.exists() {
        bail!("phone already initialized at {}", phone_path.display());
    }
    Ok(())
}

fn maybe_report_progress<F>(total: u64, last_report: &AtomicU64, report_progress: &F)
where
    F: Fn(u64),
{
    let previous = last_report.load(Ordering::Relaxed);
    if total.saturating_sub(previous) >= 1_000_000
        && last_report
            .compare_exchange(previous, total, Ordering::Relaxed, Ordering::Relaxed)
            .is_ok()
    {
        report_progress(total);
    }
}

#[derive(Debug, Clone, Copy)]
struct VanityTarget {
    value: u32,
    bit_count: u32,
}

impl VanityTarget {
    fn new(suffix: &str) -> Result<Self> {
        const CHARSET: &str = "qpzry9x8gf2tvdw0s3jn54khce6mua7l";
        if suffix.is_empty() || suffix.len() > 6 {
            bail!("vanity suffix must contain between one and six Bech32 characters");
        }
        let mut value = 0_u32;
        for character in suffix.chars() {
            let digit = CHARSET
                .find(character)
                .with_context(|| format!("{character:?} is not a lowercase Bech32 character"))?;
            value = (value << 5) | digit as u32;
        }
        Ok(Self {
            value,
            bit_count: (suffix.len() * 5) as u32,
        })
    }

    fn matches(self, output_key: &[u8; 32]) -> bool {
        let first_bits =
            u32::from_be_bytes([output_key[0], output_key[1], output_key[2], output_key[3]]);
        first_bits >> (32 - self.bit_count) == self.value
    }
}

pub fn propose_policy(
    data_dir: &Path,
    backend: &dyn HotWalletBackend,
    now: DateTime<Utc>,
    monthly_limit_sats: u64,
    emergency_access_limit_sats: u64,
    batch_dir: &Path,
) -> Result<BatchManifest> {
    let config = load_config(data_dir)?;
    ensure_backend_network(backend, &config)?;
    let utxos = backend.scan_vault(&config)?;
    let connectors = backend.scan_connectors(&config)?;
    let phone = load_device_keys(data_dir, PHONE_DEVICE_FILE)?;
    let mut wallet = HotWallet::open_or_create(data_dir)?;
    ceremony::build_policy_proposal_with_connectors(
        &config,
        &utxos,
        &connectors,
        now,
        PolicyLimits {
            monthly_limit_sats,
            emergency_access_limit_sats,
        },
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
    ceremony::validate_approved_batch(&config, &manifest, batch_dir)?;
    let phone = load_device_keys(data_dir, PHONE_DEVICE_FILE)?;

    let rollover = finalize_vault_psbt(read_psbt(&batch_dir.join(&manifest.rollover.psbt_file))?)?;
    // Stage each epoch independently. A rejected or interrupted rollover must leave the current
    // schedule and its encrypted transactions intact. Retain enough material to retry activation.
    let epoch_dir = data_dir
        .join("phone/transactions")
        .join(rollover.compute_txid().to_string());
    if epoch_dir.join("activated.json").exists() && data_dir.join(SCHEDULE_FILE).exists() {
        let active = load_schedule(data_dir)?;
        if active.rollover_txid != rollover.compute_txid().to_string() {
            bail!("this policy epoch has already been superseded; refusing to reactivate it");
        }
    }
    write_json(
        &epoch_dir.join("approved-policy.json"),
        &ceremony::package_from_batch(batch_dir)?,
    )?;
    write_json(&epoch_dir.join("rollover.json"), &rollover)?;
    let mut entries = Vec::with_capacity(manifest.allowances.len());
    for allowance in &manifest.allowances {
        let authorization = read_psbt(&batch_dir.join(&allowance.authorization.psbt_file))?;
        let authorization_path =
            encrypted_transaction_path(&epoch_dir, allowance.step, TransactionKind::Authorization);
        write_encrypted_transaction(
            &authorization_path,
            &phone.seed,
            allowance.step,
            TransactionKind::Authorization,
            &authorization,
        )?;
        entries.push(ScheduleEntry {
            step: allowance.step,
            hot_address: allowance.hot_address.clone(),
            authorization_file: relative_to(data_dir, &authorization_path)?,
            authorization_txid: authorization.unsigned_tx.compute_txid().to_string(),
            connector: allowance.connector.clone(),
            next_connector: allowance.next_connector.clone(),
        });
    }
    let emergency_access = manifest
        .emergency_access
        .as_ref()
        .map(|emergency| {
            let trigger = read_psbt(&batch_dir.join(&emergency.trigger.psbt_file))?;
            let withdrawal = read_psbt(&batch_dir.join(&emergency.withdrawal.psbt_file))?;
            let trigger_path = emergency_transaction_path(
                &epoch_dir,
                EmergencyTransactionKind::Trigger,
                trigger.unsigned_tx.compute_txid(),
            );
            let withdrawal_path = emergency_transaction_path(
                &epoch_dir,
                EmergencyTransactionKind::Withdrawal,
                withdrawal.unsigned_tx.compute_txid(),
            );
            write_encrypted_emergency_transaction(
                &trigger_path,
                &phone.seed,
                EmergencyTransactionKind::Trigger,
                &trigger,
            )?;
            write_encrypted_emergency_transaction(
                &withdrawal_path,
                &phone.seed,
                EmergencyTransactionKind::Withdrawal,
                &withdrawal,
            )?;
            Ok::<EmergencyAccessSchedule, anyhow::Error>(EmergencyAccessSchedule {
                amount_sats: emergency.amount_sats,
                delay_seconds: emergency.delay_seconds,
                hot_address: emergency.hot_address.clone(),
                trigger_file: relative_to(data_dir, &trigger_path)?,
                trigger_txid: trigger.unsigned_tx.compute_txid().to_string(),
                withdrawal_file: relative_to(data_dir, &withdrawal_path)?,
                withdrawal_txid: withdrawal.unsigned_tx.compute_txid().to_string(),
                trigger_connector: emergency.trigger_connector.clone(),
                withdrawal_connector: emergency.withdrawal_connector.clone(),
            })
        })
        .transpose()?;
    let schedule = Schedule {
        version: 5,
        rollover_txid: rollover.compute_txid().to_string(),
        controller_descriptor: manifest.controller_descriptor.clone(),
        controller_address: manifest.controller_address.clone(),
        connector_value_sats: manifest.connector_value_sats,
        monthly_limit_sats: manifest.monthly_limit_sats,
        monthly_delay_seconds: crate::core::MONTHLY_ALLOWANCE_DELAY_SECONDS,
        emergency_access_limit_sats: manifest.emergency_access_limit_sats,
        entries,
        emergency_access,
    };
    write_json(&epoch_dir.join("schedule.json"), &schedule)?;
    let broadcast_txid = backend
        .broadcast(&rollover)
        .context("failed to broadcast rollover transaction")?;
    if broadcast_txid != rollover.compute_txid() {
        bail!("chain backend returned an unexpected rollover transaction ID");
    }
    write_json(&data_dir.join(SCHEDULE_FILE), &schedule).context(
        "rollover accepted but schedule activation failed; retry the same approved policy",
    )?;
    crate::core::storage::set_policy_limits(
        data_dir,
        manifest.monthly_limit_sats,
        manifest.emergency_access_limit_sats,
    )
    .context("rollover accepted but saving policy limits failed; retry the same approved policy")?;
    write_json(&epoch_dir.join("activated.json"), &true)?;
    Ok(schedule)
}

pub fn initiate_emergency_access(
    data_dir: &Path,
    backend: &dyn HotWalletBackend,
) -> Result<EmergencyBroadcastResult> {
    let schedule = load_schedule(data_dir)?;
    schedule
        .emergency_access
        .as_ref()
        .context("emergency access is disabled for the active vault epoch")?;
    let config = load_config(data_dir)?;
    ensure_backend_network(backend, &config)?;
    let phone = load_device_keys(data_dir, PHONE_DEVICE_FILE)?;
    let transaction_txid = broadcast_emergency_transaction(
        data_dir,
        backend,
        &schedule,
        &config,
        &phone,
        EmergencyTransactionKind::Trigger,
    )?;
    Ok(EmergencyBroadcastResult { transaction_txid })
}

pub fn withdraw_emergency_access(data_dir: &Path, backend: &dyn HotWalletBackend) -> Result<Txid> {
    let schedule = load_schedule(data_dir)?;
    let config = load_config(data_dir)?;
    ensure_backend_network(backend, &config)?;
    let phone = load_device_keys(data_dir, PHONE_DEVICE_FILE)?;
    broadcast_emergency_transaction(
        data_dir,
        backend,
        &schedule,
        &config,
        &phone,
        EmergencyTransactionKind::Withdrawal,
    )
}

pub fn cancel_emergency_access(data_dir: &Path, backend: &dyn HotWalletBackend) -> Result<Txid> {
    let schedule = load_schedule(data_dir)?;
    let config = load_config(data_dir)?;
    ensure_backend_network(backend, &config)?;
    let phone = load_device_keys(data_dir, PHONE_DEVICE_FILE)?;
    let connector = schedule
        .emergency_access
        .as_ref()
        .context("emergency access is disabled for the active vault epoch")?
        .withdrawal_connector
        .clone();
    revoke_connector_to_phone(data_dir, backend, &config, &phone, &connector)
}

fn broadcast_emergency_transaction(
    data_dir: &Path,
    backend: &dyn HotWalletBackend,
    schedule: &Schedule,
    config: &VaultConfig,
    phone: &DeviceKeys,
    kind: EmergencyTransactionKind,
) -> Result<Txid> {
    let emergency = schedule
        .emergency_access
        .as_ref()
        .context("emergency access is disabled for the active vault epoch")?;
    let (file, expected_txid, connector) = match kind {
        EmergencyTransactionKind::Trigger => (
            &emergency.trigger_file,
            &emergency.trigger_txid,
            &emergency.trigger_connector,
        ),
        EmergencyTransactionKind::Withdrawal => (
            &emergency.withdrawal_file,
            &emergency.withdrawal_txid,
            &emergency.withdrawal_connector,
        ),
        EmergencyTransactionKind::Cancellation => bail!(
            "emergency cancellation is constructed dynamically from the live controller output"
        ),
    };
    let artifact: EncryptedEmergencyTransaction = read_json(&data_dir.join(file))?;
    if artifact.version != 2 || artifact.kind != kind || artifact.txid != *expected_txid {
        bail!("encrypted emergency transaction metadata does not match the requested action");
    }
    let purpose = emergency_transaction_purpose(kind, &artifact.txid);
    let plaintext = crate::core::crypto::decrypt(&phone.seed, &purpose, &artifact.encrypted_psbt)?;
    let mut psbt = Psbt::from_str(
        std::str::from_utf8(&plaintext).context("decrypted emergency PSBT was not UTF-8")?,
    )
    .context("decrypted emergency PSBT was invalid")?;
    let txid = psbt.unsigned_tx.compute_txid();
    if txid.to_string() != artifact.txid {
        bail!("decrypted emergency PSBT ID does not match its metadata");
    }
    ensure_connector_input(&psbt, connector)?;
    let controller = controller_policy(config)?;
    validate_schedule_controller(schedule, &controller)?;
    sign_controller_psbt_inputs(
        &mut psbt,
        &controller,
        ControllerPath::Phone,
        &phone.vault_keypair,
        &[1],
    )?;
    let transaction = finalize_vault_psbt(psbt)?;
    let broadcast_txid = backend
        .broadcast(&transaction)
        .with_context(|| format!("failed to broadcast emergency access {kind:?}"))?;
    if broadcast_txid != txid {
        bail!("chain backend returned an unexpected emergency transaction ID");
    }
    Ok(txid)
}

pub fn broadcast_monthly(
    data_dir: &Path,
    backend: &dyn HotWalletBackend,
    step: u8,
    kind: TransactionKind,
) -> Result<MonthlyBroadcastResult> {
    let schedule = load_schedule(data_dir)?;
    let entry = schedule
        .entries
        .iter()
        .find(|entry| entry.step == step)
        .with_context(|| format!("no allowance exists for step {step}"))?;
    let config = load_config(data_dir)?;
    ensure_backend_network(backend, &config)?;
    let phone = load_device_keys(data_dir, PHONE_DEVICE_FILE)?;
    if kind == TransactionKind::Revocation {
        let transaction_txid =
            revoke_connector_to_phone(data_dir, backend, &config, &phone, &entry.connector)?;
        return Ok(MonthlyBroadcastResult { transaction_txid });
    }
    let artifact: EncryptedTransaction = read_json(&data_dir.join(&entry.authorization_file))?;
    if artifact.version != 3
        || artifact.step != step
        || artifact.kind != kind
        || artifact.txid != entry.authorization_txid
    {
        bail!("encrypted allowance transaction metadata does not match the requested action");
    }
    let purpose = transaction_purpose(step, kind, &artifact.txid);
    let plaintext = crate::core::crypto::decrypt(&phone.seed, &purpose, &artifact.encrypted_psbt)?;
    let mut psbt = Psbt::from_str(
        std::str::from_utf8(&plaintext).context("decrypted allowance PSBT was not UTF-8")?,
    )
    .context("decrypted allowance PSBT was invalid")?;
    if psbt.unsigned_tx.compute_txid().to_string() != artifact.txid {
        bail!("decrypted allowance PSBT ID does not match its metadata");
    }
    ensure_connector_input(&psbt, &entry.connector)?;
    let controller = controller_policy(&config)?;
    validate_schedule_controller(&schedule, &controller)?;
    sign_controller_psbt_inputs(
        &mut psbt,
        &controller,
        ControllerPath::Phone,
        &phone.vault_keypair,
        &[1],
    )?;
    let transaction = finalize_vault_psbt(psbt)?;
    let transaction_txid = backend
        .broadcast(&transaction)
        .with_context(|| format!("failed to broadcast {kind:?} for allowance step {step}"))?;
    Ok(MonthlyBroadcastResult { transaction_txid })
}

fn revoke_connector_to_phone(
    data_dir: &Path,
    backend: &dyn HotWalletBackend,
    config: &VaultConfig,
    phone: &DeviceKeys,
    connector: &ceremony::ConnectorState,
) -> Result<Txid> {
    let controller = controller_policy(config)?;
    let mut wallet = HotWallet::open_or_create(data_dir)?;
    let destination = wallet.next_change_address()?.script_pubkey();
    let connector_utxo = VaultUtxo {
        outpoint: connector.outpoint,
        txout: TxOut {
            value: Amount::from_sat(connector.value_sats),
            script_pubkey: controller.address.script_pubkey(),
        },
        // The dynamic transaction can validly spend an unconfirmed connector output. Chain
        // acceptance, rather than this local placeholder, determines whether that state is live.
        confirmation_height: 0,
    };
    // TODO(production): select current-feerate phone inputs and expose RBF/CPFP controls. The MVP
    // intentionally uses its fixed 1 sat/vB fee and pays it from the controller output.
    let (mut psbt, _fee_sats) = build_controller_revocation_psbt(
        &[connector_utxo],
        destination,
        DEFAULT_FEE_RATE_SAT_VB,
        &controller,
    )?;
    sign_controller_psbt_inputs(
        &mut psbt,
        &controller,
        ControllerPath::Phone,
        &phone.vault_keypair,
        &[0],
    )?;
    let transaction = finalize_vault_psbt(psbt)?;
    let txid = transaction.compute_txid();
    let broadcast_txid = backend
        .broadcast(&transaction)
        .context("failed to broadcast dynamic policy revocation")?;
    if broadcast_txid != txid {
        bail!("chain backend returned an unexpected revocation transaction ID");
    }
    Ok(txid)
}

fn ensure_connector_input(psbt: &Psbt, connector: &ceremony::ConnectorState) -> Result<()> {
    if psbt
        .unsigned_tx
        .input
        .get(1)
        .map(|input| input.previous_output)
        != Some(connector.outpoint)
        || psbt
            .inputs
            .get(1)
            .and_then(|input| input.witness_utxo.as_ref())
            .map(|output| output.value.to_sat())
            != Some(connector.value_sats)
    {
        bail!("encrypted policy PSBT does not consume the scheduled controller state");
    }
    Ok(())
}

fn controller_policy(config: &VaultConfig) -> Result<ControllerPolicy> {
    let phone = bitcoin::secp256k1::XOnlyPublicKey::from_str(&config.phone_vault_pubkey)
        .context("invalid configured phone vault key")?;
    let hww = bitcoin::secp256k1::XOnlyPublicKey::from_str(&config.hww_vault_pubkey)
        .context("invalid configured HWW vault key")?;
    ControllerPolicy::new_for_network(phone, hww, config.bitcoin_network()?)
}

fn validate_schedule_controller(schedule: &Schedule, controller: &ControllerPolicy) -> Result<()> {
    if schedule.controller_descriptor != controller.descriptor_string()
        || schedule.controller_address != controller.address.to_string()
        || schedule.connector_value_sats != crate::core::CONNECTOR_VALUE_SATS
    {
        bail!("active schedule controller does not match the configured vault keys");
    }
    Ok(())
}

pub fn load_schedule(data_dir: &Path) -> Result<Schedule> {
    let schedule: Schedule = read_json(&data_dir.join(SCHEDULE_FILE))?;
    if schedule.version != 5 {
        bail!("unsupported active policy version; approve a new vault policy");
    }
    Ok(schedule)
}

pub fn apply_soft_limit(
    data_dir: &Path,
    backend: &dyn HotWalletBackend,
    step: u8,
    soft_limit_sats: u64,
) -> Result<Option<Txid>> {
    let schedule = load_schedule(data_dir)?;
    let config = load_config(data_dir)?;
    ensure_backend_network(backend, &config)?;
    let entry = schedule
        .entries
        .iter()
        .find(|entry| entry.step == step)
        .with_context(|| format!("no allowance exists for step {step}"))?;
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
    let phone = DeviceKeys::parse_for_network_at_index(
        &Secp256k1::new(),
        &package.phone_mnemonic,
        config.bitcoin_network()?,
        package.phone_vault_key_index,
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
            vault_key_index: package.phone_vault_key_index,
        },
    )?;
    HotWallet::open_or_create(data_dir)?.request_full_scan()?;
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
    if !ceremony::is_supported_policy_package(package) {
        bail!("unsupported policy package");
    }
    Ok(())
}

/// Chain functionality needed by a mobile wallet in addition to the shared vault operations.
pub trait HotWalletBackend: Blockchain {
    fn sync_hot_wallet(&self, wallet: &mut HotWallet) -> Result<()>;
}

impl HotWalletBackend for BitcoinCoreBackend {
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
    epoch_dir: &Path,
    step: u8,
    kind: TransactionKind,
) -> std::path::PathBuf {
    epoch_dir.join(format!(
        "allowance-{step:02}-{}.json",
        transaction_kind_name(kind)
    ))
}

fn emergency_transaction_path(
    epoch_dir: &Path,
    kind: EmergencyTransactionKind,
    txid: Txid,
) -> std::path::PathBuf {
    epoch_dir.join(format!(
        "emergency-{}-{txid}.json",
        emergency_transaction_kind_name(kind)
    ))
}

fn write_encrypted_transaction(
    path: &Path,
    phone_seed: &[u8],
    step: u8,
    kind: TransactionKind,
    psbt: &Psbt,
) -> Result<()> {
    let txid = psbt.unsigned_tx.compute_txid().to_string();
    let purpose = transaction_purpose(step, kind, &txid);
    let encrypted_psbt =
        crate::core::crypto::encrypt(phone_seed, &purpose, psbt.to_string().as_bytes())?;
    write_json(
        path,
        &EncryptedTransaction {
            version: 3,
            step,
            kind,
            txid,
            encrypted_psbt,
        },
    )
}

fn write_encrypted_emergency_transaction(
    path: &Path,
    phone_seed: &[u8],
    kind: EmergencyTransactionKind,
    psbt: &Psbt,
) -> Result<()> {
    let txid = psbt.unsigned_tx.compute_txid().to_string();
    let purpose = emergency_transaction_purpose(kind, &txid);
    let encrypted_psbt =
        crate::core::crypto::encrypt(phone_seed, &purpose, psbt.to_string().as_bytes())?;
    write_json(
        path,
        &EncryptedEmergencyTransaction {
            version: 2,
            kind,
            txid,
            encrypted_psbt,
        },
    )
}

fn emergency_transaction_purpose(kind: EmergencyTransactionKind, txid: &str) -> String {
    format!("emergency/{}/{txid}", emergency_transaction_kind_name(kind))
}

fn emergency_transaction_kind_name(kind: EmergencyTransactionKind) -> &'static str {
    match kind {
        EmergencyTransactionKind::Trigger => "trigger",
        EmergencyTransactionKind::Withdrawal => "withdrawal",
        EmergencyTransactionKind::Cancellation => "cancellation",
    }
}

fn transaction_purpose(step: u8, kind: TransactionKind, txid: &str) -> String {
    format!("allowance/{step}/{}/{txid}", transaction_kind_name(kind))
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::{
        keys::DeviceKeys,
        recovery::PhoneRecoveryPackage,
        storage::{
            DeviceFile, HWW_DEVICE_FILE, HWW_PUBLIC_FILE, PublicDeviceFile, initialize_vault,
            load_config, load_device_keys,
        },
    };

    #[test]
    fn multithreaded_vanity_search_persists_a_recoverable_indexed_phone_key() {
        let dir = tempfile::tempdir().unwrap();
        let network = Network::Regtest;
        let secp = Secp256k1::new();
        let hww = DeviceKeys::generate_for_network(&secp, network).unwrap();
        write_json(
            &dir.path().join(HWW_DEVICE_FILE),
            &DeviceFile {
                kind: "hww".to_owned(),
                network: network_name(network).to_owned(),
                mnemonic: hww.mnemonic.to_string(),
                vault_key_index: hww.vault_key_index,
            },
        )
        .unwrap();
        write_json(
            &dir.path().join(HWW_PUBLIC_FILE),
            &PublicDeviceFile {
                version: 1,
                kind: "hww-public-key".to_owned(),
                network: network_name(network).to_owned(),
                vault_pubkey: hww.vault_pubkey.to_string(),
            },
        )
        .unwrap();

        let result = initialize_vanity_with_suffix(dir.path(), network, "v", 4, |_| {}).unwrap();
        assert!(result.vault_address.starts_with("bcrt1pv"));
        assert_eq!(result.worker_count, 4);
        assert!(result.attempts > 0);

        let persisted = load_device_keys(dir.path(), PHONE_DEVICE_FILE).unwrap();
        assert_eq!(
            persisted.vault_pubkey.to_string(),
            result.device.vault_pubkey
        );
        assert_eq!(persisted.vault_key_index, result.device.vault_key_index);
        let config = initialize_vault(dir.path()).unwrap();
        assert_eq!(config.vault_address, result.vault_address);

        fs::remove_file(dir.path().join(PHONE_DEVICE_FILE)).unwrap();
        restore_phone(
            dir.path(),
            &PhoneRecoveryPackage {
                version: 2,
                kind: "phone-recovery".to_owned(),
                phone_mnemonic: result.device.mnemonic,
                phone_vault_key_index: result.device.vault_key_index,
                phone_vault_pubkey: result.device.vault_pubkey,
                vault_descriptor: config.vault_descriptor,
                vault_address: config.vault_address,
            },
        )
        .unwrap();
        let restored = load_device_keys(dir.path(), PHONE_DEVICE_FILE).unwrap();
        assert_eq!(restored.vault_pubkey, persisted.vault_pubkey);
        assert_eq!(load_config(dir.path()).unwrap().network, "regtest");
    }

    #[test]
    fn vanity_search_encodes_the_mainnet_prefix() {
        let network = Network::Bitcoin;
        let hww = DeviceKeys::generate_for_network(&Secp256k1::new(), network).unwrap();
        let result = grind_vanity_phone_key(network, hww.vault_pubkey, "v", 4, |_| {}).unwrap();
        assert!(result.vault_address.starts_with("bc1pv"));
        assert_eq!(result.worker_count, 4);
        assert!(result.attempts > 0);
    }
}
