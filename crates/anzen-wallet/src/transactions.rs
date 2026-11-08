//! PSBT tooling for transactions built by `anzen_protocol`.
//!
//! The protocol crate decides every transaction and signature message. PSBTs only carry the
//! descriptor metadata Miniscript needs to finalize witnesses, plus the collected signatures.

use anyhow::{Context, Result, bail};
use anzen_common::{
    policy::{ControllerPath, ControllerPolicy, SpendPath, VaultPolicy},
    protocol,
};
use anzen_protocol::{tx, wire::Signature};
use bitcoin::{
    Psbt, Transaction, TxOut,
    hashes::Hash,
    key::Secp256k1,
    psbt::PsbtSighashType,
    secp256k1::{Keypair, Message, XOnlyPublicKey, schnorr},
    sighash::{Prevouts, SighashCache, TapSighashType},
    taproot::{self, TapLeafHash},
};
use miniscript::psbt::{PsbtExt, PsbtSighashMsg};
use std::collections::BTreeSet;

/// A PSBT for a protocol-built transaction. Each input gets the descriptor of its spend path.
pub fn protocol_psbt(
    transaction: &tx::Transaction,
    prevouts: &[tx::TxOut],
    spends: &[tx::SpendPath],
    vault: &VaultPolicy,
    controller: &ControllerPolicy,
) -> Result<Psbt> {
    let vault_indexes = spends
        .iter()
        .enumerate()
        .filter(|(_, spend)| !spend.is_controller())
        .map(|(index, _)| index)
        .collect::<Vec<_>>();
    let controller_indexes = spends
        .iter()
        .enumerate()
        .filter(|(_, spend)| spend.is_controller())
        .map(|(index, _)| index)
        .collect::<Vec<_>>();
    create_policy_psbt(
        protocol::transaction(transaction)?,
        &protocol::prevouts(prevouts),
        &vault_indexes,
        &controller_indexes,
        vault,
        controller,
    )
}

/// Verify another device's signatures through `leaf` and add them to the PSBT.
pub fn attach_signatures(
    psbt: &mut Psbt,
    input_indexes: &[usize],
    signer: XOnlyPublicKey,
    leaf: TapLeafHash,
    signatures: &[Signature],
) -> Result<()> {
    validate_default_sighashes(psbt)?;
    if input_indexes.len() != signatures.len() {
        bail!("signature count does not match the transaction inputs");
    }
    let prevouts = psbt
        .inputs
        .iter()
        .map(|input| {
            input
                .witness_utxo
                .clone()
                .context("PSBT input lacks its witness UTXO")
        })
        .collect::<Result<Vec<TxOut>>>()?;
    let secp = Secp256k1::verification_only();
    let unsigned = psbt.unsigned_tx.clone();
    let mut cache = SighashCache::new(&unsigned);
    for (&index, signature) in input_indexes.iter().zip(signatures) {
        let sighash = cache.taproot_script_spend_signature_hash(
            index,
            &Prevouts::All(&prevouts),
            leaf,
            TapSighashType::Default,
        )?;
        let signature = schnorr::Signature::from_slice(&signature.0)?;
        secp.verify_schnorr(
            &signature,
            &Message::from_digest(sighash.to_byte_array()),
            &signer,
        )
        .with_context(|| format!("invalid signature on input {index}"))?;
        psbt.inputs[index].tap_script_sigs.insert(
            (signer, leaf),
            taproot::Signature {
                signature,
                sighash_type: TapSighashType::Default,
            },
        );
    }
    Ok(())
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
    validate_default_sighashes(psbt)?;
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
    validate_default_sighashes(psbt)?;
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
    validate_default_sighashes(psbt)?;
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
    validate_default_sighashes(psbt)?;
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

/// Every policy signature must commit to the complete transaction. Check the PSBT input field
/// that actually selects the digest, not just the type attached to an existing signature.
/// An omitted field means SIGHASH_DEFAULT under BIP371 and is safe to accept.
pub(crate) fn validate_default_sighashes(psbt: &Psbt) -> Result<()> {
    for (index, input) in psbt.inputs.iter().enumerate() {
        if let Some(sighash_type) = input.sighash_type {
            if sighash_type != PsbtSighashType::from(TapSighashType::Default) {
                bail!("PSBT input {index} must use SIGHASH_DEFAULT");
            }
        }
    }
    Ok(())
}

pub fn finalize_vault_psbt(mut psbt: Psbt) -> Result<Transaction> {
    validate_default_sighashes(&psbt)?;
    let secp = Secp256k1::verification_only();
    psbt.finalize_mut(&secp)
        .map_err(|errors| anyhow::anyhow!("unable to finalize vault PSBT: {errors:?}"))?;
    psbt.extract(&secp)
        .context("unable to extract finalized vault transaction")
}

#[cfg(test)]
mod tests {
    use super::*;
    use anzen_common::{CONNECTOR_VALUE_SATS, HWW_RECOVERY_SEQUENCE, keys::DeviceKeys};
    use anzen_protocol::{policy::Input, script::Scripts, spend};
    use bitcoin::{
        Amount, OutPoint, ScriptBuf, Sequence, TxIn, Witness, absolute, transaction::Version,
    };

    fn create_vault_psbt(
        transaction: Transaction,
        prevouts: &[TxOut],
        policy: &VaultPolicy,
    ) -> Result<Psbt> {
        let phone = policy_key(policy, 0);
        let hww = policy_key(policy, 1);
        let controller = ControllerPolicy::new(phone, hww)?;
        let indexes = (0..transaction.input.len()).collect::<Vec<_>>();
        create_policy_psbt(transaction, prevouts, &indexes, &[], policy, &controller)
    }

    fn policy_key(policy: &VaultPolicy, index: usize) -> XOnlyPublicKey {
        let text = policy.descriptor_string();
        let keys = text.split("multi_a(2,").nth(1).unwrap();
        keys.split([',', ')']).nth(index).unwrap().parse().unwrap()
    }

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
    fn vault_and_controller_signatures_require_default_sighash_metadata() {
        let (vault, phone, hww, unsigned) = fixture(Sequence::MAX);
        let controller = ControllerPolicy::new(phone.vault_pubkey, hww.vault_pubkey).unwrap();
        let controller_unsigned = create_policy_psbt(
            unsigned.unsigned_tx.clone(),
            &[TxOut {
                value: Amount::from_sat(20_000_000),
                script_pubkey: controller.address.script_pubkey(),
            }],
            &[],
            &[0],
            &vault,
            &controller,
        )
        .unwrap();
        let mut vault_signed = unsigned.clone();
        sign_vault_psbt(
            &mut vault_signed,
            &vault,
            SpendPath::Cooperative,
            &phone.vault_keypair,
        )
        .unwrap();
        let mut controller_signed = controller_unsigned.clone();
        sign_controller_psbt_inputs(
            &mut controller_signed,
            &controller,
            ControllerPath::Phone,
            &phone.vault_keypair,
            &[0],
        )
        .unwrap();

        // Only exercise rejection: no signatures using these modes should ever be produced.
        for raw_type in [1, 2, 3, 0x81, 0x82, 0x83, 0xff] {
            let rejected_type = Some(PsbtSighashType::from_u32(raw_type));
            let mut psbt = unsigned.clone();
            psbt.inputs[0].sighash_type = rejected_type;
            let error = sign_vault_psbt(
                &mut psbt,
                &vault,
                SpendPath::Cooperative,
                &phone.vault_keypair,
            )
            .unwrap_err();
            assert!(error.to_string().contains("SIGHASH_DEFAULT"));
            assert!(psbt.inputs[0].tap_script_sigs.is_empty());

            let mut psbt = controller_unsigned.clone();
            psbt.inputs[0].sighash_type = rejected_type;
            assert!(
                sign_controller_psbt_inputs(
                    &mut psbt,
                    &controller,
                    ControllerPath::Phone,
                    &phone.vault_keypair,
                    &[0],
                )
                .is_err()
            );
            assert!(psbt.inputs[0].tap_script_sigs.is_empty());

            let mut psbt = vault_signed.clone();
            psbt.inputs[0].sighash_type = rejected_type;
            assert!(
                verify_vault_psbt_signature(
                    &psbt,
                    &vault,
                    SpendPath::Cooperative,
                    phone.vault_pubkey,
                )
                .unwrap_err()
                .to_string()
                .contains("SIGHASH_DEFAULT")
            );
            assert!(
                finalize_vault_psbt(psbt)
                    .unwrap_err()
                    .to_string()
                    .contains("SIGHASH_DEFAULT")
            );

            let mut psbt = controller_signed.clone();
            psbt.inputs[0].sighash_type = rejected_type;
            assert!(
                verify_controller_psbt_signatures(
                    &psbt,
                    &controller,
                    ControllerPath::Phone,
                    phone.vault_pubkey,
                    &[0],
                )
                .unwrap_err()
                .to_string()
                .contains("SIGHASH_DEFAULT")
            );
        }
    }

    #[test]
    fn default_sighash_can_be_explicit_or_omitted() {
        let (policy, phone, hww, unsigned) = fixture(Sequence::MAX);
        for field in [None, Some(TapSighashType::Default.into())] {
            let mut psbt = unsigned.clone();
            psbt.inputs[0].sighash_type = field;
            for keys in [&phone, &hww] {
                sign_vault_psbt(
                    &mut psbt,
                    &policy,
                    SpendPath::Cooperative,
                    &keys.vault_keypair,
                )
                .unwrap();
                verify_vault_psbt_signature(
                    &psbt,
                    &policy,
                    SpendPath::Cooperative,
                    keys.vault_pubkey,
                )
                .unwrap();
            }
            assert!(finalize_vault_psbt(psbt).is_ok());
        }
    }

    #[test]
    fn unsupported_sighash_in_later_input_is_rejected_before_any_signing() {
        let (policy, phone, _, mut psbt) = fixture(Sequence::MAX);
        psbt.unsigned_tx
            .input
            .push(psbt.unsigned_tx.input[0].clone());
        psbt.inputs.push(psbt.inputs[0].clone());
        psbt.inputs[1].sighash_type = Some(TapSighashType::All.into());
        assert!(
            sign_vault_psbt(
                &mut psbt,
                &policy,
                SpendPath::Cooperative,
                &phone.vault_keypair
            )
            .is_err()
        );
        assert!(
            psbt.inputs
                .iter()
                .all(|input| input.tap_script_sigs.is_empty())
        );
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
        let mut unsigned = tx.clone();
        for input in &mut unsigned.input {
            input.witness = Witness::new();
        }
        let protocol_tx = anzen_protocol::tx::Transaction {
            input: vec![anzen_protocol::tx::TxIn {
                previous_output: protocol::outpoint(unsigned.input[0].previous_output),
                sequence: unsigned.input[0].sequence.to_consensus_u32(),
            }],
            output: vec![anzen_protocol::tx::TxOut {
                value: unsigned.output[0].value.to_sat(),
                script_pubkey: unsigned.output[0].script_pubkey.to_bytes(),
            }],
        };
        assert_eq!(
            protocol_tx
                .estimated_vsize(&[anzen_protocol::tx::SpendPath::Cooperative])
                .unwrap(),
            tx.vsize() as u64
        );
    }

    #[test]
    fn the_phone_alone_can_never_finalize_a_vault_spend() {
        for path in [SpendPath::Cooperative, SpendPath::HwwRecovery] {
            let (policy, phone, _hww, mut psbt) = fixture(Sequence(HWW_RECOVERY_SEQUENCE));
            let signed = sign_vault_psbt(&mut psbt, &policy, path, &phone.vault_keypair);
            assert!(signed.is_err() || finalize_vault_psbt(psbt).is_err());
        }
    }

    #[test]
    fn hww_recovery_finalizes_with_only_hww_signature() {
        let (policy, _phone, hww, mut psbt) = fixture(Sequence(HWW_RECOVERY_SEQUENCE));
        sign_vault_psbt(
            &mut psbt,
            &policy,
            SpendPath::HwwRecovery,
            &hww.vault_keypair,
        )
        .unwrap();
        let tx = finalize_vault_psbt(psbt).unwrap();
        assert_eq!(tx.input[0].sequence, Sequence(HWW_RECOVERY_SEQUENCE));
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
        let vault = VaultPolicy::new(phone.vault_pubkey, hww.vault_pubkey).unwrap();
        let mut crypto = protocol::SoftwareCrypto::new();
        let scripts = Scripts::new(
            &mut crypto,
            &anzen_protocol::script::VaultKeys {
                phone: protocol::key(phone.vault_pubkey),
                hww: protocol::key(hww.vault_pubkey),
            },
        )
        .unwrap();
        let revocation = spend::controller_revocation(
            &mut crypto,
            &scripts,
            &[Input {
                outpoint: protocol::outpoint(OutPoint::null()),
                value_sats: CONNECTOR_VALUE_SATS,
            }],
            anzen_protocol::tx::SpendPath::ControllerPhone,
            destination.as_bytes(),
        )
        .unwrap();
        let fee_sats = revocation.fee_sats;
        let unsigned = protocol_psbt(
            &revocation.transaction,
            &revocation.prevouts,
            &revocation.spends,
            &vault,
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
