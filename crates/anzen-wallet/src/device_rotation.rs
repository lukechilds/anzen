//! Phone-side completion of a device rotation: rebuild what the signer approved, verify its
//! signatures, finish the old vault's joint sweep signatures and finalize the old-epoch
//! transactions.

use crate::{
    cosigning,
    transactions::{
        attach_key_path_signatures, attach_signatures, finalize_vault_psbt, protocol_psbt,
    },
};
use anyhow::{Context, Result, bail, ensure};
use anzen_common::{
    device_rotation::{ApprovedDevicePhoneRotation, DevicePhoneRotation},
    keys::DeviceKeys,
    policy::{ControllerPolicy, VaultPolicy},
    protocol::{self, SoftwareCrypto},
    storage::VaultConfig,
};
use anzen_protocol::wire::{self, PolicyApproval};
use bitcoin::{Transaction, hashes::Hash, secp256k1::XOnlyPublicKey, taproot::TapLeafHash};
use std::str::FromStr;

/// Everything a rotation approval lets the phone broadcast, in order.
pub struct FinalizedRotation {
    pub revocation: Option<Transaction>,
    pub sweep: Transaction,
    pub renewed_policy: Option<PolicyApproval>,
}

/// Rebuild what the signer approved, verify its signatures, finish the old vault's joint sweep
/// signatures with the old phone key and finalize the old-epoch transactions.
pub fn finalize(
    data_dir: &std::path::Path,
    request: &DevicePhoneRotation,
    old: &VaultConfig,
    approved: &ApprovedDevicePhoneRotation,
    old_phone: &DeviceKeys,
) -> Result<FinalizedRotation> {
    request.validate(old)?;
    let approval = &approved.approval;
    ensure!(
        approved.request_id == request.id()?
            && approval.version == wire::VERSION
            && approval.request == request.rotation
            && serde_json::to_vec(&approved.new_config)?
                == serde_json::to_vec(&request.new_config)?,
        "rotation approval belongs to another request"
    );
    ensure!(
        old_phone.vault_pubkey.to_string() == old.phone_vault_pubkey,
        "rotation must be signed by the old phone key"
    );
    let plan = protocol::protocol(request.rotation.build(&mut SoftwareCrypto::new()))?;
    let network = old.bitcoin_network()?;
    let vault = VaultPolicy::from_descriptor_for_network(&old.vault_descriptor, network)?;
    let hww = XOnlyPublicKey::from_str(&old.hww_vault_pubkey)?;
    let controller = ControllerPolicy::new_for_network(old_phone.vault_pubkey, hww, network)?;

    let revocation = match &plan.revocation {
        Some(spend) => {
            let mut psbt = protocol_psbt(
                &spend.transaction,
                &spend.prevouts,
                &spend.spends,
                &vault,
                &controller,
            )?;
            attach_signatures(
                &mut psbt,
                &(0..spend.transaction.input.len()).collect::<Vec<_>>(),
                hww,
                TapLeafHash::from_byte_array(plan.old_scripts.controller.hww_leaf),
                &approval.revocation_signatures,
            )
            .context("signer changed connector revocation")?;
            Some(finalize_vault_psbt(psbt)?)
        }
        None if approval.revocation_signatures.is_empty() => None,
        None => bail!("signer changed whether connectors are revoked"),
    };

    let mut sweep = protocol_psbt(
        &plan.sweep.transaction,
        &plan.sweep.prevouts,
        &plan.sweep.spends,
        &vault,
        &controller,
    )?;
    let signatures = cosigning::Phone {
        data_dir,
        keys: old_phone,
    }
    .finish(
        &cosigning::spend_jobs(&plan.old_scripts, &plan.sweep)?,
        &request.rotation.sweep_nonces,
        &approval.sweep_signatures,
    )
    .context("signer changed the proposed rotation")?;
    attach_key_path_signatures(
        &mut sweep,
        &(0..plan.sweep.transaction.input.len()).collect::<Vec<_>>(),
        XOnlyPublicKey::from_slice(&plan.old_scripts.vault.output.output_key)?,
        &signatures,
    )?;
    let sweep = finalize_vault_psbt(sweep)?;

    let renewed_policy = match &request.rotation.renewed_policy {
        Some(request) => Some(PolicyApproval {
            version: wire::VERSION,
            request: request.clone(),
            signatures: approval.policy_signatures.clone(),
        }),
        None if approval.policy_signatures.is_empty() => None,
        None => bail!("signer changed whether the renewed policy is enabled"),
    };
    Ok(FinalizedRotation {
        revocation,
        sweep,
        renewed_policy,
    })
}
