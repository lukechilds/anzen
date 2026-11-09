//! The Anzen phone wallet: hot wallet, vault policy proposal and execution, rotation and
//! recovery.
//!
//! Mobile and desktop applications build on this crate without any hardware-wallet code.
//! Transactions come from `anzen_protocol`; this crate adds BDK, PSBT finalization and the
//! durable state each multi-step flow needs.

use anzen_common::storage::fs::StoragePath;
mod activity;
pub mod application;
#[cfg(target_arch = "wasm32")]
pub mod browser_backend;
#[cfg(target_arch = "wasm32")]
mod browser_store;
pub mod ceremony;
pub mod chain_transport;
pub mod cosigning;
pub mod device_rotation;
#[cfg(feature = "electrum")]
pub mod electrum;
pub mod offline_action;
mod pairing;
pub mod recovery;
pub mod transactions;
mod wallet;
pub mod watch;

/// Consumer-facing shared types. Wallet front ends do not need to import `core`.
pub mod api {
    #[cfg(feature = "electrum")]
    pub use crate::electrum::ElectrumBackend;
    #[cfg(feature = "native")]
    pub use crate::electrum::native::WebSocketConnector;
    pub use crate::{
        ceremony::{PolicyLimits, Schedule, TransactionKind},
        recovery::{RecoveryClock, RecoveryTiming, SweepResult},
    };
    #[cfg(feature = "native")]
    pub use anzen_chain::BitcoinCoreBackend;
    pub use anzen_chain::{Blockchain, ChainTip, RpcConfig};
    pub use anzen_common::cloud_backup::CloudRecoveryBackup;
    pub use anzen_common::cloud_restore::RestoreKeyRequest;
    pub use anzen_common::device_channel::DeviceMessage;
    pub use anzen_common::device_rotation::{ApprovedDevicePhoneRotation, DevicePhoneRotation};
    pub use anzen_common::outbox::{OutboxItem, OutgoingMessage, OutgoingMessageKind};
    pub use anzen_common::recovery_bundle::RecoveryVaultBundle;
    pub use anzen_common::storage::with_protected_storage;
    pub use anzen_common::{
        storage::{PublicDeviceFile, VaultConfig},
        types::VaultUtxo,
    };
    pub use anzen_protocol::wire::{
        OfflineAction, OfflineActionApproval, OfflineActionRequest, PolicyApproval, PolicyRequest,
        SweepApproval, SweepRequest,
    };
    pub use bitcoin::Network;
}

pub use wallet::HotWallet;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MonthlyBroadcastResult {
    pub transaction_txid: Txid,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EmergencyBroadcastResult {
    pub transaction_txid: Txid,
}

/// The HWW hand-off is a plain protocol approval; the phone's durable retry copy is encrypted.
#[derive(serde::Deserialize)]
#[serde(untagged)]
pub enum ApprovedPolicyInput {
    Approval(Box<PolicyApproval>),
    Encrypted(anzen_common::crypto::EncryptedBlob),
}

pub fn open_approved_policy(data_dir: &Path, input: ApprovedPolicyInput) -> Result<PolicyApproval> {
    match input {
        ApprovedPolicyInput::Approval(approval) => Ok(*approval),
        ApprovedPolicyInput::Encrypted(blob) => {
            if !blob.purpose.starts_with("policy-activation-v2:") {
                bail!("encrypted artifact is not an approved policy backup");
            }
            let phone = load_device_keys(data_dir, PHONE_DEVICE_FILE)?;
            let plaintext = anzen_common::crypto::decrypt(&phone.seed, &blob.purpose, &blob)?;
            let approval: PolicyApproval = serde_json::from_slice(&plaintext)?;
            let (_, graph) = ceremony::graph(&approval.request)?;
            if blob.purpose != format!("policy-activation-v2:{}", graph.rollover.txid) {
                bail!("encrypted policy backup does not match its rollover");
            }
            Ok(approval)
        }
    }
}

use crate::{
    ceremony::{
        ApprovedPolicy, EmergencyAccessSchedule, EmergencyTransactionKind,
        EncryptedEmergencyTransaction, EncryptedTransaction, HotAddressProvider, PolicyLimits,
        SCHEDULE_FILE, Schedule, ScheduleEntry, TransactionKind,
    },
    recovery::SweepResult,
};
use anyhow::{Context, Result, bail};
use anzen_chain::Blockchain;
use anzen_common::policy::{ControllerPath, ControllerPolicy, VaultAddressTemplate};
use anzen_protocol::wire::{PolicyApproval, PolicyRequest, SweepApproval, SweepRequest};
use bitcoin::{Address, Amount, Network, OutPoint, Psbt, TxOut, Txid, key::Secp256k1};
use std::{
    path::Path,
    str::FromStr,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc,
    },
    thread,
};

use crate::transactions::{finalize_vault_psbt, sign_controller_psbt_inputs};
use anzen_common::{
    DEFAULT_RECOVERY_LOCKTIME,
    keys::DeviceKeys,
    storage::{
        DeviceFile, InitializedDevice, PHONE_DEVICE_FILE, PublicDeviceFile, VaultConfig,
        load_config, load_device_keys, network_name, read_json, validate_supported_network,
        write_json,
    },
    types::VaultUtxo,
};

/// Initialize the phone key material and its BDK wallet state.
pub fn initialize(data_dir: &Path, network: Network) -> Result<InitializedDevice> {
    validate_supported_network(network)?;
    let phone = DeviceKeys::generate_for_network(&Secp256k1::new(), network)?;
    persist_phone(data_dir, network, &phone, DEFAULT_RECOVERY_LOCKTIME)
}

pub const VANITY_SUFFIX: &str = "vault";

/// Where a vanity search runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VanityEngine {
    /// The GPU through wgpu. Builds without the `gpu` feature refuse it.
    Gpu,
    /// Every CPU thread, through the library code.
    Cpu,
}

#[derive(Debug)]
pub struct VanityInitialization {
    pub device: InitializedDevice,
    pub vault_address: String,
    pub attempts: u64,
    /// What ran the search, for display: a GPU adapter or a CPU thread count.
    pub searcher: String,
}

#[derive(Debug)]
pub(crate) struct VanityPhoneKey {
    pub(crate) phone: DeviceKeys,
    pub(crate) recovery_locktime: u32,
    pub(crate) vault_address: String,
    pub(crate) attempts: u64,
    pub(crate) searcher: String,
}

/// Find a vault address that begins with the vanity prefix by grinding the recovery leaf's
/// locktime for a fresh phone key, on the GPU or on every CPU thread.
pub fn initialize_vanity<F>(
    data_dir: &Path,
    signer: &PublicDeviceFile,
    engine: VanityEngine,
    report_progress: F,
) -> Result<VanityInitialization>
where
    F: Fn(u64) + Sync,
{
    initialize_vanity_with_suffix(data_dir, signer, VANITY_SUFFIX, engine, report_progress)
}

pub fn vanity_address_prefix(network: Network) -> Result<&'static str> {
    match network {
        Network::Bitcoin => Ok("bc1pvault"),
        Network::Signet => Ok("tb1pvault"),
        Network::Regtest => Ok("bcrt1pvault"),
        other => bail!("vanity initialization is unsupported on {other}"),
    }
}

fn initialize_vanity_with_suffix<F>(
    data_dir: &Path,
    signer: &PublicDeviceFile,
    suffix: &str,
    engine: VanityEngine,
    report_progress: F,
) -> Result<VanityInitialization>
where
    F: Fn(u64) + Sync,
{
    signer.validate_kind("hww-public-key")?;
    let network = signer.bitcoin_network()?;
    ensure_phone_is_uninitialized(data_dir)?;
    let vanity = grind_vanity_phone_key(
        network,
        signer.parsed_vault_pubkey()?,
        suffix,
        engine,
        report_progress,
    )?;
    let device = persist_phone(data_dir, network, &vanity.phone, vanity.recovery_locktime)?;
    Ok(VanityInitialization {
        device,
        vault_address: vanity.vault_address,
        attempts: vanity.attempts,
        searcher: vanity.searcher,
    })
}

/// Threads a CPU vanity search uses: every one the system reports.
pub fn available_worker_count() -> usize {
    thread::available_parallelism()
        .map(std::num::NonZeroUsize::get)
        .unwrap_or(1)
}

/// A finished CPU vanity benchmark.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CpuBenchmark {
    pub threads: usize,
    /// Candidates evaluated across every thread.
    pub attempts: u64,
    pub seconds: f64,
    /// Candidates per second.
    pub rate: f64,
}

/// The CPU vanity search's throughput on fixed public inputs: `threads` workers grinding recovery
/// locktimes with the library code, exactly as [`VanityEngine::Cpu`] does, for about `seconds`.
/// `now` is a clock in seconds: browsers have no `std::time::Instant`, so they pass
/// `performance.now()`. A single thread runs on the caller's, so it needs no thread support.
pub fn cpu_vanity_benchmark(
    seconds: f64,
    threads: usize,
    now: impl Fn() -> f64 + Sync,
) -> Result<CpuBenchmark> {
    use bitcoin::secp256k1::XOnlyPublicKey;
    use std::str::FromStr;
    if threads == 0 {
        bail!("the benchmark requires at least one thread");
    }
    // G and 2G: any valid keys will do.
    let template = VaultAddressTemplate::new(
        XOnlyPublicKey::from_str(
            "79be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798",
        )?,
        XOnlyPublicKey::from_str(
            "c6047f9441ed7d6d3045406e95c07cd85c778e4b8cef3ca7abac09b95c709ee5",
        )?,
    )?;
    let locktimes = anzen_common::RECOVERY_LOCKTIMES;
    let started = now();
    let worker = |offset: usize| -> Result<u64> {
        let mut crypto = anzen_common::protocol::SoftwareCrypto::new();
        let mut attempts = 0_u64;
        while now() - started < seconds {
            for _ in 0..64 {
                let locktime = locktimes.start
                    + ((attempts * threads as u64 + offset as u64)
                        % u64::from(locktimes.len() as u32)) as u32;
                template.output_key(&mut crypto, locktime)?;
                attempts += 1;
            }
        }
        Ok(attempts)
    };
    let attempts = if threads == 1 {
        worker(0)?
    } else {
        thread::scope(|scope| {
            let workers = (0..threads)
                .map(|offset| scope.spawn(move || worker(offset)))
                .collect::<Vec<_>>();
            workers.into_iter().try_fold(0_u64, |sum, handle| {
                Ok::<_, anyhow::Error>(
                    sum + handle
                        .join()
                        .map_err(|_| anyhow::anyhow!("a benchmark thread panicked"))??,
                )
            })
        })?
    };
    let elapsed = now() - started;
    Ok(CpuBenchmark {
        threads,
        attempts,
        seconds: elapsed,
        rate: attempts as f64 / elapsed.max(f64::MIN_POSITIVE),
    })
}

pub(crate) fn grind_vanity_phone_key<F>(
    network: Network,
    hww_pubkey: bitcoin::secp256k1::XOnlyPublicKey,
    suffix: &str,
    engine: VanityEngine,
    report_progress: F,
) -> Result<VanityPhoneKey>
where
    F: Fn(u64) + Sync,
{
    match engine {
        VanityEngine::Cpu => grind_vanity_phone_key_on_cpu(
            network,
            hww_pubkey,
            suffix,
            available_worker_count(),
            report_progress,
        ),
        VanityEngine::Gpu => {
            grind_vanity_phone_key_on_gpu(network, hww_pubkey, suffix, report_progress)
        }
    }
}

/// The public inputs of a GPU search over the recovery locktimes of one phone and signer key.
pub fn vanity_nonce_inputs(
    phone_pubkey: bitcoin::secp256k1::XOnlyPublicKey,
    hww_pubkey: bitcoin::secp256k1::XOnlyPublicKey,
    suffix: &str,
) -> Result<anzen_vanity::NonceInputs> {
    let target = VanityTarget::new(suffix)?;
    Ok(anzen_vanity::NonceInputs {
        internal_key: anzen_common::policy::internal_key(phone_pubkey, hww_pubkey)?.serialize(),
        signer_key: hww_pubkey.serialize(),
        target_value: target.value,
        target_bits: target.bit_count,
    })
}

/// Grind the recovery locktime on the GPU for a fresh phone key, then check the winner with the
/// library code like a CPU result.
#[cfg(feature = "gpu")]
fn grind_vanity_phone_key_on_gpu<F>(
    network: Network,
    hww_pubkey: bitcoin::secp256k1::XOnlyPublicKey,
    suffix: &str,
    report_progress: F,
) -> Result<VanityPhoneKey>
where
    F: Fn(u64) + Sync,
{
    validate_supported_network(network)?;
    let phone = DeviceKeys::generate_for_network(&Secp256k1::new(), network)?;
    let inputs = vanity_nonce_inputs(phone.vault_pubkey, hww_pubkey, suffix)?;
    let gpu = anzen_vanity::GpuSearch::open()?;
    let last_report = AtomicU64::new(0);
    let result =
        gpu.search_nonces_blocking(&inputs, anzen_common::RECOVERY_LOCKTIMES, |attempts| {
            maybe_report_progress(attempts, &last_report, &report_progress);
            true
        })?;
    let locktime = result
        .index
        .context("vanity search tried every recovery locktime without a match")?;
    verified_vanity(
        phone,
        network,
        hww_pubkey,
        locktime,
        suffix,
        result.attempts,
        gpu.adapter_name().to_owned(),
    )
}

#[cfg(not(feature = "gpu"))]
fn grind_vanity_phone_key_on_gpu<F>(
    _network: Network,
    _hww_pubkey: bitcoin::secp256k1::XOnlyPublicKey,
    _suffix: &str,
    _report_progress: F,
) -> Result<VanityPhoneKey> {
    bail!("this build has no GPU vanity search; use the CPU")
}

fn grind_vanity_phone_key_on_cpu<F>(
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
    let phone = DeviceKeys::generate_for_network(&Secp256k1::new(), network)?;
    let template = Arc::new(VaultAddressTemplate::new(phone.vault_pubkey, hww_pubkey)?);
    let locktimes = anzen_common::RECOVERY_LOCKTIMES;
    let stopped = Arc::new(AtomicBool::new(false));
    let attempts = Arc::new(AtomicU64::new(0));
    let last_report = Arc::new(AtomicU64::new(0));
    let (sender, receiver) = mpsc::channel::<Result<u32>>();

    let winning_locktime = thread::scope(|scope| -> Result<u32> {
        for worker in 0..worker_count {
            let sender = sender.clone();
            let template = Arc::clone(&template);
            let stopped = Arc::clone(&stopped);
            let attempts = Arc::clone(&attempts);
            let last_report = Arc::clone(&last_report);
            let report_progress = &report_progress;
            scope.spawn(move || {
                let result = (|| -> Result<()> {
                    let mut crypto = anzen_common::protocol::SoftwareCrypto::new();
                    let mut locktime = u64::from(locktimes.start) + worker as u64;
                    let stride = worker_count as u64;
                    let mut pending_attempts = 0_u64;
                    while locktime < u64::from(locktimes.end) && !stopped.load(Ordering::Relaxed) {
                        pending_attempts += 1;
                        let output_key = template.output_key(&mut crypto, locktime as u32)?;
                        if target.matches(&output_key) {
                            attempts.fetch_add(pending_attempts, Ordering::Relaxed);
                            if !stopped.swap(true, Ordering::Relaxed) {
                                let _ = sender.send(Ok(locktime as u32));
                            }
                            return Ok(());
                        }
                        if pending_attempts == 4_096 {
                            let total = attempts.fetch_add(pending_attempts, Ordering::Relaxed)
                                + pending_attempts;
                            pending_attempts = 0;
                            maybe_report_progress(total, &last_report, report_progress);
                        }
                        locktime += stride;
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
            .context("vanity search tried every recovery locktime without a match")?
    })?;

    verified_vanity(
        phone,
        network,
        hww_pubkey,
        winning_locktime,
        suffix,
        attempts.load(Ordering::Relaxed),
        format!("{worker_count} CPU threads"),
    )
}

/// A search's winning recovery locktime, after re-deriving the address through the canonical
/// Miniscript policy: a search result is never trusted on its own.
fn verified_vanity(
    phone: DeviceKeys,
    network: Network,
    hww_pubkey: bitcoin::secp256k1::XOnlyPublicKey,
    recovery_locktime: u32,
    suffix: &str,
    attempts: u64,
    searcher: String,
) -> Result<VanityPhoneKey> {
    let vault_address = anzen_common::policy::VaultPolicy::new_for_network(
        phone.vault_pubkey,
        hww_pubkey,
        recovery_locktime,
        network,
    )?
    .address;
    let expected_prefix = match network {
        Network::Bitcoin => format!("bc1p{suffix}"),
        Network::Signet => format!("tb1p{suffix}"),
        Network::Regtest => format!("bcrt1p{suffix}"),
        other => bail!("vanity initialization is unsupported on {other}"),
    };
    if !vault_address.to_string().starts_with(&expected_prefix) {
        bail!("vanity search result did not reproduce the requested address prefix");
    }
    Ok(VanityPhoneKey {
        phone,
        recovery_locktime,
        vault_address: vault_address.to_string(),
        attempts,
        searcher,
    })
}

fn persist_phone(
    data_dir: &Path,
    network: Network,
    phone: &DeviceKeys,
    recovery_locktime: u32,
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
            recovery_locktime,
        },
    )?;
    // Only newly generated seeds reach here; restored seeds are never marked.
    HotWallet::open_or_create(data_dir)?.mark_fresh()?;
    Ok(InitializedDevice {
        mnemonic,
        vault_pubkey: phone.vault_pubkey.to_string(),
        vault_key_index: phone.vault_key_index,
        recovery_locktime,
    })
}

fn ensure_phone_is_uninitialized(data_dir: &Path) -> Result<()> {
    let phone_path = data_dir.join(PHONE_DEVICE_FILE);
    if phone_path.storage_exists() {
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
    limits: PolicyLimits,
) -> Result<PolicyRequest> {
    let config = load_config(data_dir)?;
    ensure_backend_network(backend, &config)?;
    let utxos = backend.scan_vault(&config)?;
    let connectors = backend.scan_connectors(&config)?;
    let schedule = active_schedule(data_dir)?;
    let spent = utxos.iter().chain(&connectors).cloned().collect::<Vec<_>>();
    let complete = recovery::allowances_reported_complete(schedule.as_ref(), &spent)?;
    let mut wallet = HotWallet::open_or_create(data_dir)?;
    let phone = load_device_keys(data_dir, PHONE_DEVICE_FILE)?;
    ceremony::propose(
        &config,
        &utxos,
        &connectors,
        limits,
        &mut wallet,
        complete,
        cosigning::Phone {
            data_dir,
            keys: &phone,
        },
    )
}

fn active_schedule(data_dir: &Path) -> Result<Option<Schedule>> {
    if data_dir.join(SCHEDULE_FILE).storage_is_file() {
        Ok(Some(load_schedule(data_dir)?))
    } else {
        Ok(None)
    }
}

pub fn activate_policy(
    data_dir: &Path,
    backend: &dyn HotWalletBackend,
    approval: &PolicyApproval,
) -> Result<Schedule> {
    let config = load_config(data_dir)?;
    ensure_backend_network(backend, &config)?;
    let phone = load_device_keys(data_dir, PHONE_DEVICE_FILE)?;
    let approved = ApprovedPolicy::verify(data_dir, &config, approval, &phone)?;
    let network = config.bitcoin_network()?;
    let wallet = HotWallet::open_or_create(data_dir)?;
    let graph = &approved.graph;
    for script in graph
        .allowances
        .iter()
        .map(|step| &step.hot_script)
        .chain(graph.emergency.iter().map(|access| &access.hot_script))
    {
        if !wallet.is_mine(bitcoin::ScriptBuf::from_bytes(script.clone())) {
            bail!("approved policy pays an address outside this phone's hot wallet");
        }
    }
    drop(wallet);
    let hot_address = |script: &[u8]| -> Result<String> {
        Ok(Address::from_script(bitcoin::Script::from_bytes(script), network)?.to_string())
    };

    let rollover = finalize_vault_psbt(approved.rollover().clone())?;
    // Stage each epoch independently. A rejected or interrupted rollover must leave the current
    // schedule and its encrypted transactions intact. Retain enough material to retry activation.
    let epoch_dir = data_dir
        .join("phone/transactions")
        .join(rollover.compute_txid().to_string());
    let previous_epoch = active_schedule(data_dir)?
        .map(|schedule| schedule.rollover_txid.parse::<Txid>())
        .transpose()?;
    if epoch_dir.join("activated.json").storage_exists()
        && previous_epoch.is_some_and(|txid| txid != rollover.compute_txid())
    {
        bail!("this policy epoch has already been superseded; refusing to reactivate it");
    }
    write_json(
        &epoch_dir.join("approved-policy.json"),
        &anzen_common::crypto::encrypt(
            &phone.seed,
            &format!("policy-activation-v2:{}", rollover.compute_txid()),
            &serde_json::to_vec(approval)?,
        )?,
    )?;
    write_json(&epoch_dir.join("rollover.json"), &rollover)?;
    let mut entries = Vec::with_capacity(graph.allowances.len());
    for (allowance, authorization) in graph.allowances.iter().zip(approved.authorizations()) {
        let authorization_path =
            encrypted_transaction_path(&epoch_dir, allowance.step, TransactionKind::Authorization);
        write_encrypted_transaction(
            &authorization_path,
            &phone.seed,
            allowance.step,
            TransactionKind::Authorization,
            authorization,
        )?;
        entries.push(ScheduleEntry {
            step: allowance.step,
            hot_address: hot_address(&allowance.hot_script)?,
            authorization_file: relative_to(data_dir, &authorization_path)?,
            authorization_txid: authorization.unsigned_tx.compute_txid().to_string(),
            connector: ApprovedPolicy::connector(allowance.connector),
            next_connector: allowance.next_connector.map(ApprovedPolicy::connector),
        });
    }
    let emergency_access = match (&graph.emergency, approved.emergency()) {
        (Some(emergency), Some((trigger, withdrawal))) => {
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
                trigger,
            )?;
            write_encrypted_emergency_transaction(
                &withdrawal_path,
                &phone.seed,
                EmergencyTransactionKind::Withdrawal,
                withdrawal,
            )?;
            Some(EmergencyAccessSchedule {
                amount_sats: emergency.amount_sats,
                delay_seconds: anzen_common::EMERGENCY_ACCESS_DELAY_SECONDS,
                hot_address: hot_address(&emergency.hot_script)?,
                trigger_file: relative_to(data_dir, &trigger_path)?,
                trigger_txid: trigger.unsigned_tx.compute_txid().to_string(),
                withdrawal_file: relative_to(data_dir, &withdrawal_path)?,
                withdrawal_txid: withdrawal.unsigned_tx.compute_txid().to_string(),
                trigger_connector: ApprovedPolicy::connector(emergency.trigger_connector),
                withdrawal_connector: ApprovedPolicy::connector(emergency.withdrawal_connector),
            })
        }
        (None, None) => None,
        _ => bail!("approved emergency access is incomplete"),
    };
    let controller = controller_policy(&config)?;
    let limits = approval.request.limits;
    let schedule = Schedule {
        version: 5,
        rollover_txid: rollover.compute_txid().to_string(),
        controller_descriptor: controller.descriptor_string(),
        controller_address: controller.address.to_string(),
        connector_value_sats: anzen_common::CONNECTOR_VALUE_SATS,
        monthly_limit_sats: limits.monthly_limit_sats,
        monthly_delay_seconds: anzen_common::MONTHLY_ALLOWANCE_DELAY_SECONDS,
        emergency_access_limit_sats: limits.emergency_access_limit_sats,
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
    // Pre-staging versions have no activation marker. Remember their active epoch too before
    // replacing the pointer, so an already-confirmed legacy approval cannot reactivate it later.
    if let Some(previous) = previous_epoch.filter(|txid| *txid != broadcast_txid) {
        write_json(
            &data_dir.join("phone/transactions").join(previous.to_string()).join("activated.json"),
            &true,
        ).context("rollover accepted but archiving the prior epoch failed; retry the same approved policy")?;
    }
    write_json(&data_dir.join(SCHEDULE_FILE), &schedule).context(
        "rollover accepted but schedule activation failed; retry the same approved policy",
    )?;
    anzen_common::storage::set_policy_limits(
        data_dir,
        limits.monthly_limit_sats,
        limits.emergency_access_limit_sats,
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
    let plaintext = anzen_common::crypto::decrypt(&phone.seed, &purpose, &artifact.encrypted_psbt)?;
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
    let plaintext = anzen_common::crypto::decrypt(&phone.seed, &purpose, &artifact.encrypted_psbt)?;
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
    let destination = wallet.next_change_address()?;
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
    let transaction =
        offline_action::phone_revocation(config, &[connector_utxo], &destination, phone)?;
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
        || schedule.connector_value_sats != anzen_common::CONNECTOR_VALUE_SATS
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

pub fn create_cooperative_sweep(
    data_dir: &Path,
    backend: &dyn HotWalletBackend,
    destination: &Address,
) -> Result<SweepRequest> {
    let config = load_config(data_dir)?;
    ensure_backend_network(backend, &config)?;
    let utxos = backend.scan_vault(&config)?;
    let phone = load_device_keys(data_dir, PHONE_DEVICE_FILE)?;
    recovery::sweep_request(
        &config,
        &utxos,
        destination,
        cosigning::Phone {
            data_dir,
            keys: &phone,
        },
    )
}

pub fn broadcast_cooperative_sweep(
    data_dir: &Path,
    backend: &dyn HotWalletBackend,
    approval: &SweepApproval,
) -> Result<SweepResult> {
    let config = load_config(data_dir)?;
    ensure_backend_network(backend, &config)?;
    let phone = load_device_keys(data_dir, PHONE_DEVICE_FILE)?;
    let (transaction, result) = recovery::finalize_cooperative_sweep(
        &config,
        approval,
        cosigning::Phone {
            data_dir,
            keys: &phone,
        },
    )?;
    let txid = backend
        .broadcast(&transaction)
        .context("failed to broadcast cooperative vault sweep")?;
    if txid != result.txid {
        bail!("chain backend returned an unexpected cooperative sweep transaction ID");
    }
    Ok(result)
}

/// Chain functionality needed by a mobile wallet in addition to the shared vault operations.
pub trait HotWalletBackend: Blockchain {
    fn sync_hot_wallet(&self, wallet: &mut HotWallet) -> Result<()>;
}

#[cfg(feature = "native")]
impl HotWalletBackend for anzen_chain::BitcoinCoreBackend {
    fn sync_hot_wallet(&self, wallet: &mut HotWallet) -> Result<()> {
        wallet.sync_core(&self.client)
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
        anzen_common::crypto::encrypt(phone_seed, &purpose, psbt.to_string().as_bytes())?;
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
        anzen_common::crypto::encrypt(phone_seed, &purpose, psbt.to_string().as_bytes())?;
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
    use anzen_common::{
        keys::DeviceKeys,
        storage::{
            DeviceFile, HWW_DEVICE_FILE, HWW_PUBLIC_FILE, PublicDeviceFile, load_config,
            load_device_keys, load_public_device,
        },
    };

    #[test]
    fn multithreaded_vanity_search_persists_a_recoverable_indexed_phone_key() {
        for (network, prefix) in [(Network::Regtest, "bcrt1pv"), (Network::Signet, "tb1pv")] {
            vanity_search_persists_a_recoverable_indexed_phone_key(network, prefix);
        }
    }

    fn vanity_search_persists_a_recoverable_indexed_phone_key(network: Network, prefix: &str) {
        let dir = anzen_common::storage::tempdir().unwrap();
        let secp = Secp256k1::new();
        let hww = DeviceKeys::generate_for_network(&secp, network).unwrap();
        write_json(
            &dir.path().join(HWW_DEVICE_FILE),
            &DeviceFile {
                kind: "hww".to_owned(),
                network: network_name(network).to_owned(),
                mnemonic: hww.mnemonic.to_string(),
                vault_key_index: hww.vault_key_index,
                recovery_locktime: DEFAULT_RECOVERY_LOCKTIME,
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

        let signer = load_public_device(dir.path(), HWW_PUBLIC_FILE).unwrap();
        let result =
            initialize_vanity_with_suffix(dir.path(), &signer, "v", VanityEngine::Cpu, |_| {})
                .unwrap();
        assert!(
            result.vault_address.starts_with(prefix),
            "{}",
            result.vault_address
        );
        assert!(result.searcher.ends_with("CPU threads"));
        assert!(result.attempts > 0);

        let persisted = load_device_keys(dir.path(), PHONE_DEVICE_FILE).unwrap();
        assert_eq!(
            persisted.vault_pubkey.to_string(),
            result.device.vault_pubkey
        );
        assert_eq!(persisted.vault_key_index, result.device.vault_key_index);
        let config =
            crate::pairing::initialize_vault_with_public_hww(dir.path(), network, &signer).unwrap();
        assert_eq!(config.vault_address, result.vault_address);

        let restored = DeviceKeys::parse_for_network_at_index(
            &secp,
            &result.device.mnemonic,
            network,
            result.device.vault_key_index,
        )
        .unwrap();
        assert_eq!(restored.vault_pubkey, persisted.vault_pubkey);
        assert_eq!(config.phone_vault_pubkey, restored.vault_pubkey.to_string());
        assert_eq!(
            load_config(dir.path()).unwrap().network,
            network_name(network)
        );
    }

    #[test]
    fn vanity_search_encodes_the_mainnet_prefix() {
        let network = Network::Bitcoin;
        let hww = DeviceKeys::generate_for_network(&Secp256k1::new(), network).unwrap();
        let result =
            grind_vanity_phone_key_on_cpu(network, hww.vault_pubkey, "v", 4, |_| {}).unwrap();
        assert!(result.vault_address.starts_with("bc1pv"));
        assert_eq!(result.searcher, "4 CPU threads");
        assert!(result.attempts > 0);
        assert_eq!(result.phone.vault_key_index, 0);
        assert!(anzen_common::RECOVERY_LOCKTIMES.contains(&result.recovery_locktime));
    }

    #[test]
    fn the_cpu_benchmark_counts_candidates_on_every_thread() {
        let epoch = std::time::Instant::now();
        let clock = || epoch.elapsed().as_secs_f64();
        let one = cpu_vanity_benchmark(0.2, 1, clock).unwrap();
        let two = cpu_vanity_benchmark(0.2, 2, clock).unwrap();
        assert_eq!((one.threads, two.threads), (1, 2));
        assert!(one.attempts > 0 && one.seconds >= 0.2);
        assert!((one.rate - one.attempts as f64 / one.seconds).abs() < 1e-6);
        // Every thread evaluates at least one run of 64 before checking the clock, so two threads
        // count at least 128 however loaded the machine is (comparing rates would be flaky).
        assert!(
            two.attempts >= 128 && two.attempts % 64 == 0,
            "{}",
            two.attempts
        );
        assert!(cpu_vanity_benchmark(0.1, 0, clock).is_err());
    }

    #[test]
    fn the_gpu_searches_exactly_the_protocol_recovery_locktimes() {
        assert_eq!(
            anzen_vanity::NONCE_LOCKTIMES,
            anzen_common::RECOVERY_LOCKTIMES
        );
    }

    #[cfg(feature = "gpu")]
    #[test]
    fn gpu_vanity_search_finds_a_locktime_the_library_confirms() {
        // Only a machine without a GPU skips; a rejected shader must fail.
        if let Err(error) = anzen_vanity::GpuSearch::open() {
            assert!(error.to_string().contains("no GPU adapter"), "{error:#}");
            eprintln!("skipping: {error:#}");
            return;
        }
        let network = Network::Signet;
        let hww = DeviceKeys::generate_for_network(&Secp256k1::new(), network).unwrap();
        // Ten bits keeps the test quick while still spanning several batches now and then.
        let result =
            grind_vanity_phone_key(network, hww.vault_pubkey, "va", VanityEngine::Gpu, |_| {})
                .unwrap();
        assert!(result.vault_address.starts_with("tb1pva"));
        assert_eq!(result.phone.vault_key_index, 0);
        assert!(anzen_common::RECOVERY_LOCKTIMES.contains(&result.recovery_locktime));
        let template =
            VaultAddressTemplate::new(result.phone.vault_pubkey, hww.vault_pubkey).unwrap();
        let address = template
            .address(
                &mut anzen_common::protocol::SoftwareCrypto::new(),
                result.recovery_locktime,
                network,
            )
            .unwrap();
        assert_eq!(address.to_string(), result.vault_address);
    }
}

#[cfg(test)]
pub(crate) mod test_support {
    use anyhow::Result;
    use anzen_common::storage::VaultConfig;
    use anzen_signer_app::application::SignerApplication;
    use bitcoin::Network;
    use std::path::Path;

    pub struct InitializedVault {
        pub config: VaultConfig,
    }

    /// Both simulated devices in one directory, paired and holding the first cloud backup.
    pub fn initialize(data_dir: &Path) -> Result<InitializedVault> {
        crate::initialize(data_dir, Network::Regtest)?;
        anzen_signer_app::initialize(data_dir, Network::Regtest)?;
        let wallet = crate::application::WalletApplication::new(data_dir.into(), false);
        let signer = SignerApplication::new(data_dir.into(), false);
        let config = wallet.pair_signer(&signer.public_identity()?)?;
        signer.accept_vault(&config)?;
        let request = wallet.propose_backup()?;
        let review = signer.review_backup(&request)?;
        wallet.store_backup(&signer.approve_backup(&review.review_id, &request)?)?;
        Ok(InitializedVault { config })
    }
}
