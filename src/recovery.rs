use crate::{
    HWW_RECOVERY_BLOCKS, PHONE_RECOVERY_BLOCKS,
    ceremony::{DEFAULT_BATCH_DIR, SCHEDULE_FILE},
    crypto,
    hot::HotWallet,
    keys::DeviceKeys,
    policy::{SpendPath, VaultPolicy},
    rpc::{RegtestRpc, VaultUtxo},
    state::{
        CONFIG_FILE, DeviceFile, HWW_DEVICE_FILE, PHONE_BACKUP_FILE, PHONE_DEVICE_FILE,
        VaultConfig, load_config, load_device, recover_phone_mnemonic, write_json,
    },
    transactions::{create_vault_psbt, estimate_vault_vsize, finalize_vault_psbt, sign_vault_psbt},
};
use anyhow::{Context, Result, bail};
use bitcoin::{
    Address, Amount, ScriptBuf, Sequence, Transaction, TxIn, TxOut, Txid, Witness, absolute,
    key::Secp256k1, transaction::Version,
};
use bitcoincore_rpc::RpcApi;
use serde::{Deserialize, Serialize};
use std::{
    fs,
    path::{Path, PathBuf},
};

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

pub fn rotate_phone(data_dir: &Path, rpc: &RegtestRpc) -> Result<RotationResult> {
    let old_config = load_config(data_dir)?;
    let old_phone_file = load_device(data_dir, PHONE_DEVICE_FILE)?;
    let hww_file = load_device(data_dir, HWW_DEVICE_FILE)?;
    let secp = Secp256k1::new();
    let old_phone = DeviceKeys::parse(&secp, &old_phone_file.mnemonic)?;
    let hww = DeviceKeys::parse(&secp, &hww_file.mnemonic)?;
    let new_phone = DeviceKeys::generate(&secp)?;
    let new_policy = VaultPolicy::new(new_phone.vault_pubkey, hww.vault_pubkey)?;
    let (hot_external, hot_internal) = new_phone.hot_descriptors(&secp)?;

    let old_policy = VaultPolicy::from_descriptor(&old_config.vault_descriptor)?;
    let utxos = rpc.scan_vault(&old_config)?;
    if utxos.is_empty() {
        bail!("vault has no confirmed UTXOs to rotate");
    }
    let (mut psbt, fee_sats, sent_sats) = build_sweep_psbt(
        &old_policy,
        &utxos,
        SweepPath::Cooperative,
        &new_policy.address,
    )?;
    sign_vault_psbt(
        &mut psbt,
        &old_policy,
        SpendPath::Cooperative,
        &old_phone.vault_keypair,
    )?;
    sign_vault_psbt(
        &mut psbt,
        &old_policy,
        SpendPath::Cooperative,
        &hww.vault_keypair,
    )?;
    let transaction = finalize_vault_psbt(psbt)?;
    let txid = rpc
        .client
        .send_raw_transaction(&transaction)
        .context("failed to broadcast emergency phone-key rotation")?;

    archive_old_epoch(data_dir, &old_config, txid)?;
    let new_phone_mnemonic = new_phone.mnemonic.to_string();
    let new_config = VaultConfig {
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
        hard_limit_sats: old_config.hard_limit_sats,
    };
    write_json(&data_dir.join(CONFIG_FILE), &new_config)?;
    write_json(
        &data_dir.join(PHONE_DEVICE_FILE),
        &DeviceFile {
            kind: "phone".to_owned(),
            mnemonic: new_phone_mnemonic.clone(),
        },
    )?;
    let backup = crypto::encrypt(
        &hww.seed,
        "phone-seed-backup",
        new_phone_mnemonic.as_bytes(),
    )?;
    write_json(&data_dir.join(PHONE_BACKUP_FILE), &backup)?;
    HotWallet::open_or_create(data_dir)?;

    Ok(RotationResult {
        sweep: SweepResult {
            txid,
            input_count: utxos.len(),
            sent_sats,
            fee_sats,
        },
        old_address: old_config.vault_address,
        new_address: new_config.vault_address,
        new_phone_mnemonic,
    })
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
        let initialized = initialize(dir.path(), 10_000_000).unwrap();
        fs::remove_file(dir.path().join(PHONE_DEVICE_FILE)).unwrap();
        let restored = restore_phone_from_hww_backup(dir.path()).unwrap();
        assert_eq!(restored, initialized.phone_mnemonic);
        assert!(dir.path().join(PHONE_DEVICE_FILE).exists());
    }

    #[test]
    fn phone_backup_cannot_be_recovered_without_the_hww() {
        let dir = tempfile::tempdir().unwrap();
        initialize(dir.path(), 10_000_000).unwrap();
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
