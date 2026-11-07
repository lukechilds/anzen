//! Actions a device takes alone: HWW recovery after its delay, and connector revocation.
//!
//! Requests carry outpoints and values only. `SIGHASH_DEFAULT` commits to every input amount,
//! so the signer's review cannot be satisfied on chain by misreported values. The signer does
//! not claim maturity or unspent status; Bitcoin enforces the recovery sequence.

use crate::transactions::{
    attach_signatures, finalize_vault_psbt, protocol_psbt, sign_controller_psbt_inputs,
};
use anyhow::{Context, Result, bail};
use anzen_common::{
    keys::DeviceKeys,
    policy::{ControllerPath, ControllerPolicy, VaultPolicy},
    protocol::{self, SoftwareCrypto},
    storage::VaultConfig,
    types::VaultUtxo,
};
pub use anzen_protocol::wire::{OfflineAction, OfflineActionApproval, OfflineActionRequest};
use anzen_protocol::{
    script::Scripts,
    spend::{self, Spend},
    tx::SpendPath,
    wire,
};
use bitcoin::{
    Address, Psbt, Transaction, hashes::Hash, secp256k1::XOnlyPublicKey, taproot::TapLeafHash,
};
use std::str::FromStr;

fn policies(config: &VaultConfig) -> Result<(Scripts, VaultPolicy, ControllerPolicy)> {
    config.validate_public_policy()?;
    let network = config.bitcoin_network()?;
    let scripts = protocol::protocol(Scripts::new(
        &mut SoftwareCrypto::new(),
        &protocol::vault_keys(config)?,
    ))?;
    Ok((
        scripts,
        VaultPolicy::from_descriptor_for_network(&config.vault_descriptor, network)?,
        ControllerPolicy::new_for_network(
            XOnlyPublicKey::from_str(&config.phone_vault_pubkey)?,
            XOnlyPublicKey::from_str(&config.hww_vault_pubkey)?,
            network,
        )?,
    ))
}

/// Parameters for the signer. Inputs must belong to the script the action spends.
pub fn request(
    config: &VaultConfig,
    action: OfflineAction,
    utxos: &[VaultUtxo],
    destination: &str,
) -> Result<OfflineActionRequest> {
    let (scripts, _, _) = policies(config)?;
    let expected = match action {
        OfflineAction::HwwRecovery => &scripts.vault.script_pubkey,
        OfflineAction::RevokeActions => &scripts.controller.script_pubkey,
    };
    if utxos
        .iter()
        .any(|utxo| utxo.txout.script_pubkey.as_bytes() != expected.as_slice())
    {
        bail!("offline action input does not belong to the expected vault path");
    }
    let request = OfflineActionRequest {
        version: wire::VERSION,
        network: protocol::config_network(config)?,
        keys: protocol::vault_keys(config)?,
        action,
        inputs: protocol::inputs(utxos),
        destination: destination.to_owned(),
    };
    protocol::protocol(request.build(&mut SoftwareCrypto::new()))
        .context("offline action cannot create a non-dust destination output")?;
    Ok(request)
}

/// A host-assembled HWW recovery request over every vault output mature at the next block.
/// A computer connected to the signer supplies public chain data; the signer adds nothing.
pub fn mature_recovery_request(
    config: &VaultConfig,
    all_utxos: &[VaultUtxo],
    tip_height: u64,
    destination: &str,
) -> Result<OfflineActionRequest> {
    let mature = crate::recovery::mature_utxos(
        all_utxos,
        crate::recovery::SweepPath::HwwRecovery,
        tip_height,
    );
    if mature.is_empty() {
        match all_utxos.iter().map(|utxo| utxo.confirmation_height).min() {
            Some(oldest) => {
                let valid = oldest.saturating_add(u64::from(anzen_common::HWW_RECOVERY_BLOCKS));
                bail!(
                    "no vault UTXOs are mature for HWW recovery; earliest next-block validity height is {valid} ({} blocks remaining)",
                    valid.saturating_sub(tip_height.saturating_add(1))
                );
            }
            None => bail!("vault has no confirmed UTXOs to sweep"),
        }
    }
    request(config, OfflineAction::HwwRecovery, &mature, destination)
}

/// Rebuild an approved action, verify every signer signature and finalize it.
pub fn finalize(config: &VaultConfig, approval: &OfflineActionApproval) -> Result<Transaction> {
    let request = &approval.request;
    if approval.version != wire::VERSION
        || request.network != protocol::config_network(config)?
        || request.keys != protocol::vault_keys(config)?
    {
        bail!("offline action does not match the accepted vault");
    }
    let (_, vault, controller) = policies(config)?;
    let (_, spend, leaf) = protocol::protocol(request.build(&mut SoftwareCrypto::new()))?;
    let mut psbt = spend_psbt(&spend, &vault, &controller)?;
    attach_signatures(
        &mut psbt,
        &(0..spend.transaction.input.len()).collect::<Vec<_>>(),
        XOnlyPublicKey::from_str(&config.hww_vault_pubkey)?,
        TapLeafHash::from_byte_array(leaf),
        &approval.signatures,
    )
    .context("offline action approval contains an invalid signature")?;
    finalize_vault_psbt(psbt)
}

/// Spend connectors through the phone's controller leaf to a destination the phone chose.
pub fn phone_revocation(
    config: &VaultConfig,
    connectors: &[VaultUtxo],
    destination: &Address,
    phone: &DeviceKeys,
) -> Result<Transaction> {
    if phone.vault_pubkey.to_string() != config.phone_vault_pubkey {
        bail!("revocation key is not this vault's phone");
    }
    let (scripts, vault, controller) = policies(config)?;
    if connectors
        .iter()
        .any(|utxo| utxo.txout.script_pubkey.as_bytes() != scripts.controller.script_pubkey)
    {
        bail!("revocation input is outside the fixed controller policy");
    }
    // TODO(production): select current-feerate inputs and expose RBF/CPFP controls. The MVP
    // pays its fixed 1 sat/vB fee from the connector value.
    let spend = protocol::protocol(spend::controller_revocation(
        &mut SoftwareCrypto::new(),
        &scripts,
        &protocol::inputs(connectors),
        SpendPath::ControllerPhone,
        destination.script_pubkey().as_bytes(),
    ))
    .context("controller revocation would create dust change")?;
    let mut psbt = spend_psbt(&spend, &vault, &controller)?;
    sign_controller_psbt_inputs(
        &mut psbt,
        &controller,
        ControllerPath::Phone,
        &phone.vault_keypair,
        &(0..spend.transaction.input.len()).collect::<Vec<_>>(),
    )?;
    finalize_vault_psbt(psbt)
}

fn spend_psbt(spend: &Spend, vault: &VaultPolicy, controller: &ControllerPolicy) -> Result<Psbt> {
    protocol_psbt(
        &spend.transaction,
        &spend.prevouts,
        &spend.spends,
        vault,
        controller,
    )
}
