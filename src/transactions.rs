use crate::policy::{SpendPath, VaultPolicy};
use anyhow::{Context, Result, bail};
use bitcoin::{
    Psbt, Transaction, TxOut, Witness,
    hashes::Hash,
    key::Secp256k1,
    psbt::PsbtSighashType,
    secp256k1::{All, Keypair, Message, XOnlyPublicKey},
    sighash::{SighashCache, TapSighashType},
    taproot,
};
use miniscript::psbt::{PsbtExt, PsbtSighashMsg};

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

pub fn sign_vault_psbt(
    psbt: &mut Psbt,
    policy: &VaultPolicy,
    path: SpendPath,
    keypair: &Keypair,
) -> Result<()> {
    let secp = Secp256k1::new();
    let (signing_pubkey, _) = XOnlyPublicKey::from_keypair(keypair);
    let leaf = policy.leaf(path)?;

    for index in 0..psbt.inputs.len() {
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
    use crate::{HWW_RECOVERY_BLOCKS, PHONE_RECOVERY_BLOCKS, keys::DeviceKeys};
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
}
