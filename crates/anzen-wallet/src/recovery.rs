//! Vault sweeps: cooperative moves the signer approves, and the hardware wallet's solo path
//! once its time lock has passed.

use crate::{
    ceremony::Schedule,
    transactions::{attach_signatures, finalize_vault_psbt, protocol_psbt, sign_vault_psbt},
};
use anyhow::{Context, Result, bail};
use anzen_chain::{Blockchain, ChainTip};
use anzen_common::{
    DEFAULT_FEE_RATE_SAT_VB, HWW_RECOVERY_SECONDS,
    keys::DeviceKeys,
    policy::{ControllerPolicy, SpendPath, VaultPolicy},
    protocol::{self, SoftwareCrypto},
    storage::VaultConfig,
    types::VaultUtxo,
};
use anzen_protocol::{
    script::Scripts,
    spend::{self, Spend},
    tx,
    wire::{self, SweepApproval, SweepRequest},
};
use bitcoin::{
    Address, Psbt, Transaction, Txid, hashes::Hash, key::Secp256k1, secp256k1::XOnlyPublicKey,
    taproot::TapLeafHash,
};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, str::FromStr};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SweepPath {
    Cooperative,
    HwwRecovery,
}

impl SweepPath {
    fn policy_path(self) -> SpendPath {
        match self {
            Self::Cooperative => SpendPath::Cooperative,
            Self::HwwRecovery => SpendPath::HwwRecovery,
        }
    }

    fn protocol_path(self) -> tx::SpendPath {
        match self {
            Self::Cooperative => tx::SpendPath::Cooperative,
            Self::HwwRecovery => tx::SpendPath::HwwRecovery,
        }
    }
}

/// What the solo path's time lock is judged by. BIP68 counts a coin's time from the median
/// time past of the block before the one that confirmed it, and a spend fits in the next block
/// once the tip's median time past has reached that start plus the delay.
#[derive(Debug, Clone)]
pub struct RecoveryClock {
    /// The chain tip's median time past.
    pub median_time: u64,
    /// For each confirmation height, the median time past of the block before it.
    coin_times: BTreeMap<u64, u64>,
}

impl RecoveryClock {
    /// Read the start time of every coin in `utxos` from the chain.
    pub fn read(
        chain: &(impl Blockchain + ?Sized),
        tip: &ChainTip,
        utxos: &[VaultUtxo],
    ) -> Result<Self> {
        let mut coin_times = BTreeMap::new();
        for utxo in utxos {
            let height = utxo.confirmation_height;
            if let std::collections::btree_map::Entry::Vacant(entry) = coin_times.entry(height) {
                entry.insert(chain.median_time_past(height.saturating_sub(1))?);
            }
        }
        Ok(Self::new(tip.median_time, coin_times))
    }

    pub fn new(median_time: u64, coin_times: BTreeMap<u64, u64>) -> Self {
        Self {
            median_time,
            coin_times,
        }
    }

    /// When the solo path opens for `utxo`, as the tip median time past it needs.
    pub fn opens_at(&self, utxo: &VaultUtxo) -> Result<u64> {
        let start = self
            .coin_times
            .get(&utxo.confirmation_height)
            .context("recovery clock has no start time for this coin")?;
        Ok(start.saturating_add(u64::from(HWW_RECOVERY_SECONDS)))
    }

    pub fn is_open(&self, utxo: &VaultUtxo) -> Result<bool> {
        Ok(self.median_time >= self.opens_at(utxo)?)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SweepResult {
    pub txid: Txid,
    pub input_count: usize,
    pub sent_sats: u64,
    pub fee_sats: u64,
}

impl SweepResult {
    fn from_spend(spend: &Spend) -> Self {
        Self {
            txid: protocol::txid(spend.txid),
            input_count: spend.transaction.input.len(),
            sent_sats: spend.amount_sats,
            fee_sats: spend.fee_sats,
        }
    }
}

fn policies(config: &VaultConfig) -> Result<(Scripts, VaultPolicy, ControllerPolicy)> {
    config.validate_public_policy()?;
    let network = config.bitcoin_network()?;
    let scripts = protocol::protocol(Scripts::new(
        &mut SoftwareCrypto::new(),
        &protocol::vault_keys(config)?,
    ))?;
    let vault = VaultPolicy::from_descriptor_for_network(&config.vault_descriptor, network)?;
    let controller = ControllerPolicy::new_for_network(
        XOnlyPublicKey::from_str(&config.phone_vault_pubkey)?,
        XOnlyPublicKey::from_str(&config.hww_vault_pubkey)?,
        network,
    )?;
    Ok((scripts, vault, controller))
}

/// Build the single-output spend of vault outputs through one vault path.
pub fn vault_spend(
    config: &VaultConfig,
    utxos: &[VaultUtxo],
    path: SweepPath,
    destination: &Address,
) -> Result<Spend> {
    let (scripts, _, _) = policies(config)?;
    if utxos
        .iter()
        .any(|utxo| utxo.txout.script_pubkey.as_bytes() != scripts.vault.script_pubkey)
    {
        bail!("sweep input is outside the vault policy");
    }
    let spend = protocol::protocol(spend::vault_sweep(
        &mut SoftwareCrypto::new(),
        &scripts,
        &protocol::inputs(utxos),
        path.protocol_path(),
        destination.script_pubkey().as_bytes(),
    ))
    .context("vault balance cannot pay the recovery sweep fee")?;
    debug_assert_eq!(
        DEFAULT_FEE_RATE_SAT_VB,
        anzen_protocol::consts::FEE_RATE_SAT_VB
    );
    Ok(spend)
}

/// Sign and finalize a sweep through the hardware wallet's solo path.
pub fn sign_single_key_sweep(
    config: &VaultConfig,
    utxos: &[VaultUtxo],
    path: SweepPath,
    destination: &Address,
    signer: &DeviceKeys,
) -> Result<(Transaction, SweepResult)> {
    if path == SweepPath::Cooperative {
        bail!("cooperative sweeps require both device signatures");
    }
    let spend = vault_spend(config, utxos, path, destination)?;
    let (_, vault, controller) = policies(config)?;
    let mut psbt = protocol_psbt(
        &spend.transaction,
        &spend.prevouts,
        &spend.spends,
        &vault,
        &controller,
    )?;
    sign_vault_psbt(&mut psbt, &vault, path.policy_path(), &signer.vault_keypair)?;
    let transaction = finalize_vault_psbt(psbt)?;
    Ok((transaction, SweepResult::from_spend(&spend)))
}

/// Sweep every vault output whose solo path is open, signed by the hardware wallet key.
pub fn recover(
    config: &VaultConfig,
    all_utxos: &[VaultUtxo],
    clock: &RecoveryClock,
    destination: &Address,
    signer: &DeviceKeys,
) -> Result<(Transaction, SweepResult)> {
    let utxos = mature_utxos(all_utxos, clock)?;
    if utxos.is_empty() {
        not_mature(all_utxos, clock)?;
    }
    sign_single_key_sweep(config, &utxos, SweepPath::HwwRecovery, destination, signer)
}

/// The error for a vault with nothing past its time lock yet.
pub fn not_mature(all_utxos: &[VaultUtxo], clock: &RecoveryClock) -> Result<()> {
    let timing = recovery_timing(all_utxos, clock)?;
    match (timing.opens_at, timing.seconds_remaining) {
        (Some(opens_at), Some(remaining)) => bail!(
            "HWW recovery is not mature: the signer's time lock opens at median time {opens_at} ({} days remaining)",
            remaining.div_ceil(86_400)
        ),
        _ => bail!("vault has no confirmed UTXOs to sweep"),
    }
}

/// Parameters for a cooperative sweep of `utxos` to `destination`.
pub fn sweep_request(
    config: &VaultConfig,
    utxos: &[VaultUtxo],
    destination: &Address,
) -> Result<SweepRequest> {
    if utxos.is_empty() {
        bail!("vault has no confirmed UTXOs to sweep");
    }
    vault_spend(config, utxos, SweepPath::Cooperative, destination)?;
    let request = SweepRequest {
        version: wire::VERSION,
        network: protocol::config_network(config)?,
        keys: protocol::vault_keys(config)?,
        inputs: protocol::inputs(utxos),
        destination: destination.to_string(),
    };
    protocol::protocol(request.build(&mut SoftwareCrypto::new()))?;
    Ok(request)
}

/// Rebuild an approved cooperative sweep, verify the signer, add the phone and finalize.
pub fn finalize_cooperative_sweep(
    config: &VaultConfig,
    approval: &SweepApproval,
    phone: &DeviceKeys,
) -> Result<(Transaction, SweepResult)> {
    let request = &approval.request;
    if approval.version != wire::VERSION
        || request.network != protocol::config_network(config)?
        || request.keys != protocol::vault_keys(config)?
    {
        bail!("cooperative sweep approval belongs to another vault");
    }
    let (scripts, spend) = protocol::protocol(request.build(&mut SoftwareCrypto::new()))?;
    let (_, vault, controller) = policies(config)?;
    let mut psbt = cooperative_psbt(&spend, &vault, &controller)?;
    attach_signatures(
        &mut psbt,
        &(0..spend.transaction.input.len()).collect::<Vec<_>>(),
        XOnlyPublicKey::from_str(&config.hww_vault_pubkey)?,
        TapLeafHash::from_byte_array(scripts.vault.cooperative_leaf),
        &approval.signatures,
    )
    .context("cooperative sweep approval contains an invalid signature")?;
    sign_vault_psbt(
        &mut psbt,
        &vault,
        SpendPath::Cooperative,
        &phone.vault_keypair,
    )?;
    let transaction = finalize_vault_psbt(psbt)?;
    Ok((transaction, SweepResult::from_spend(&spend)))
}

fn cooperative_psbt(
    spend: &Spend,
    vault: &VaultPolicy,
    controller: &ControllerPolicy,
) -> Result<Psbt> {
    protocol_psbt(
        &spend.transaction,
        &spend.prevouts,
        &spend.spends,
        vault,
        controller,
    )
}

/// The configuration a replacement phone key produces for an existing vault.
pub fn rotated_config(
    old_config: &VaultConfig,
    new_phone: &DeviceKeys,
    new_policy: &VaultPolicy,
) -> Result<VaultConfig> {
    let (hot_external, hot_internal) = new_phone.hot_descriptors(&Secp256k1::new())?;
    Ok(VaultConfig {
        version: old_config.version,
        network: old_config.network.clone(),
        phone_vault_pubkey: new_phone.vault_pubkey.to_string(),
        hww_vault_pubkey: old_config.hww_vault_pubkey.clone(),
        phone_hot_external_descriptor: hot_external,
        phone_hot_internal_descriptor: hot_internal,
        vault_descriptor: new_policy.descriptor_string(),
        vault_address: new_policy.address.to_string(),
        hww_recovery_seconds: old_config.hww_recovery_seconds,
        monthly_limit_sats: old_config.monthly_limit_sats,
        emergency_access_limit_sats: old_config.emergency_access_limit_sats,
    })
}

/// Whether a proposal must report the active monthly chain finished: the schedule has
/// allowances, yet no chain output or connector is among the inputs being spent.
pub fn allowances_reported_complete(
    schedule: Option<&Schedule>,
    spent: &[VaultUtxo],
) -> Result<bool> {
    let Some(schedule) = schedule else {
        return Ok(false);
    };
    if schedule.entries.is_empty() {
        return Ok(false);
    }
    let chain = crate::ceremony::monthly_chain_outpoints(schedule)?;
    Ok(!spent.iter().any(|utxo| chain.contains(&utxo.outpoint)))
}

pub fn mature_utxos(utxos: &[VaultUtxo], clock: &RecoveryClock) -> Result<Vec<VaultUtxo>> {
    let mut mature = Vec::new();
    for utxo in utxos {
        if clock.is_open(utxo)? {
            mature.push(utxo.clone());
        }
    }
    Ok(mature)
}

#[derive(Debug, Serialize)]
pub struct RecoveryTiming {
    pub delay_seconds: u64,
    /// The tip median time past at which the earliest coin's solo path opens.
    pub opens_at: Option<u64>,
    pub seconds_remaining: Option<u64>,
    pub total_sats: u64,
    pub mature_sats: u64,
}

/// Report the earliest live recovery window, not the age of a newer unrelated deposit.
pub fn recovery_timing(utxos: &[VaultUtxo], clock: &RecoveryClock) -> Result<RecoveryTiming> {
    let mut opens_at: Option<u64> = None;
    let mut total_sats = 0_u64;
    let mut mature_sats = 0_u64;
    for utxo in utxos {
        let opens = clock.opens_at(utxo)?;
        opens_at = Some(opens_at.map_or(opens, |earliest| earliest.min(opens)));
        let amount = utxo.txout.value.to_sat();
        total_sats = total_sats
            .checked_add(amount)
            .context("recovery balance overflow")?;
        if clock.median_time >= opens {
            mature_sats = mature_sats
                .checked_add(amount)
                .context("mature recovery balance overflow")?;
        }
    }
    Ok(RecoveryTiming {
        delay_seconds: u64::from(HWW_RECOVERY_SECONDS),
        opens_at,
        seconds_remaining: opens_at.map(|time| time.saturating_sub(clock.median_time)),
        total_sats,
        mature_sats,
    })
}

/// Review mature savings, or the oldest cohort if none is mature yet. Keep each request
/// bounded and prioritize value so a stream of small deposits cannot block larger savings.
pub fn recovery_review_inputs(
    utxos: &[VaultUtxo],
    clock: &RecoveryClock,
) -> Result<Vec<VaultUtxo>> {
    let mut selected = mature_utxos(utxos, clock)?;
    if selected.is_empty() {
        if let Some(oldest) = utxos.iter().map(|utxo| utxo.confirmation_height).min() {
            selected = utxos
                .iter()
                .filter(|utxo| utxo.confirmation_height == oldest)
                .cloned()
                .collect();
        }
    }
    selected.sort_by_key(|utxo| (std::cmp::Reverse(utxo.txout.value.to_sat()), utxo.outpoint));
    selected.truncate(anzen_protocol::consts::MAX_INPUTS);
    Ok(selected)
}

#[cfg(test)]
mod tests {
    use super::*;
    use bitcoin::{Amount, OutPoint, ScriptBuf, TxOut};

    const DELAY: u64 = HWW_RECOVERY_SECONDS as u64;

    fn utxo(vout: u32, sats: u64, height: u64) -> VaultUtxo {
        VaultUtxo {
            outpoint: OutPoint {
                txid: bitcoin::Txid::from_byte_array([0; 32]),
                vout,
            },
            txout: TxOut {
                value: Amount::from_sat(sats),
                script_pubkey: ScriptBuf::new(),
            },
            confirmation_height: height,
        }
    }

    /// Coins confirmed at height 100 start at time 1,000; at height 200, at time 50,000.
    fn clock(median_time: u64) -> RecoveryClock {
        RecoveryClock::new(median_time, BTreeMap::from([(100, 1_000), (200, 50_000)]))
    }

    #[test]
    fn the_solo_path_opens_when_the_tip_median_time_reaches_the_delay() {
        let coin = utxo(0, 1, 100);
        assert_eq!(DELAY, 65_535 * 512);
        assert_eq!(clock(0).opens_at(&coin).unwrap(), 1_000 + DELAY);
        assert!(!clock(1_000 + DELAY - 1).is_open(&coin).unwrap());
        assert!(clock(1_000 + DELAY).is_open(&coin).unwrap());
        assert!(clock(0).is_open(&utxo(0, 1, 300)).is_err());
    }

    #[test]
    fn recovery_reviews_are_bounded_and_newer_deposits_cannot_postpone_mature_savings() {
        let mut inputs = (0..1002)
            .map(|vout| {
                utxo(
                    vout,
                    u64::from(vout) + 1,
                    if vout == 1001 { 200 } else { 100 },
                )
            })
            .collect::<Vec<_>>();
        let before = clock(1_000 + DELAY - 1);
        let timing = recovery_timing(&inputs, &before).unwrap();
        assert_eq!(timing.seconds_remaining, Some(1));
        assert_eq!(timing.mature_sats, 0);
        let reviewed = recovery_review_inputs(&inputs, &before).unwrap();
        assert_eq!(reviewed.len(), 1000);
        assert_eq!(reviewed[0].outpoint.vout, 1000);
        assert!(reviewed.iter().all(|utxo| utxo.confirmation_height == 100));
        let open = clock(1_000 + DELAY);
        let timing = recovery_timing(&inputs, &open).unwrap();
        assert_eq!(timing.seconds_remaining, Some(0));
        assert_eq!(timing.total_sats - timing.mature_sats, 1002);
        assert_eq!(
            recovery_review_inputs(&inputs, &open).unwrap()[0]
                .outpoint
                .vout,
            1000
        );
        inputs.reverse();
        assert_eq!(
            recovery_review_inputs(&inputs, &open).unwrap()[0]
                .outpoint
                .vout,
            1000
        );
        assert_eq!(recovery_timing(&[], &open).unwrap().seconds_remaining, None);
        inputs[0].txout.value = Amount::from_sat(u64::MAX);
        assert!(recovery_timing(&inputs, &open).is_err());
    }
}
