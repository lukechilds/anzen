use crate::{
    DEFAULT_FEE_RATE_SAT_VB, HWW_RECOVERY_BLOCKS, PHONE_RECOVERY_BLOCKS,
    ceremony::{DEFAULT_BATCH_DIR, SCHEDULE_FILE},
    crypto,
    hot::HotWallet,
    keys::DeviceKeys,
    policy::{SpendPath, VaultPolicy},
    rpc::{RegtestRpc, VaultUtxo},
    state::{
        CONFIG_FILE, DeviceFile, HWW_DEVICE_FILE, PHONE_BACKUP_FILE, PHONE_DEVICE_FILE,
        VaultConfig, load_config, load_device, read_json, recover_phone_mnemonic, write_json,
    },
    transactions::{
        create_vault_psbt, estimate_vault_vsize, finalize_vault_psbt, sign_vault_psbt,
        verify_vault_psbt_signature,
    },
};
use anyhow::{Context, Result, bail};
use bitcoin::{
    Address, Amount, Psbt, ScriptBuf, Sequence, Transaction, TxIn, TxOut, Txid, Witness, absolute,
    key::Secp256k1, secp256k1::XOnlyPublicKey, transaction::Version,
};
use bitcoincore_rpc::RpcApi;
use serde::{Deserialize, Serialize};
use std::{
    fs,
    path::{Path, PathBuf},
    str::FromStr,
};

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
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PhoneRecoveryPackage {
    pub version: u8,
    pub kind: String,
    pub phone_mnemonic: String,
    pub phone_vault_pubkey: String,
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
    pub sweep: CooperativeSweepPackage,
    pub encrypted_phone_backup: Option<crate::crypto::EncryptedBlob>,
}

pub fn decrypt_phone_backup_package(
    data_dir: &Path,
    backup_path: &Path,
) -> Result<PhoneRecoveryPackage> {
    let hww_file = load_device(data_dir, HWW_DEVICE_FILE)?;
    let hww = DeviceKeys::parse(&Secp256k1::new(), &hww_file.mnemonic)?;
    let backup: crate::crypto::EncryptedBlob = read_json(backup_path)?;
    let words = crypto::decrypt(&hww.seed, "phone-seed-backup", &backup)?;
    let words =
        String::from_utf8(words.to_vec()).context("decrypted phone backup was not UTF-8")?;
    let phone = DeviceKeys::parse(&Secp256k1::new(), &words)?;
    let config = load_config(data_dir)?;
    if phone.vault_pubkey.to_string() != config.phone_vault_pubkey {
        bail!("decrypted phone backup does not match the configured vault policy");
    }
    Ok(PhoneRecoveryPackage {
        version: 1,
        kind: "phone-recovery".to_owned(),
        phone_mnemonic: words,
        phone_vault_pubkey: phone.vault_pubkey.to_string(),
    })
}

pub fn restore_phone_package(data_dir: &Path, package: &PhoneRecoveryPackage) -> Result<String> {
    if package.version != 1 || package.kind != "phone-recovery" {
        bail!("unsupported phone recovery package");
    }
    let phone_path = data_dir.join(PHONE_DEVICE_FILE);
    if phone_path.exists() {
        bail!(
            "phone key still exists at {}; refusing to overwrite it",
            phone_path.display()
        );
    }
    let phone = DeviceKeys::parse(&Secp256k1::new(), &package.phone_mnemonic)?;
    let config = load_config(data_dir)?;
    if phone.vault_pubkey.to_string() != package.phone_vault_pubkey
        || package.phone_vault_pubkey != config.phone_vault_pubkey
    {
        bail!("phone recovery package does not match the configured vault policy");
    }
    write_json(
        &phone_path,
        &DeviceFile {
            kind: "phone".to_owned(),
            mnemonic: package.phone_mnemonic.clone(),
        },
    )?;
    Ok(package.phone_mnemonic.clone())
}

pub fn restore_phone_from_hww_backup(data_dir: &Path) -> Result<String> {
    let phone_path = data_dir.join(PHONE_DEVICE_FILE);
    if phone_path.exists() {
        bail!(
            "phone key still exists at {}; refusing to overwrite it",
            phone_path.display()
        );
    }

    let words = recover_phone_mnemonic(data_dir)?;
    let secp = Secp256k1::new();
    let recovered = DeviceKeys::parse(&secp, &words)?;
    let config = load_config(data_dir)?;
    if recovered.vault_pubkey.to_string() != config.phone_vault_pubkey {
        bail!("recovered phone backup does not match the configured vault policy");
    }
    write_json(
        &phone_path,
        &DeviceFile {
            kind: "phone".to_owned(),
            mnemonic: words.clone(),
        },
    )?;
    Ok(words)
}

pub fn sweep(
    data_dir: &Path,
    rpc: &RegtestRpc,
    path: SweepPath,
    destination: &Address,
) -> Result<SweepResult> {
    let config = load_config(data_dir)?;
    let policy = VaultPolicy::from_descriptor(&config.vault_descriptor)?;
    let tip_height = rpc.chain_info()?.blocks;
    let all_utxos = rpc.scan_vault(&config)?;
    let utxos = mature_utxos(&all_utxos, path, tip_height);
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

    let (mut psbt, fee_sats, sent_sats) = build_sweep_psbt(&policy, &utxos, path, destination)?;
    let secp = Secp256k1::new();
    match path {
        SweepPath::Cooperative => {
            let phone_file = load_device(data_dir, PHONE_DEVICE_FILE)?;
            let hww_file = load_device(data_dir, HWW_DEVICE_FILE)?;
            let phone = DeviceKeys::parse(&secp, &phone_file.mnemonic)?;
            let hww = DeviceKeys::parse(&secp, &hww_file.mnemonic)?;
            sign_vault_psbt(
                &mut psbt,
                &policy,
                SpendPath::Cooperative,
                &phone.vault_keypair,
            )?;
            sign_vault_psbt(
                &mut psbt,
                &policy,
                SpendPath::Cooperative,
                &hww.vault_keypair,
            )?;
        }
        SweepPath::PhoneRecovery => {
            let phone_file = load_device(data_dir, PHONE_DEVICE_FILE)?;
            let phone = DeviceKeys::parse(&secp, &phone_file.mnemonic)?;
            sign_vault_psbt(
                &mut psbt,
                &policy,
                SpendPath::PhoneRecovery,
                &phone.vault_keypair,
            )?;
        }
        SweepPath::HwwRecovery => {
            let hww_file = load_device(data_dir, HWW_DEVICE_FILE)?;
            let hww = DeviceKeys::parse(&secp, &hww_file.mnemonic)?;
            sign_vault_psbt(
                &mut psbt,
                &policy,
                SpendPath::HwwRecovery,
                &hww.vault_keypair,
            )?;
        }
    }

    let transaction = finalize_vault_psbt(psbt)?;
    let txid = rpc
        .client
        .send_raw_transaction(&transaction)
        .context("failed to broadcast vault recovery sweep")?;
    Ok(SweepResult {
        txid,
        input_count: utxos.len(),
        sent_sats,
        fee_sats,
    })
}

pub fn create_cooperative_sweep(
    data_dir: &Path,
    rpc: &RegtestRpc,
    destination: &Address,
) -> Result<CooperativeSweepPackage> {
    let config = load_config(data_dir)?;
    let policy = VaultPolicy::from_descriptor(&config.vault_descriptor)?;
    let utxos = rpc.scan_vault(&config)?;
    if utxos.is_empty() {
        bail!("vault has no confirmed UTXOs to sweep");
    }
    let (mut psbt, fee_sats, sent_sats) =
        build_sweep_psbt(&policy, &utxos, SweepPath::Cooperative, destination)?;
    let phone_file = load_device(data_dir, PHONE_DEVICE_FILE)?;
    let phone = DeviceKeys::parse(&Secp256k1::new(), &phone_file.mnemonic)?;
    sign_vault_psbt(
        &mut psbt,
        &policy,
        SpendPath::Cooperative,
        &phone.vault_keypair,
    )?;
    Ok(CooperativeSweepPackage {
        version: 1,
        kind: "cooperative-sweep".to_owned(),
        vault_descriptor: config.vault_descriptor,
        destination: destination.to_string(),
        psbt: psbt.to_string(),
        input_count: utxos.len(),
        sent_sats,
        fee_sats,
        phone_approved: true,
        hww_approved: false,
    })
}

pub fn confirm_cooperative_sweep(
    data_dir: &Path,
    package: &CooperativeSweepPackage,
) -> Result<CooperativeSweepPackage> {
    let (policy, mut psbt) = validate_cooperative_sweep(data_dir, package)?;
    let config = load_config(data_dir)?;
    let phone_pubkey = XOnlyPublicKey::from_str(&config.phone_vault_pubkey)?;
    verify_vault_psbt_signature(&psbt, &policy, SpendPath::Cooperative, phone_pubkey)?;
    let hww_file = load_device(data_dir, HWW_DEVICE_FILE)?;
    let hww = DeviceKeys::parse(&Secp256k1::new(), &hww_file.mnemonic)?;
    sign_vault_psbt(
        &mut psbt,
        &policy,
        SpendPath::Cooperative,
        &hww.vault_keypair,
    )?;
    let mut approved = package.clone();
    approved.psbt = psbt.to_string();
    approved.hww_approved = true;
    Ok(approved)
}

pub fn broadcast_cooperative_sweep(
    data_dir: &Path,
    rpc: &RegtestRpc,
    package: &CooperativeSweepPackage,
) -> Result<SweepResult> {
    if !package.phone_approved || !package.hww_approved {
        bail!("both phone and HWW approval are required for a cooperative sweep");
    }
    let (policy, psbt) = validate_cooperative_sweep(data_dir, package)?;
    let config = load_config(data_dir)?;
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
    let txid = rpc
        .client
        .send_raw_transaction(&transaction)
        .context("failed to broadcast cooperative vault sweep")?;
    Ok(SweepResult {
        txid,
        input_count: package.input_count,
        sent_sats: package.sent_sats,
        fee_sats: package.fee_sats,
    })
}

fn validate_cooperative_sweep(
    data_dir: &Path,
    package: &CooperativeSweepPackage,
) -> Result<(VaultPolicy, Psbt)> {
    if package.version != 1 || package.kind != "cooperative-sweep" || !package.phone_approved {
        bail!("unsupported or unsigned cooperative sweep package");
    }
    let config = load_config(data_dir)?;
    if package.vault_descriptor != config.vault_descriptor {
        bail!("cooperative sweep package does not match the configured vault policy");
    }
    let policy = VaultPolicy::from_descriptor(&config.vault_descriptor)?;
    let destination =
        Address::from_str(&package.destination)?.require_network(bitcoin::Network::Regtest)?;
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

pub fn rotate_phone(data_dir: &Path, rpc: &RegtestRpc) -> Result<RotationResult> {
    let proposal = create_phone_rotation(data_dir, rpc)?;
    let approved = confirm_phone_rotation(data_dir, &proposal)?;
    activate_phone_rotation(data_dir, rpc, &approved)
}

pub fn create_phone_rotation(data_dir: &Path, rpc: &RegtestRpc) -> Result<PhoneRotationPackage> {
    let old_config = load_config(data_dir)?;
    let secp = Secp256k1::new();
    let new_phone = DeviceKeys::generate(&secp)?;
    let hww_pubkey = XOnlyPublicKey::from_str(&old_config.hww_vault_pubkey)?;
    let new_policy = VaultPolicy::new(new_phone.vault_pubkey, hww_pubkey)?;
    write_json(
        &data_dir.join(PENDING_PHONE_ROTATION_FILE),
        &DeviceFile {
            kind: "pending-phone-rotation".to_owned(),
            mnemonic: new_phone.mnemonic.to_string(),
        },
    )?;
    let sweep = create_cooperative_sweep(data_dir, rpc, &new_policy.address)?;
    Ok(PhoneRotationPackage {
        version: 1,
        kind: "phone-key-rotation".to_owned(),
        old_vault_descriptor: old_config.vault_descriptor,
        new_phone_vault_pubkey: new_phone.vault_pubkey.to_string(),
        new_vault_descriptor: new_policy.descriptor_string(),
        new_vault_address: new_policy.address.to_string(),
        sweep,
        encrypted_phone_backup: None,
    })
}

pub fn confirm_phone_rotation(
    data_dir: &Path,
    package: &PhoneRotationPackage,
) -> Result<PhoneRotationPackage> {
    validate_phone_rotation(data_dir, package)?;
    let hww_file = load_device(data_dir, HWW_DEVICE_FILE)?;
    let hww = DeviceKeys::parse(&Secp256k1::new(), &hww_file.mnemonic)?;
    let pending: DeviceFile = read_json(&data_dir.join(PENDING_PHONE_ROTATION_FILE))?;
    let pending_phone = DeviceKeys::parse(&Secp256k1::new(), &pending.mnemonic)?;
    if pending_phone.vault_pubkey.to_string() != package.new_phone_vault_pubkey {
        bail!("pending phone key does not match the rotation proposal");
    }
    let mut approved = package.clone();
    approved.sweep = confirm_cooperative_sweep(data_dir, &package.sweep)?;
    approved.encrypted_phone_backup = Some(crypto::encrypt(
        &hww.seed,
        "phone-seed-backup",
        pending.mnemonic.as_bytes(),
    )?);
    Ok(approved)
}

pub fn activate_phone_rotation(
    data_dir: &Path,
    rpc: &RegtestRpc,
    package: &PhoneRotationPackage,
) -> Result<RotationResult> {
    validate_phone_rotation(data_dir, package)?;
    let backup = package
        .encrypted_phone_backup
        .as_ref()
        .context("HWW-approved phone backup is missing from the rotation package")?;
    if !package.sweep.hww_approved {
        bail!("HWW approval is missing from the rotation package");
    }
    let old_config = load_config(data_dir)?;
    let pending: DeviceFile = read_json(&data_dir.join(PENDING_PHONE_ROTATION_FILE))?;
    let new_phone = DeviceKeys::parse(&Secp256k1::new(), &pending.mnemonic)?;
    if new_phone.vault_pubkey.to_string() != package.new_phone_vault_pubkey {
        bail!("pending phone key does not match the approved rotation");
    }
    let sweep = broadcast_cooperative_sweep(data_dir, rpc, &package.sweep)?;
    archive_old_epoch(data_dir, &old_config, sweep.txid)?;
    let (hot_external, hot_internal) = new_phone.hot_descriptors(&Secp256k1::new())?;
    let new_config = VaultConfig {
        version: old_config.version,
        network: old_config.network.clone(),
        phone_vault_pubkey: new_phone.vault_pubkey.to_string(),
        hww_vault_pubkey: old_config.hww_vault_pubkey.clone(),
        phone_hot_external_descriptor: hot_external,
        phone_hot_internal_descriptor: hot_internal,
        vault_descriptor: package.new_vault_descriptor.clone(),
        vault_address: package.new_vault_address.clone(),
        phone_recovery_blocks: old_config.phone_recovery_blocks,
        hww_recovery_blocks: old_config.hww_recovery_blocks,
        monthly_limit_sats: 0,
    };
    write_json(&data_dir.join(CONFIG_FILE), &new_config)?;
    write_json(
        &data_dir.join(PHONE_DEVICE_FILE),
        &DeviceFile {
            kind: "phone".to_owned(),
            mnemonic: pending.mnemonic.clone(),
        },
    )?;
    write_json(&data_dir.join(PHONE_BACKUP_FILE), &backup)?;
    if data_dir.join(PENDING_PHONE_ROTATION_FILE).exists() {
        fs::remove_file(data_dir.join(PENDING_PHONE_ROTATION_FILE))?;
    }
    HotWallet::open_or_create(data_dir)?;

    Ok(RotationResult {
        sweep,
        old_address: old_config.vault_address,
        new_address: new_config.vault_address,
        new_phone_mnemonic: pending.mnemonic,
    })
}

fn validate_phone_rotation(data_dir: &Path, package: &PhoneRotationPackage) -> Result<()> {
    if package.version != 1 || package.kind != "phone-key-rotation" {
        bail!("unsupported phone rotation package");
    }
    let config = load_config(data_dir)?;
    if package.old_vault_descriptor != config.vault_descriptor
        || package.sweep.vault_descriptor != config.vault_descriptor
        || package.sweep.destination != package.new_vault_address
    {
        bail!("phone rotation package does not match the current vault");
    }
    let hww_pubkey = XOnlyPublicKey::from_str(&config.hww_vault_pubkey)?;
    let phone_pubkey = XOnlyPublicKey::from_str(&package.new_phone_vault_pubkey)?;
    let policy = VaultPolicy::new(phone_pubkey, hww_pubkey)?;
    if policy.descriptor_string() != package.new_vault_descriptor
        || policy.address.to_string() != package.new_vault_address
    {
        bail!("phone rotation destination does not match the proposed new keys");
    }
    validate_cooperative_sweep(data_dir, &package.sweep)?;
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
    // The MVP's fixed regtest feerate is exactly one satoshi per estimated vbyte.
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

fn archive_old_epoch(data_dir: &Path, old_config: &VaultConfig, txid: Txid) -> Result<()> {
    let archive = data_dir.join("history").join(format!("rotation-{txid}"));
    fs::create_dir_all(&archive)
        .with_context(|| format!("failed to create rotation archive {}", archive.display()))?;
    write_json(&archive.join(CONFIG_FILE), old_config)?;

    for relative in [
        PathBuf::from(SCHEDULE_FILE),
        PathBuf::from("phone/transactions"),
        PathBuf::from(DEFAULT_BATCH_DIR),
        PathBuf::from("phone/hot-wallet.sqlite-wal"),
        PathBuf::from("phone/hot-wallet.sqlite-shm"),
        PathBuf::from("phone/hot-wallet.sqlite"),
    ] {
        let source = data_dir.join(&relative);
        if !source.exists() {
            continue;
        }
        let destination = archive.join(&relative);
        if let Some(parent) = destination.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::rename(&source, &destination).with_context(|| {
            format!(
                "failed to archive obsolete state {} to {}",
                source.display(),
                destination.display()
            )
        })?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::{PHONE_BACKUP_FILE, initialize};
    use bitcoin::{Amount, OutPoint, TxOut};

    #[test]
    fn hww_backup_restores_a_deleted_phone_key() {
        let dir = tempfile::tempdir().unwrap();
        let initialized = initialize(dir.path()).unwrap();
        fs::remove_file(dir.path().join(PHONE_DEVICE_FILE)).unwrap();
        let restored = restore_phone_from_hww_backup(dir.path()).unwrap();
        assert_eq!(restored, initialized.phone_mnemonic);
        assert!(dir.path().join(PHONE_DEVICE_FILE).exists());
    }

    #[test]
    fn phone_backup_cannot_be_recovered_without_the_hww() {
        let dir = tempfile::tempdir().unwrap();
        initialize(dir.path()).unwrap();
        fs::remove_file(dir.path().join(PHONE_DEVICE_FILE)).unwrap();
        fs::remove_file(dir.path().join(HWW_DEVICE_FILE)).unwrap();
        assert!(restore_phone_from_hww_backup(dir.path()).is_err());
        assert!(dir.path().join(PHONE_BACKUP_FILE).exists());
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
