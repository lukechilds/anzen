//! Vault sweeps: cooperative moves the signer approves and single-key recovery paths.

use crate::{
    ceremony::Schedule,
    transactions::{attach_signatures, finalize_vault_psbt, protocol_psbt, sign_vault_psbt},
};
use anyhow::{Context, Result, bail};
use anzen_common::{
    DEFAULT_FEE_RATE_SAT_VB, HWW_RECOVERY_BLOCKS, PHONE_RECOVERY_BLOCKS,
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
use std::str::FromStr;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SweepPath {
    Cooperative,
    PhoneRecovery,
    HwwRecovery,
}

impl SweepPath {
    fn policy_path(self) -> SpendPath {
        match self {
            Self::Cooperative => SpendPath::Cooperative,
            Self::PhoneRecovery => SpendPath::PhoneRecovery,
            Self::HwwRecovery => SpendPath::HwwRecovery,
        }
    }

    fn protocol_path(self) -> tx::SpendPath {
        match self {
            Self::Cooperative => tx::SpendPath::Cooperative,
            Self::PhoneRecovery => tx::SpendPath::PhoneRecovery,
            Self::HwwRecovery => tx::SpendPath::HwwRecovery,
        }
    }

    fn delay(self) -> Option<u64> {
        match self {
            Self::Cooperative => None,
            Self::PhoneRecovery => Some(u64::from(PHONE_RECOVERY_BLOCKS)),
            Self::HwwRecovery => Some(u64::from(HWW_RECOVERY_BLOCKS)),
        }
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

/// Phone key material decrypted from a cloud backup by the HWW or a recovery friend.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PhoneRecoveryPackage {
    pub version: u8,
    pub kind: String,
    pub phone_mnemonic: String,
    #[serde(default)]
    pub phone_vault_key_index: u32,
    pub phone_vault_pubkey: String,
    pub vault_descriptor: String,
    pub vault_address: String,
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

/// Sign and finalize a sweep through a single-key recovery path.
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

/// Sweep every mature vault output through a single-key recovery path.
pub fn recover(
    config: &VaultConfig,
    all_utxos: &[VaultUtxo],
    tip_height: u64,
    path: SweepPath,
    destination: &Address,
    signer: &DeviceKeys,
) -> Result<(Transaction, SweepResult)> {
    let utxos = mature_utxos(all_utxos, path, tip_height);
    if utxos.is_empty() {
        if let (Some(delay), Some(oldest)) = (path.delay(), all_utxos.first()) {
            let valid_height = oldest.confirmation_height.saturating_add(delay);
            let remaining = valid_height.saturating_sub(tip_height.saturating_add(1));
            bail!(
                "no vault UTXOs are mature for {path:?}; earliest next-block validity height is {valid_height} ({remaining} blocks remaining)"
            );
        }
        bail!("vault has no confirmed UTXOs to sweep");
    }
    sign_single_key_sweep(config, &utxos, path, destination, signer)
}

/// A reviewed phone-recovery transfer. The phone signs it only after its delay matures, and
/// its transaction ID is fixed at review so a retry can never become a different spend.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PhoneRecoveryTransfer {
    pub inputs: Vec<VaultUtxo>,
    pub destination: String,
    pub txid: Txid,
    pub amount_sats: u64,
    pub fee_sats: u64,
}

impl PhoneRecoveryTransfer {
    pub fn prepare(
        config: &VaultConfig,
        inputs: &[VaultUtxo],
        destination: &Address,
    ) -> Result<Self> {
        let spend = vault_spend(config, inputs, SweepPath::PhoneRecovery, destination)?;
        if spend.amount_sats < destination.script_pubkey().minimal_non_dust().to_sat() {
            bail!("recovery destination is dust");
        }
        Ok(Self {
            inputs: inputs.to_vec(),
            destination: destination.to_string(),
            txid: protocol::txid(spend.txid),
            amount_sats: spend.amount_sats,
            fee_sats: spend.fee_sats,
        })
    }

    /// Sign the reviewed transaction with the phone key and finalize it.
    pub fn sign(&self, config: &VaultConfig, phone: &DeviceKeys) -> Result<Transaction> {
        if phone.vault_pubkey.to_string() != config.phone_vault_pubkey {
            bail!("recovery key is not the surviving phone");
        }
        let destination =
            Address::from_str(&self.destination)?.require_network(config.bitcoin_network()?)?;
        let (transaction, result) = sign_single_key_sweep(
            config,
            &self.inputs,
            SweepPath::PhoneRecovery,
            &destination,
            phone,
        )?;
        if result.txid != self.txid {
            bail!("phone recovery transfer changed the reviewed transaction");
        }
        Ok(transaction)
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
        phone_recovery_blocks: old_config.phone_recovery_blocks,
        hww_recovery_blocks: old_config.hww_recovery_blocks,
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

pub fn mature_utxos(utxos: &[VaultUtxo], path: SweepPath, tip_height: u64) -> Vec<VaultUtxo> {
    let Some(delay) = path.delay() else {
        return utxos.to_vec();
    };
    let next_height = tip_height.saturating_add(1);
    utxos
        .iter()
        .filter(|utxo| utxo.confirmation_height.saturating_add(delay) <= next_height)
        .cloned()
        .collect()
}

#[derive(Debug, Serialize)]
pub struct RecoveryTiming {
    pub delay_blocks: u64,
    pub earliest_next_block_height: Option<u64>,
    pub blocks_remaining: Option<u64>,
    pub total_sats: u64,
    pub mature_sats: u64,
}

/// Report the earliest live recovery window, not the age of a newer unrelated deposit.
pub fn recovery_timing(
    utxos: &[VaultUtxo],
    path: SweepPath,
    tip_height: u64,
) -> Result<RecoveryTiming> {
    let delay_blocks = path.delay().unwrap_or(0);
    let next = tip_height.saturating_add(1);
    let earliest_next_block_height = utxos
        .iter()
        .map(|utxo| utxo.confirmation_height.saturating_add(delay_blocks))
        .min();
    let mut total_sats = 0_u64;
    let mut mature_sats = 0_u64;
    for utxo in utxos {
        let amount = utxo.txout.value.to_sat();
        total_sats = total_sats
            .checked_add(amount)
            .context("recovery balance overflow")?;
        if utxo.confirmation_height.saturating_add(delay_blocks) <= next {
            mature_sats = mature_sats
                .checked_add(amount)
                .context("mature recovery balance overflow")?;
        }
    }
    Ok(RecoveryTiming {
        delay_blocks,
        earliest_next_block_height,
        blocks_remaining: earliest_next_block_height.map(|height| height.saturating_sub(next)),
        total_sats,
        mature_sats,
    })
}

/// Review mature savings, or the oldest cohort if none is mature yet. Keep each request
/// bounded and prioritize value so a stream of small deposits cannot block larger savings.
pub fn recovery_review_inputs(
    utxos: &[VaultUtxo],
    path: SweepPath,
    tip_height: u64,
) -> Vec<VaultUtxo> {
    let mut selected = mature_utxos(utxos, path, tip_height);
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
    selected
}

#[cfg(test)]
mod tests {
    use super::*;
    use bitcoin::{Amount, OutPoint, ScriptBuf, TxOut};

    #[test]
    fn recovery_maturity_uses_next_block_height() {
        let utxo = VaultUtxo {
            outpoint: OutPoint::null(),
            txout: TxOut {
                value: Amount::from_sat(1),
                script_pubkey: ScriptBuf::new(),
            },
            confirmation_height: 100,
        };
        assert!(
            mature_utxos(
                std::slice::from_ref(&utxo),
                SweepPath::PhoneRecovery,
                61_298
            )
            .is_empty()
        );
        assert_eq!(
            mature_utxos(
                std::slice::from_ref(&utxo),
                SweepPath::PhoneRecovery,
                61_299
            )
            .len(),
            1
        );
        assert!(
            mature_utxos(std::slice::from_ref(&utxo), SweepPath::HwwRecovery, 61_299).is_empty()
        );
        assert_eq!(
            mature_utxos(&[utxo], SweepPath::HwwRecovery, 65_634).len(),
            1
        );
    }

    #[test]
    fn recovery_reviews_are_bounded_and_newer_deposits_cannot_postpone_mature_savings() {
        use bitcoin::hashes::Hash;
        let mut inputs = (0..1002)
            .map(|vout| VaultUtxo {
                outpoint: OutPoint {
                    txid: bitcoin::Txid::all_zeros(),
                    vout,
                },
                txout: TxOut {
                    value: Amount::from_sat(u64::from(vout) + 1),
                    script_pubkey: ScriptBuf::new(),
                },
                confirmation_height: if vout == 1001 { 200 } else { 100 },
            })
            .collect::<Vec<_>>();
        let timing = recovery_timing(&inputs, SweepPath::HwwRecovery, 65_633).unwrap();
        assert_eq!(timing.blocks_remaining, Some(1));
        assert_eq!(timing.mature_sats, 0);
        let reviewed = recovery_review_inputs(&inputs, SweepPath::HwwRecovery, 65_633);
        assert_eq!(reviewed.len(), 1000);
        assert_eq!(reviewed[0].outpoint.vout, 1000);
        assert!(reviewed.iter().all(|utxo| utxo.confirmation_height == 100));
        let timing = recovery_timing(&inputs, SweepPath::HwwRecovery, 65_634).unwrap();
        assert_eq!(timing.blocks_remaining, Some(0));
        assert_eq!(timing.total_sats - timing.mature_sats, 1002);
        assert_eq!(
            recovery_review_inputs(&inputs, SweepPath::HwwRecovery, 65_634)[0]
                .outpoint
                .vout,
            1000
        );
        inputs.reverse();
        assert_eq!(
            recovery_review_inputs(&inputs, SweepPath::HwwRecovery, 65_634)[0]
                .outpoint
                .vout,
            1000
        );
        assert_eq!(
            recovery_timing(&[], SweepPath::HwwRecovery, 65_634)
                .unwrap()
                .blocks_remaining,
            None
        );
        inputs[0].txout.value = Amount::from_sat(u64::MAX);
        assert!(recovery_timing(&inputs, SweepPath::HwwRecovery, 65_634).is_err());
    }

    #[test]
    fn friend_emergency_access_still_obeys_the_phone_recovery_delay() {
        use anzen_common::{
            PHONE_RECOVERY_BLOCKS,
            social::{RecoveryPayload, create_backup, decrypt_with_friend, generate_friend_key},
            storage::{HWW_DEVICE_FILE, PHONE_DEVICE_FILE, load_device_keys},
        };
        use bitcoin::{Address, Txid, hashes::Hash};
        use std::str::FromStr;

        let dir = anzen_common::storage::tempdir().unwrap();
        let config = crate::test_support::initialize(dir.path()).unwrap().config;
        let network = config.bitcoin_network().unwrap();
        let phone = load_device_keys(dir.path(), PHONE_DEVICE_FILE).unwrap();
        let hww = load_device_keys(dir.path(), HWW_DEVICE_FILE).unwrap();
        let friend = generate_friend_key("Alice").unwrap();
        let backup = create_backup(
            &RecoveryPayload::new(&config, &phone).unwrap(),
            &hww.seed,
            &[friend.public_key_armored],
        )
        .unwrap();
        let recovered =
            decrypt_with_friend(&backup, friend.private_key_armored.as_bytes()).unwrap();
        let recovered_phone = DeviceKeys::parse_for_network_at_index(
            &bitcoin::key::Secp256k1::new(),
            &recovered.phone_mnemonic,
            network,
            recovered.phone_vault_key_index,
        )
        .unwrap();
        let destination = Address::from_str(&config.vault_address)
            .unwrap()
            .require_network(network)
            .unwrap();
        let utxo = VaultUtxo {
            outpoint: OutPoint::new(Txid::all_zeros(), 0),
            txout: TxOut {
                value: Amount::from_sat(1_000_000),
                script_pubkey: destination.script_pubkey(),
            },
            confirmation_height: 1,
        };
        let early = recover(
            &config,
            std::slice::from_ref(&utxo),
            u64::from(PHONE_RECOVERY_BLOCKS) - 1,
            SweepPath::PhoneRecovery,
            &destination,
            &recovered_phone,
        );
        assert!(early.is_err());
        let (_, result) = recover(
            &config,
            &[utxo],
            u64::from(PHONE_RECOVERY_BLOCKS),
            SweepPath::PhoneRecovery,
            &destination,
            &recovered_phone,
        )
        .unwrap();
        assert_eq!(result.input_count, 1);
    }
}
