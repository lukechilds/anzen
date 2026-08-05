use super::{
    DEFAULT_FEE_RATE_SAT_VB, HWW_RECOVERY_BLOCKS, PHONE_RECOVERY_BLOCKS,
    ceremony::{PolicyPackage, Schedule},
    keys::DeviceKeys,
    policy::{SpendPath, VaultPolicy},
    social::CloudRecoveryBackup,
    storage::{DeviceFile, VaultConfig, load_config, read_json},
    transactions::{
        create_vault_psbt, estimate_vault_vsize, finalize_vault_psbt, sign_vault_psbt,
        verify_vault_psbt_signature,
    },
    types::VaultUtxo,
};
use anyhow::{Context, Result, bail};
use bitcoin::{
    Address, Amount, Psbt, ScriptBuf, Sequence, Transaction, TxIn, TxOut, Txid, Witness, absolute,
    key::Secp256k1, secp256k1::XOnlyPublicKey, transaction::Version,
};
use serde::{Deserialize, Serialize};
use std::{path::Path, str::FromStr};

pub const PENDING_PHONE_ROTATION_FILE: &str = "phone/pending-rotation.json";

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

    fn sequence(self) -> Sequence {
        match self {
            Self::Cooperative => Sequence::MAX,
            Self::PhoneRecovery => Sequence(u32::from(PHONE_RECOVERY_BLOCKS)),
            Self::HwwRecovery => Sequence(u32::from(HWW_RECOVERY_BLOCKS)),
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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RotationResult {
    pub sweep: SweepResult,
    pub old_address: String,
    pub new_address: String,
    pub new_phone_mnemonic: String,
    pub renewed_schedule: Option<Schedule>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PhoneRecoveryPackage {
    pub version: u8,
    pub kind: String,
    pub phone_mnemonic: String,
    pub phone_vault_pubkey: String,
    pub vault_descriptor: String,
    pub vault_address: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CooperativeSweepPackage {
    pub version: u8,
    pub kind: String,
    pub vault_descriptor: String,
    pub destination: String,
    pub psbt: String,
    pub input_count: usize,
    pub sent_sats: u64,
    pub fee_sats: u64,
    pub phone_approved: bool,
    pub hww_approved: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PhoneRotationPackage {
    pub version: u8,
    pub kind: String,
    pub old_vault_descriptor: String,
    pub new_phone_vault_pubkey: String,
    pub new_vault_descriptor: String,
    pub new_vault_address: String,
    pub monthly_limit_sats: u64,
    pub sweep: CooperativeSweepPackage,
    pub renewed_policy: Option<PolicyPackage>,
    pub cloud_recovery_backup: Option<CloudRecoveryBackup>,
}

pub struct SweepPlan {
    policy: VaultPolicy,
    psbt: Psbt,
    input_count: usize,
    sent_sats: u64,
    fee_sats: u64,
}

pub fn prepare_sweep(
    config: &VaultConfig,
    all_utxos: &[VaultUtxo],
    tip_height: u64,
    path: SweepPath,
    destination: &Address,
) -> Result<SweepPlan> {
    let policy = VaultPolicy::from_descriptor_for_network(
        &config.vault_descriptor,
        config.bitcoin_network()?,
    )?;
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

    let (psbt, fee_sats, sent_sats) = build_sweep_psbt(&policy, &utxos, path, destination)?;
    Ok(SweepPlan {
        policy,
        psbt,
        input_count: utxos.len(),
        sent_sats,
        fee_sats,
    })
}

pub fn sign_recovery_sweep(
    mut plan: SweepPlan,
    path: SweepPath,
    signer: &DeviceKeys,
) -> Result<(Transaction, SweepResult)> {
    if path == SweepPath::Cooperative {
        bail!("cooperative sweeps require both device signatures");
    }
    sign_vault_psbt(
        &mut plan.psbt,
        &plan.policy,
        path.policy_path(),
        &signer.vault_keypair,
    )?;
    let transaction = finalize_vault_psbt(plan.psbt)?;
    let result = SweepResult {
        txid: transaction.compute_txid(),
        input_count: plan.input_count,
        sent_sats: plan.sent_sats,
        fee_sats: plan.fee_sats,
    };
    Ok((transaction, result))
}

pub fn create_cooperative_sweep(
    config: &VaultConfig,
    utxos: &[VaultUtxo],
    destination: &Address,
    phone: &DeviceKeys,
) -> Result<CooperativeSweepPackage> {
    let policy = VaultPolicy::from_descriptor_for_network(
        &config.vault_descriptor,
        config.bitcoin_network()?,
    )?;
    if utxos.is_empty() {
        bail!("vault has no confirmed UTXOs to sweep");
    }
    let (mut psbt, fee_sats, sent_sats) =
        build_sweep_psbt(&policy, utxos, SweepPath::Cooperative, destination)?;
    sign_vault_psbt(
        &mut psbt,
        &policy,
        SpendPath::Cooperative,
        &phone.vault_keypair,
    )?;
    Ok(CooperativeSweepPackage {
        version: 1,
        kind: "cooperative-sweep".to_owned(),
        vault_descriptor: config.vault_descriptor.clone(),
        destination: destination.to_string(),
        psbt: psbt.to_string(),
        input_count: utxos.len(),
        sent_sats,
        fee_sats,
        phone_approved: true,
        hww_approved: false,
    })
}

pub fn finalize_cooperative_sweep(
    config: &VaultConfig,
    package: &CooperativeSweepPackage,
) -> Result<(Transaction, SweepResult)> {
    if !package.phone_approved || !package.hww_approved {
        bail!("both phone and HWW approval are required for a cooperative sweep");
    }
    let (policy, psbt) = validate_cooperative_sweep(config, package)?;
    verify_vault_psbt_signature(
        &psbt,
        &policy,
        SpendPath::Cooperative,
        XOnlyPublicKey::from_str(&config.phone_vault_pubkey)?,
    )?;
    verify_vault_psbt_signature(
        &psbt,
        &policy,
        SpendPath::Cooperative,
        XOnlyPublicKey::from_str(&config.hww_vault_pubkey)?,
    )?;
    let transaction = finalize_vault_psbt(psbt)?;
    let result = SweepResult {
        txid: transaction.compute_txid(),
        input_count: package.input_count,
        sent_sats: package.sent_sats,
        fee_sats: package.fee_sats,
    };
    Ok((transaction, result))
}

pub fn validate_cooperative_sweep(
    config: &VaultConfig,
    package: &CooperativeSweepPackage,
) -> Result<(VaultPolicy, Psbt)> {
    if package.version != 1 || package.kind != "cooperative-sweep" || !package.phone_approved {
        bail!("unsupported or unsigned cooperative sweep package");
    }
    if package.vault_descriptor != config.vault_descriptor {
        bail!("cooperative sweep package does not match the configured vault policy");
    }
    let policy = VaultPolicy::from_descriptor_for_network(
        &config.vault_descriptor,
        config.bitcoin_network()?,
    )?;
    let destination =
        Address::from_str(&package.destination)?.require_network(config.bitcoin_network()?)?;
    let psbt = Psbt::from_str(&package.psbt).context("invalid cooperative sweep PSBT")?;
    let transaction = &psbt.unsigned_tx;
    if transaction.input.len() != package.input_count
        || psbt.inputs.len() != package.input_count
        || transaction.input.is_empty()
        || transaction
            .input
            .iter()
            .any(|input| input.sequence != Sequence::MAX)
        || transaction.output.len() != 1
        || transaction.output[0].script_pubkey != destination.script_pubkey()
        || transaction.output[0].value.to_sat() != package.sent_sats
    {
        bail!("cooperative sweep transaction does not match its package");
    }
    let vault_script = policy.address.script_pubkey();
    let input_sats = psbt.inputs.iter().try_fold(0_u64, |total, input| {
        let prevout = input
            .witness_utxo
            .as_ref()
            .context("cooperative sweep input lacks witness UTXO")?;
        if prevout.script_pubkey != vault_script {
            bail!("cooperative sweep input is outside the vault policy");
        }
        total
            .checked_add(prevout.value.to_sat())
            .context("cooperative sweep input sum overflowed")
    })?;
    let fee_sats = input_sats
        .checked_sub(package.sent_sats)
        .context("cooperative sweep outputs exceed its inputs")?;
    let expected_fee = estimate_vault_vsize(transaction, &policy, SpendPath::Cooperative)?
        * DEFAULT_FEE_RATE_SAT_VB;
    if fee_sats != package.fee_sats || fee_sats != expected_fee {
        bail!("cooperative sweep fee does not match its package");
    }
    Ok((policy, psbt))
}

pub fn validate_phone_rotation(
    data_dir: &Path,
    package: &PhoneRotationPackage,
) -> Result<(VaultConfig, VaultConfig, DeviceKeys)> {
    if package.version != 1 || package.kind != "phone-key-rotation" {
        bail!("unsupported phone rotation package");
    }
    let old_config = load_config(data_dir)?;
    if package.old_vault_descriptor != old_config.vault_descriptor
        || package.sweep.vault_descriptor != old_config.vault_descriptor
        || package.sweep.destination != package.new_vault_address
        || package.monthly_limit_sats != old_config.monthly_limit_sats
    {
        bail!("phone rotation package does not match the current vault");
    }
    let pending: DeviceFile = read_json(&data_dir.join(PENDING_PHONE_ROTATION_FILE))?;
    if pending.bitcoin_network()? != old_config.bitcoin_network()? {
        bail!("pending phone key network does not match the current vault");
    }
    let new_phone = DeviceKeys::parse_for_network(
        &Secp256k1::new(),
        &pending.mnemonic,
        old_config.bitcoin_network()?,
    )?;
    if new_phone.vault_pubkey.to_string() != package.new_phone_vault_pubkey {
        bail!("pending phone key does not match the rotation proposal");
    }
    let hww_pubkey = XOnlyPublicKey::from_str(&old_config.hww_vault_pubkey)?;
    let phone_pubkey = XOnlyPublicKey::from_str(&package.new_phone_vault_pubkey)?;
    let policy =
        VaultPolicy::new_for_network(phone_pubkey, hww_pubkey, old_config.bitcoin_network()?)?;
    if policy.descriptor_string() != package.new_vault_descriptor
        || policy.address.to_string() != package.new_vault_address
    {
        bail!("phone rotation destination does not match the proposed new keys");
    }
    validate_cooperative_sweep(&old_config, &package.sweep)?;
    let new_config = rotated_config(&old_config, &new_phone, &policy)?;
    match (old_config.monthly_limit_sats, &package.renewed_policy) {
        (0, None) => {}
        (0, Some(_)) => bail!("disabled monthly policy must not create renewed transactions"),
        (_, None) => bail!("phone rotation is missing the renewed monthly policy"),
        (_, Some(renewed)) => {
            validate_rotation_policy_binding(&old_config, &new_config, &package.sweep, renewed)?
        }
    }
    Ok((old_config, new_config, new_phone))
}

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
    })
}

pub fn validate_rotation_policy_binding(
    old_config: &VaultConfig,
    new_config: &VaultConfig,
    sweep: &CooperativeSweepPackage,
    renewed: &PolicyPackage,
) -> Result<()> {
    if renewed.version != 2
        || renewed.kind != "monthly-policy"
        || !renewed.manifest.phone_approved
        || renewed.manifest.vault_descriptor != new_config.vault_descriptor
        || renewed.manifest.vault_address != new_config.vault_address
        || renewed.manifest.monthly_limit_sats != old_config.monthly_limit_sats
    {
        bail!("renewed monthly policy does not preserve the active policy");
    }
    let sweep_psbt = Psbt::from_str(&sweep.psbt).context("invalid rotation sweep PSBT")?;
    let sweep_tx = &sweep_psbt.unsigned_tx;
    let sweep_output = sweep_tx
        .output
        .first()
        .context("rotation sweep has no output")?;
    let rollover_text = renewed
        .psbts
        .get(&renewed.manifest.rollover.psbt_file)
        .context("renewed monthly policy is missing its rollover PSBT")?;
    let rollover = Psbt::from_str(rollover_text).context("invalid renewed policy rollover PSBT")?;
    if rollover.unsigned_tx.input.len() != 1
        || rollover.inputs.len() != 1
        || rollover.unsigned_tx.input[0].previous_output
            != bitcoin::OutPoint::new(sweep_tx.compute_txid(), 0)
        || rollover.inputs[0].witness_utxo.as_ref() != Some(sweep_output)
    {
        bail!("renewed monthly policy is not chained to the rotation sweep");
    }
    Ok(())
}

fn mature_utxos(utxos: &[VaultUtxo], path: SweepPath, tip_height: u64) -> Vec<VaultUtxo> {
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

fn build_sweep_psbt(
    policy: &VaultPolicy,
    utxos: &[VaultUtxo],
    path: SweepPath,
    destination: &Address,
) -> Result<(bitcoin::Psbt, u64, u64)> {
    let input_sats = utxos.iter().try_fold(0_u64, |sum, utxo| {
        sum.checked_add(utxo.txout.value.to_sat())
            .context("vault input total overflowed")
    })?;
    let mut transaction = Transaction {
        version: Version::TWO,
        lock_time: absolute::LockTime::ZERO,
        input: utxos
            .iter()
            .map(|utxo| TxIn {
                previous_output: utxo.outpoint,
                script_sig: ScriptBuf::new(),
                sequence: path.sequence(),
                witness: Witness::new(),
            })
            .collect(),
        output: vec![TxOut {
            value: Amount::from_sat(input_sats),
            script_pubkey: destination.script_pubkey(),
        }],
    };
    // The MVP uses exactly one satoshi per estimated vbyte. This is deterministic on regtest and
    // explicitly unsafe under dangerously enabled mainnet mode.
    let fee_sats = estimate_vault_vsize(&transaction, policy, path.policy_path())?;
    let sent_sats = input_sats
        .checked_sub(fee_sats)
        .context("vault balance cannot pay the recovery sweep fee")?;
    transaction.output[0].value = Amount::from_sat(sent_sats);
    let prevouts = utxos
        .iter()
        .map(|utxo| utxo.txout.clone())
        .collect::<Vec<_>>();
    Ok((
        create_vault_psbt(transaction, &prevouts, policy)?,
        fee_sats,
        sent_sats,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        cold_wallet,
        core::storage::{HWW_DEVICE_FILE, PHONE_BACKUP_FILE, PHONE_DEVICE_FILE},
        hot_wallet,
        test_support::initialize,
    };
    use bitcoin::{Amount, OutPoint, TxOut};
    use std::fs;

    #[test]
    fn hww_backup_restores_a_deleted_phone_key() {
        let dir = tempfile::tempdir().unwrap();
        let initialized = initialize(dir.path()).unwrap();
        fs::remove_file(dir.path().join(PHONE_DEVICE_FILE)).unwrap();
        let package = cold_wallet::decrypt_phone_backup_package(
            dir.path(),
            &dir.path().join(PHONE_BACKUP_FILE),
        )
        .unwrap();
        let restored = hot_wallet::restore_phone(dir.path(), &package).unwrap();
        assert_eq!(restored, initialized.phone_mnemonic);
        assert!(dir.path().join(PHONE_DEVICE_FILE).exists());
    }

    #[test]
    fn phone_backup_cannot_be_recovered_without_the_hww() {
        let dir = tempfile::tempdir().unwrap();
        initialize(dir.path()).unwrap();
        fs::remove_file(dir.path().join(PHONE_DEVICE_FILE)).unwrap();
        fs::remove_file(dir.path().join(HWW_DEVICE_FILE)).unwrap();
        assert!(
            cold_wallet::decrypt_phone_backup_package(
                dir.path(),
                &dir.path().join(PHONE_BACKUP_FILE)
            )
            .is_err()
        );
        assert!(dir.path().join(PHONE_BACKUP_FILE).exists());
    }

    #[test]
    fn legacy_hww_phone_backup_migrates_to_the_descriptor_bound_envelope() {
        let dir = tempfile::tempdir().unwrap();
        let initialized = initialize(dir.path()).unwrap();
        let hww = crate::core::storage::load_device_keys(dir.path(), HWW_DEVICE_FILE).unwrap();
        let legacy = crate::core::crypto::encrypt(
            &hww.seed,
            "phone-seed-backup",
            initialized.phone_mnemonic.as_bytes(),
        )
        .unwrap();
        crate::core::storage::write_json(&dir.path().join(PHONE_BACKUP_FILE), &legacy).unwrap();

        assert_eq!(
            cold_wallet::decrypt_phone_backup(dir.path()).unwrap(),
            initialized.phone_mnemonic
        );
        let migrated: crate::core::social::CloudRecoveryBackup =
            crate::core::storage::read_json(&dir.path().join(PHONE_BACKUP_FILE)).unwrap();
        assert_eq!(migrated.kind, "vault-cloud-recovery");
    }

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
}
