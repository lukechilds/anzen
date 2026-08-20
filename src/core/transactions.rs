use super::{
    policy::{ControllerPath, ControllerPolicy, SpendPath, VaultPolicy},
    types::VaultUtxo,
};
use anyhow::{Context, Result, bail};
use bitcoin::{
    Amount, Psbt, ScriptBuf, Sequence, Transaction, TxIn, TxOut, Witness, absolute,
    hashes::Hash,
    key::Secp256k1,
    psbt::PsbtSighashType,
    secp256k1::{All, Keypair, Message, XOnlyPublicKey},
    sighash::{SighashCache, TapSighashType},
    taproot,
    transaction::Version,
};
use miniscript::psbt::{PsbtExt, PsbtSighashMsg};
use std::collections::BTreeSet;

pub fn create_vault_psbt(
    transaction: Transaction,
    prevouts: &[TxOut],
    policy: &VaultPolicy,
) -> Result<Psbt> {
    if transaction.input.len() != prevouts.len() {
        bail!(
            "transaction has {} inputs but {} previous outputs were provided",
            transaction.input.len(),
            prevouts.len()
        );
    }
    let mut psbt = Psbt::from_unsigned_tx(transaction)?;
    let descriptor = policy.definite_descriptor()?;
    for (index, prevout) in prevouts.iter().enumerate() {
        psbt.inputs[index].witness_utxo = Some(prevout.clone());
        psbt.inputs[index].sighash_type = Some(PsbtSighashType::from(TapSighashType::Default));
        psbt.update_input_with_descriptor(index, &descriptor)
            .with_context(|| format!("failed to add vault descriptor to PSBT input {index}"))?;
    }
    Ok(psbt)
}

/// Create a PSBT whose inputs are partitioned between the 2-of-2 vault policy and the fixed
/// 1-of-2 connector policy. Every input must belong to exactly one partition.
pub fn create_policy_psbt(
    transaction: Transaction,
    prevouts: &[TxOut],
    vault_input_indexes: &[usize],
    controller_input_indexes: &[usize],
    vault: &VaultPolicy,
    controller: &ControllerPolicy,
) -> Result<Psbt> {
    if transaction.input.len() != prevouts.len() {
        bail!(
            "transaction has {} inputs but {} previous outputs were provided",
            transaction.input.len(),
            prevouts.len()
        );
    }
    let vault_indexes = vault_input_indexes.iter().copied().collect::<BTreeSet<_>>();
    let controller_indexes = controller_input_indexes
        .iter()
        .copied()
        .collect::<BTreeSet<_>>();
    if vault_indexes.len() != vault_input_indexes.len()
        || controller_indexes.len() != controller_input_indexes.len()
        || !vault_indexes.is_disjoint(&controller_indexes)
        || vault_indexes.len() + controller_indexes.len() != transaction.input.len()
        || vault_indexes
            .iter()
            .chain(&controller_indexes)
            .any(|index| *index >= transaction.input.len())
    {
        bail!("policy PSBT input roles do not form an exact input partition");
    }

    let mut psbt = Psbt::from_unsigned_tx(transaction)?;
    let vault_descriptor = vault.definite_descriptor()?;
    let controller_descriptor = controller.definite_descriptor()?;
    for (index, prevout) in prevouts.iter().enumerate() {
        psbt.inputs[index].witness_utxo = Some(prevout.clone());
        psbt.inputs[index].sighash_type = Some(PsbtSighashType::from(TapSighashType::Default));
        if vault_indexes.contains(&index) {
            psbt.update_input_with_descriptor(index, &vault_descriptor)
                .with_context(|| format!("failed to add vault descriptor to PSBT input {index}"))?;
        } else {
            psbt.update_input_with_descriptor(index, &controller_descriptor)
                .with_context(|| {
                    format!("failed to add controller descriptor to PSBT input {index}")
                })?;
        }
    }
    Ok(psbt)
}

/// Create a PSBT whose every input is controlled by the fixed 1-of-2 controller policy.
pub fn create_controller_psbt(
    transaction: Transaction,
    prevouts: &[TxOut],
    controller: &ControllerPolicy,
) -> Result<Psbt> {
    if transaction.input.len() != prevouts.len() {
        bail!(
            "transaction has {} inputs but {} previous outputs were provided",
            transaction.input.len(),
            prevouts.len()
        );
    }
    let mut psbt = Psbt::from_unsigned_tx(transaction)?;
    let descriptor = controller.definite_descriptor()?;
    for (index, prevout) in prevouts.iter().enumerate() {
        psbt.inputs[index].witness_utxo = Some(prevout.clone());
        psbt.inputs[index].sighash_type = Some(PsbtSighashType::from(TapSighashType::Default));
        psbt.update_input_with_descriptor(index, &descriptor)
            .with_context(|| {
                format!("failed to add controller descriptor to PSBT input {index}")
            })?;
    }
    Ok(psbt)
}

/// Build an immediate revocation that consumes live controller states and sends their remainder
/// to a normal wallet address. It deliberately creates no new controller output: once this
/// transaction confirms, every presigned policy transaction committed to any consumed outpoint is
/// permanently invalid.
pub fn build_controller_revocation_psbt(
    controller_utxos: &[VaultUtxo],
    destination: ScriptBuf,
    fee_rate_sat_vb: u64,
    controller: &ControllerPolicy,
) -> Result<(Psbt, u64)> {
    if controller_utxos.is_empty() {
        bail!("no live controller outputs were provided for revocation");
    }
    let controller_script = controller.address.script_pubkey();
    if controller_utxos
        .iter()
        .any(|utxo| utxo.txout.script_pubkey != controller_script)
    {
        bail!("revocation input is outside the fixed controller policy");
    }
    let total_input = controller_utxos.iter().try_fold(0_u64, |total, utxo| {
        total
            .checked_add(utxo.txout.value.to_sat())
            .context("controller revocation input total overflowed")
    })?;
    let inputs = controller_utxos
        .iter()
        .map(|utxo| TxIn {
            previous_output: utxo.outpoint,
            script_sig: ScriptBuf::new(),
            sequence: Sequence::ENABLE_RBF_NO_LOCKTIME,
            witness: Witness::new(),
        })
        .collect::<Vec<_>>();
    let template = Transaction {
        version: Version::TWO,
        lock_time: absolute::LockTime::ZERO,
        input: inputs.clone(),
        output: vec![TxOut {
            value: Amount::from_sat(1),
            script_pubkey: destination.clone(),
        }],
    };
    let fee_sats = estimate_controller_vsize(&template, controller, ControllerPath::Phone)?
        .checked_mul(fee_rate_sat_vb)
        .context("controller revocation fee overflowed")?;
    let change_sats = total_input
        .checked_sub(fee_sats)
        .context("controller outputs cannot pay the revocation fee")?;
    if change_sats < destination.minimal_non_dust().to_sat() {
        bail!("controller revocation would create dust change");
    }
    let transaction = Transaction {
        version: Version::TWO,
        lock_time: absolute::LockTime::ZERO,
        input: inputs,
        output: vec![TxOut {
            value: Amount::from_sat(change_sats),
            script_pubkey: destination,
        }],
    };
    let prevouts = controller_utxos
        .iter()
        .map(|utxo| utxo.txout.clone())
        .collect::<Vec<_>>();
    Ok((
        create_controller_psbt(transaction, &prevouts, controller)?,
        fee_sats,
    ))
}

pub fn sign_vault_psbt(
    psbt: &mut Psbt,
    policy: &VaultPolicy,
    path: SpendPath,
    keypair: &Keypair,
) -> Result<()> {
    let indexes = (0..psbt.inputs.len()).collect::<Vec<_>>();
    sign_vault_psbt_inputs(psbt, policy, path, keypair, &indexes)
}

pub fn sign_vault_psbt_inputs(
    psbt: &mut Psbt,
    policy: &VaultPolicy,
    path: SpendPath,
    keypair: &Keypair,
    input_indexes: &[usize],
) -> Result<()> {
    let secp = Secp256k1::new();
    let (signing_pubkey, _) = XOnlyPublicKey::from_keypair(keypair);
    let leaf = policy.leaf(path)?;

    for &index in input_indexes {
        if index >= psbt.inputs.len() {
            bail!("vault signing input index {index} is out of range");
        }
        let origins = psbt.inputs[index]
            .tap_key_origins
            .get(&signing_pubkey)
            .with_context(|| {
                format!("signing key {signing_pubkey} is not in PSBT input {index}")
            })?;
        if !origins.0.contains(&leaf.leaf_hash) {
            bail!("signing key {signing_pubkey} is not authorized by the {path:?} leaf");
        }

        let unsigned_tx = psbt.unsigned_tx.clone();
        let mut cache = SighashCache::new(&unsigned_tx);
        let message = match psbt.sighash_msg(index, &mut cache, Some(leaf.leaf_hash))? {
            PsbtSighashMsg::TapSighash(sighash) => Message::from_digest(sighash.to_byte_array()),
            _ => bail!("vault input {index} did not produce a Taproot sighash"),
        };
        let signature = secp.sign_schnorr_no_aux_rand(&message, keypair);
        psbt.inputs[index].tap_script_sigs.insert(
            (signing_pubkey, leaf.leaf_hash),
            taproot::Signature {
                signature,
                sighash_type: TapSighashType::Default,
            },
        );
    }
    Ok(())
}

pub fn sign_controller_psbt_inputs(
    psbt: &mut Psbt,
    policy: &ControllerPolicy,
    path: ControllerPath,
    keypair: &Keypair,
    input_indexes: &[usize],
) -> Result<()> {
    let secp = Secp256k1::new();
    let (signing_pubkey, _) = XOnlyPublicKey::from_keypair(keypair);
    let leaf = policy.leaf(path)?;
    for &index in input_indexes {
        if index >= psbt.inputs.len() {
            bail!("controller signing input index {index} is out of range");
        }
        let origins = psbt.inputs[index]
            .tap_key_origins
            .get(&signing_pubkey)
            .with_context(|| {
                format!("controller signing key {signing_pubkey} is not in PSBT input {index}")
            })?;
        if !origins.0.contains(&leaf.leaf_hash) {
            bail!("signing key {signing_pubkey} is not authorized by the {path:?} controller leaf");
        }
        let unsigned_tx = psbt.unsigned_tx.clone();
        let mut cache = SighashCache::new(&unsigned_tx);
        let message = match psbt.sighash_msg(index, &mut cache, Some(leaf.leaf_hash))? {
            PsbtSighashMsg::TapSighash(sighash) => Message::from_digest(sighash.to_byte_array()),
            _ => bail!("controller input {index} did not produce a Taproot sighash"),
        };
        let signature = secp.sign_schnorr_no_aux_rand(&message, keypair);
        psbt.inputs[index].tap_script_sigs.insert(
            (signing_pubkey, leaf.leaf_hash),
            taproot::Signature {
                signature,
                sighash_type: TapSighashType::Default,
            },
        );
    }
    Ok(())
}

pub fn verify_vault_psbt_signature(
    psbt: &Psbt,
    policy: &VaultPolicy,
    path: SpendPath,
    signing_pubkey: XOnlyPublicKey,
) -> Result<()> {
    let indexes = (0..psbt.inputs.len()).collect::<Vec<_>>();
    verify_vault_psbt_signatures(psbt, policy, path, signing_pubkey, &indexes)
}

pub fn verify_vault_psbt_signatures(
    psbt: &Psbt,
    policy: &VaultPolicy,
    path: SpendPath,
    signing_pubkey: XOnlyPublicKey,
    input_indexes: &[usize],
) -> Result<()> {
    let secp = Secp256k1::verification_only();
    let leaf = policy.leaf(path)?;
    for &index in input_indexes {
        if index >= psbt.inputs.len() {
            bail!("vault verification input index {index} is out of range");
        }
        let signature = psbt.inputs[index]
            .tap_script_sigs
            .get(&(signing_pubkey, leaf.leaf_hash))
            .with_context(|| {
                format!("PSBT input {index} lacks the expected signature from {signing_pubkey}")
            })?;
        if signature.sighash_type != TapSighashType::Default {
            bail!("PSBT input {index} uses a non-default Taproot sighash");
        }
        let unsigned_tx = psbt.unsigned_tx.clone();
        let mut cache = SighashCache::new(&unsigned_tx);
        let message = match psbt.sighash_msg(index, &mut cache, Some(leaf.leaf_hash))? {
            PsbtSighashMsg::TapSighash(sighash) => Message::from_digest(sighash.to_byte_array()),
            _ => bail!("vault input {index} did not produce a Taproot sighash"),
        };
        secp.verify_schnorr(&signature.signature, &message, &signing_pubkey)
            .with_context(|| format!("invalid vault signature on PSBT input {index}"))?;
    }
    Ok(())
}

pub fn verify_controller_psbt_signatures(
    psbt: &Psbt,
    policy: &ControllerPolicy,
    path: ControllerPath,
    signing_pubkey: XOnlyPublicKey,
    input_indexes: &[usize],
) -> Result<()> {
    let secp = Secp256k1::verification_only();
    let leaf = policy.leaf(path)?;
    for &index in input_indexes {
        if index >= psbt.inputs.len() {
            bail!("controller verification input index {index} is out of range");
        }
        let signature = psbt.inputs[index]
            .tap_script_sigs
            .get(&(signing_pubkey, leaf.leaf_hash))
            .with_context(|| {
                format!("PSBT input {index} lacks the expected controller signature from {signing_pubkey}")
            })?;
        if signature.sighash_type != TapSighashType::Default {
            bail!("PSBT input {index} uses a non-default Taproot sighash");
        }
        let unsigned_tx = psbt.unsigned_tx.clone();
        let mut cache = SighashCache::new(&unsigned_tx);
        let message = match psbt.sighash_msg(index, &mut cache, Some(leaf.leaf_hash))? {
            PsbtSighashMsg::TapSighash(sighash) => Message::from_digest(sighash.to_byte_array()),
            _ => bail!("controller input {index} did not produce a Taproot sighash"),
        };
        secp.verify_schnorr(&signature.signature, &message, &signing_pubkey)
            .with_context(|| format!("invalid controller signature on PSBT input {index}"))?;
    }
    Ok(())
}

pub fn finalize_vault_psbt(mut psbt: Psbt) -> Result<Transaction> {
    let secp = Secp256k1::verification_only();
    psbt.finalize_mut(&secp)
        .map_err(|errors| anyhow::anyhow!("unable to finalize vault PSBT: {errors:?}"))?;
    psbt.extract(&secp)
        .context("unable to extract finalized vault transaction")
}

pub fn signed_vsize(transaction: &Transaction) -> u64 {
    transaction.vsize() as u64
}

pub fn estimate_vault_vsize(
    transaction: &Transaction,
    policy: &VaultPolicy,
    path: SpendPath,
) -> Result<u64> {
    let leaf = policy.leaf(path)?;
    let signature_count = match path {
        SpendPath::Cooperative => 2,
        SpendPath::PhoneRecovery | SpendPath::HwwRecovery => 1,
    };
    let mut estimated = transaction.clone();
    for input in &mut estimated.input {
        let mut witness = Witness::new();
        for _ in 0..signature_count {
            witness.push([0_u8; 64]);
        }
        witness.push(leaf.script.as_bytes());
        witness.push(leaf.control_block.serialize());
        input.witness = witness;
    }
    Ok(estimated.vsize() as u64)
}

pub fn estimate_policy_vsize(
    transaction: &Transaction,
    vault: &VaultPolicy,
    vault_path: SpendPath,
    vault_input_indexes: &[usize],
    controller: &ControllerPolicy,
    controller_path: ControllerPath,
    controller_input_indexes: &[usize],
) -> Result<u64> {
    let vault_leaf = vault.leaf(vault_path)?;
    let controller_leaf = controller.leaf(controller_path)?;
    let vault_signature_count = match vault_path {
        SpendPath::Cooperative => 2,
        SpendPath::PhoneRecovery | SpendPath::HwwRecovery => 1,
    };
    let mut estimated = transaction.clone();
    let vault_indexes = vault_input_indexes.iter().copied().collect::<BTreeSet<_>>();
    let controller_indexes = controller_input_indexes
        .iter()
        .copied()
        .collect::<BTreeSet<_>>();
    if !vault_indexes.is_disjoint(&controller_indexes)
        || vault_indexes.len() + controller_indexes.len() != estimated.input.len()
    {
        bail!("policy vsize input roles do not form an exact input partition");
    }
    for (index, input) in estimated.input.iter_mut().enumerate() {
        let mut witness = Witness::new();
        if vault_indexes.contains(&index) {
            for _ in 0..vault_signature_count {
                witness.push([0_u8; 64]);
            }
            witness.push(vault_leaf.script.as_bytes());
            witness.push(vault_leaf.control_block.serialize());
        } else if controller_indexes.contains(&index) {
            witness.push([0_u8; 64]);
            witness.push(controller_leaf.script.as_bytes());
            witness.push(controller_leaf.control_block.serialize());
        } else {
            bail!("policy vsize input {index} has no role");
        }
        input.witness = witness;
    }
    Ok(estimated.vsize() as u64)
}

pub fn estimate_controller_vsize(
    transaction: &Transaction,
    controller: &ControllerPolicy,
    path: ControllerPath,
) -> Result<u64> {
    let leaf = controller.leaf(path)?;
    let mut estimated = transaction.clone();
    for input in &mut estimated.input {
        let mut witness = Witness::new();
        witness.push([0_u8; 64]);
        witness.push(leaf.script.as_bytes());
        witness.push(leaf.control_block.serialize());
        input.witness = witness;
    }
    Ok(estimated.vsize() as u64)
}

pub fn witness_for_path(
    policy: &VaultPolicy,
    path: SpendPath,
    signatures_in_script_order: &[taproot::Signature],
) -> Result<Witness> {
    let leaf = policy.leaf(path)?;
    let mut witness = Witness::new();
    for signature in signatures_in_script_order.iter().rev() {
        witness.push(signature.to_vec());
    }
    witness.push(leaf.script.as_bytes());
    witness.push(leaf.control_block.serialize());
    Ok(witness)
}

pub fn keypair_pubkey(keypair: &Keypair, _secp: &Secp256k1<All>) -> XOnlyPublicKey {
    XOnlyPublicKey::from_keypair(keypair).0
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::{
        CONNECTOR_VALUE_SATS, HWW_RECOVERY_BLOCKS, PHONE_RECOVERY_BLOCKS, keys::DeviceKeys,
    };
    use bitcoin::{Amount, OutPoint, ScriptBuf, Sequence, TxIn, absolute, transaction::Version};

    fn fixture(sequence: Sequence) -> (VaultPolicy, DeviceKeys, DeviceKeys, Psbt) {
        let secp = Secp256k1::new();
        let phone = DeviceKeys::generate(&secp).unwrap();
        let hww = DeviceKeys::generate(&secp).unwrap();
        let policy = VaultPolicy::new(phone.vault_pubkey, hww.vault_pubkey).unwrap();
        let prevout = TxOut {
            value: Amount::from_sat(20_000_000),
            script_pubkey: policy.address.script_pubkey(),
        };
        let tx = Transaction {
            version: Version::TWO,
            lock_time: absolute::LockTime::ZERO,
            input: vec![TxIn {
                previous_output: OutPoint::null(),
                script_sig: ScriptBuf::new(),
                sequence,
                witness: Witness::new(),
            }],
            output: vec![TxOut {
                value: Amount::from_sat(19_999_800),
                script_pubkey: ScriptBuf::new_p2tr(&secp, phone.vault_pubkey, None),
            }],
        };
        let psbt = create_vault_psbt(tx, &[prevout], &policy).unwrap();
        (policy, phone, hww, psbt)
    }

    #[test]
    fn cooperative_psbt_requires_and_accepts_both_signatures() {
        let (policy, phone, hww, mut psbt) = fixture(Sequence::MAX);
        sign_vault_psbt(
            &mut psbt,
            &policy,
            SpendPath::Cooperative,
            &phone.vault_keypair,
        )
        .unwrap();
        verify_vault_psbt_signature(&psbt, &policy, SpendPath::Cooperative, phone.vault_pubkey)
            .unwrap();
        assert!(finalize_vault_psbt(psbt.clone()).is_err());
        sign_vault_psbt(
            &mut psbt,
            &policy,
            SpendPath::Cooperative,
            &hww.vault_keypair,
        )
        .unwrap();
        let tx = finalize_vault_psbt(psbt).unwrap();
        assert_eq!(tx.input[0].witness.len(), 4);
        assert!(signed_vsize(&tx) > 0);
        assert_eq!(
            estimate_vault_vsize(&tx, &policy, SpendPath::Cooperative).unwrap(),
            signed_vsize(&tx)
        );
    }

    #[test]
    fn phone_recovery_finalizes_with_only_phone_signature() {
        let (policy, phone, _hww, mut psbt) = fixture(Sequence(PHONE_RECOVERY_BLOCKS.into()));
        sign_vault_psbt(
            &mut psbt,
            &policy,
            SpendPath::PhoneRecovery,
            &phone.vault_keypair,
        )
        .unwrap();
        let tx = finalize_vault_psbt(psbt).unwrap();
        assert_eq!(tx.input[0].sequence, Sequence(PHONE_RECOVERY_BLOCKS.into()));
        assert_eq!(tx.input[0].witness.len(), 3);
    }

    #[test]
    fn hww_recovery_finalizes_with_only_hww_signature() {
        let (policy, _phone, hww, mut psbt) = fixture(Sequence(HWW_RECOVERY_BLOCKS.into()));
        sign_vault_psbt(
            &mut psbt,
            &policy,
            SpendPath::HwwRecovery,
            &hww.vault_keypair,
        )
        .unwrap();
        let tx = finalize_vault_psbt(psbt).unwrap();
        assert_eq!(tx.input[0].sequence, Sequence(HWW_RECOVERY_BLOCKS.into()));
        assert_eq!(tx.input[0].witness.len(), 3);
    }

    #[test]
    fn policy_action_stays_incomplete_until_a_controller_signature_is_added() {
        let secp = Secp256k1::new();
        let phone = DeviceKeys::generate(&secp).unwrap();
        let hww = DeviceKeys::generate(&secp).unwrap();
        let vault = VaultPolicy::new(phone.vault_pubkey, hww.vault_pubkey).unwrap();
        let controller = ControllerPolicy::new(phone.vault_pubkey, hww.vault_pubkey).unwrap();
        let prevouts = vec![
            TxOut {
                value: Amount::from_sat(1_000_000),
                script_pubkey: vault.address.script_pubkey(),
            },
            TxOut {
                value: Amount::from_sat(CONNECTOR_VALUE_SATS),
                script_pubkey: controller.address.script_pubkey(),
            },
        ];
        let tx = Transaction {
            version: Version::TWO,
            lock_time: absolute::LockTime::ZERO,
            input: vec![
                TxIn {
                    previous_output: OutPoint::null(),
                    script_sig: ScriptBuf::new(),
                    sequence: Sequence::MAX,
                    witness: Witness::new(),
                },
                TxIn {
                    previous_output: OutPoint::new(bitcoin::Txid::all_zeros(), 1),
                    script_sig: ScriptBuf::new(),
                    sequence: Sequence::ENABLE_RBF_NO_LOCKTIME,
                    witness: Witness::new(),
                },
            ],
            output: vec![TxOut {
                value: Amount::from_sat(1_009_000),
                script_pubkey: ScriptBuf::new_p2tr(&secp, phone.vault_pubkey, None),
            }],
        };
        let mut psbt = create_policy_psbt(tx, &prevouts, &[0], &[1], &vault, &controller).unwrap();
        sign_vault_psbt_inputs(
            &mut psbt,
            &vault,
            SpendPath::Cooperative,
            &phone.vault_keypair,
            &[0],
        )
        .unwrap();
        sign_vault_psbt_inputs(
            &mut psbt,
            &vault,
            SpendPath::Cooperative,
            &hww.vault_keypair,
            &[0],
        )
        .unwrap();
        assert!(finalize_vault_psbt(psbt.clone()).is_err());
        assert!(
            sign_controller_psbt_inputs(
                &mut psbt.clone(),
                &controller,
                ControllerPath::Phone,
                &phone.vault_keypair,
                &[0],
            )
            .is_err()
        );
        sign_controller_psbt_inputs(
            &mut psbt,
            &controller,
            ControllerPath::Phone,
            &phone.vault_keypair,
            &[1],
        )
        .unwrap();
        let finalized = finalize_vault_psbt(psbt).unwrap();
        assert_eq!(finalized.input[0].witness.len(), 4);
        assert_eq!(finalized.input[1].witness.len(), 3);
    }

    #[test]
    fn either_device_can_revoke_a_connector_and_change_never_recreates_controller_state() {
        let secp = Secp256k1::new();
        let phone = DeviceKeys::generate(&secp).unwrap();
        let hww = DeviceKeys::generate(&secp).unwrap();
        let controller = ControllerPolicy::new(phone.vault_pubkey, hww.vault_pubkey).unwrap();
        let destination = ScriptBuf::new_p2tr(&secp, phone.vault_pubkey, None);
        let utxo = VaultUtxo {
            outpoint: OutPoint::null(),
            txout: TxOut {
                value: Amount::from_sat(CONNECTOR_VALUE_SATS),
                script_pubkey: controller.address.script_pubkey(),
            },
            confirmation_height: 1,
        };
        let (unsigned, fee_sats) = build_controller_revocation_psbt(
            std::slice::from_ref(&utxo),
            destination.clone(),
            1,
            &controller,
        )
        .unwrap();
        assert_eq!(
            unsigned.unsigned_tx.input[0].sequence,
            Sequence::ENABLE_RBF_NO_LOCKTIME
        );
        assert_eq!(unsigned.unsigned_tx.output.len(), 1);
        assert_eq!(unsigned.unsigned_tx.output[0].script_pubkey, destination);
        assert_ne!(
            unsigned.unsigned_tx.output[0].script_pubkey,
            controller.address.script_pubkey()
        );
        assert_eq!(
            unsigned.unsigned_tx.output[0].value.to_sat() + fee_sats,
            CONNECTOR_VALUE_SATS
        );
        for (path, keypair) in [
            (ControllerPath::Phone, &phone.vault_keypair),
            (ControllerPath::Hww, &hww.vault_keypair),
        ] {
            let mut psbt = unsigned.clone();
            sign_controller_psbt_inputs(&mut psbt, &controller, path, keypair, &[0]).unwrap();
            assert!(finalize_vault_psbt(psbt).is_ok());
        }
    }
}
