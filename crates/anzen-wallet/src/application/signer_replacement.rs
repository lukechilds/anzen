//! Surviving-phone HWW replacement, including maturity checks and durable activation.
use super::rotation::broadcast_once;
use super::*;
use crate::{
    offline_action,
    recovery::{self, PhoneRecoveryTransfer, SweepPath},
};
use anzen_common::storage::fs::StoragePath;
use anzen_common::{
    PHONE_RECOVERY_BLOCKS, crypto, device_channel,
    policy::VaultPolicy,
    protocol,
    social::{self, CloudBackupDraft, DeviceBackupKeyResponse},
};
use anzen_protocol::wire::{self, ReplacementRequest};

#[derive(Serialize, Deserialize)]
struct PendingReplacement {
    request: DeviceSignerReplacement,
    backup_draft: CloudBackupDraft,
    old_backup: Option<CloudRecoveryBackup>,
    /// The phone's own CSV recovery transfer, signed only once its delay has matured.
    recovery: PhoneRecoveryTransfer,
    /// Old-epoch connectors the phone revokes before moving the vault.
    connectors: Vec<VaultUtxo>,
}

#[derive(Serialize)]
pub struct SignerReplacementCandidate {
    pub signer: PublicDeviceFile,
    pub new_config: VaultConfig,
    pub recovery_next_block_height: Option<u64>,
    pub recovery_blocks_remaining: Option<u64>,
    pub total_sats: u64,
    pub selected_sats: u64,
    pub selected_input_count: usize,
}

impl WalletApplication {
    fn replacement_config(&self, signer: &PublicDeviceFile) -> Result<VaultConfig> {
        signer.validate_kind("hww-public-key")?;
        let mut config = self.configuration()?;
        if signer.network != config.network
            || signer.vault_pubkey == config.hww_vault_pubkey
            || signer.vault_pubkey == config.phone_vault_pubkey
        {
            bail!("choose a fresh independent signer on the same network");
        }
        let policy = VaultPolicy::new_for_network(
            config.phone_vault_pubkey.parse()?,
            signer.parsed_vault_pubkey()?,
            config.bitcoin_network()?,
        )?;
        config.hww_vault_pubkey = signer.vault_pubkey.clone();
        config.vault_descriptor = policy.descriptor_string();
        config.vault_address = policy.address.to_string();
        config.validate_public_policy()?;
        Ok(config)
    }

    pub fn inspect_signer_replacement(
        &self,
        backend: &dyn HotWalletBackend,
        signer: &PublicDeviceFile,
    ) -> Result<SignerReplacementCandidate> {
        self.check_backend(backend)?;
        let new_config = self.replacement_config(signer)?;
        let all_inputs = backend.scan_vault(&self.configuration()?)?;
        let tip = backend.chain_tip()?.height;
        let inputs = recovery::recovery_review_inputs(&all_inputs, SweepPath::PhoneRecovery, tip);
        let total_sats =
            recovery::recovery_timing(&all_inputs, SweepPath::PhoneRecovery, tip)?.total_sats;
        let selected_sats =
            recovery::recovery_timing(&inputs, SweepPath::PhoneRecovery, tip)?.total_sats;
        let recovery_next_block_height = inputs
            .iter()
            .map(|u| {
                u.confirmation_height
                    .saturating_add(u64::from(PHONE_RECOVERY_BLOCKS))
            })
            .max();
        let next = tip.saturating_add(1);
        Ok(SignerReplacementCandidate {
            signer: signer.clone(),
            new_config,
            recovery_next_block_height,
            recovery_blocks_remaining: recovery_next_block_height.map(|h| h.saturating_sub(next)),
            total_sats,
            selected_sats,
            selected_input_count: inputs.len(),
        })
    }

    fn active_recovery_bundle(&self) -> Result<RecoveryVaultBundle> {
        let config = self.configuration()?;
        let last_approved_policy = if self.data_dir.join(ceremony::SCHEDULE_FILE).storage_exists() {
            let schedule = super::super::load_schedule(&self.data_dir)?;
            let txid: Txid = schedule.rollover_txid.parse()?;
            let blob: crypto::EncryptedBlob = storage::read_json(
                &self
                    .data_dir
                    .join("phone/transactions")
                    .join(txid.to_string())
                    .join("approved-policy.json"),
            )?;
            Some(super::super::open_approved_policy(
                &self.data_dir,
                super::super::ApprovedPolicyInput::Encrypted(blob),
            )?)
        } else {
            None
        };
        let bundle = RecoveryVaultBundle {
            version: anzen_common::recovery_bundle::BUNDLE_VERSION,
            config,
            last_approved_policy,
        };
        bundle.validate()?;
        Ok(bundle)
    }

    pub fn propose_signer_replacement(
        &self,
        backend: &dyn HotWalletBackend,
        signer: &PublicDeviceFile,
    ) -> Result<DeviceSignerReplacement> {
        self.check_backend(backend)?;
        let new_config = self.replacement_config(signer)?;
        let source = self.active_recovery_bundle()?;
        let old_backup: Option<CloudRecoveryBackup> = if self
            .data_dir
            .join(storage::PHONE_BACKUP_FILE)
            .storage_exists()
        {
            Some(self.export_backup()?)
        } else {
            None
        };
        let friend_public_keys = old_backup.as_ref().map_or_else(Vec::new, |backup| {
            backup
                .friends
                .iter()
                .map(|friend| friend.public_key_armored.clone())
                .collect()
        });
        let phone = load_device_keys(&self.data_dir, PHONE_DEVICE_FILE)?;
        let inputs = recovery::recovery_review_inputs(
            &backend.scan_vault(&source.config)?,
            SweepPath::PhoneRecovery,
            backend.chain_tip()?.height,
        );
        if inputs.is_empty() {
            bail!("no confirmed vault inputs to recover");
        }
        let destination =
            Address::from_str(&new_config.vault_address)?.require_network(backend.network())?;
        let recovery = PhoneRecoveryTransfer::prepare(&source.config, &inputs, &destination)?;
        let funding = recovery::vault_spend(
            &source.config,
            &inputs,
            SweepPath::PhoneRecovery,
            &destination,
        )?;
        let mut addresses = HotWallet::open_or_create(&self.data_dir)?;
        let renewed_policy = self.renewal_request(&new_config, &funding, &mut addresses)?;
        let connectors = backend.scan_connectors(&source.config)?;
        let (backup_draft, backup_request) =
            self.prepare_rotation_backup(&new_config, &phone, &friend_public_keys)?;
        let request = DeviceSignerReplacement {
            version: anzen_common::signer_replacement::REPLACEMENT_VERSION,
            replacement: ReplacementRequest {
                version: wire::VERSION,
                network: protocol::config_network(&source.config)?,
                old_keys: protocol::vault_keys(&source.config)?,
                new_hww: protocol::vault_keys(&new_config)?.hww,
                recovery_inputs: protocol::inputs(&inputs),
                renewed_policy,
            },
            source,
            new_config,
            backup_request,
        };
        request.validate()?;
        storage::write_json(
            &self.signer_replacement_path(&request.id()?, "proposal")?,
            &PendingReplacement {
                request: request.clone(),
                backup_draft,
                old_backup,
                recovery,
                connectors,
            },
        )?;
        Ok(request)
    }

    pub fn activate_signer_replacement(
        &self,
        backend: &dyn HotWalletBackend,
        approval: &ApprovedSignerReplacement,
    ) -> Result<DeviceRotationResult> {
        self.check_backend(backend)?;
        let pending: PendingReplacement =
            storage::read_json(&self.signer_replacement_path(&approval.request_id, "proposal")?)?;
        let renewed_policy = pending.request.renewed_policy(approval)?;
        let old = &pending.request.source.config;
        let new = &pending.request.new_config;
        let current = self.configuration()?;
        if serde_json::to_vec(&current)? != serde_json::to_vec(old)?
            && serde_json::to_vec(&current)? != serde_json::to_vec(new)?
        {
            bail!("vault or active limits changed since proposing signer replacement");
        }
        let phone = load_device_keys(&self.data_dir, PHONE_DEVICE_FILE)?;
        if phone.vault_pubkey.to_string() != new.phone_vault_pubkey {
            bail!("replacement belongs to another phone");
        }
        let response: DeviceBackupKeyResponse = device_channel::open(
            &phone,
            new.hww_vault_pubkey.parse()?,
            &format!("cloud-backup-response/{}", new.vault_address),
            &approval.backup_response,
        )?;
        if response.backup_id
            != sha256::Hash::hash(&serde_json::to_vec(&pending.backup_draft)?).to_string()
        {
            bail!("replacement backup approval mismatch");
        }
        let backup = social::complete_backup(pending.backup_draft, response.wrapped_key)?;
        let receipt = self.signer_replacement_path(&approval.request_id, "result")?;
        if receipt.storage_exists() {
            return storage::read_json(&receipt);
        }
        let submitted = self.signer_replacement_path(&approval.request_id, "submitted")?;
        if submitted.storage_exists() {
            let expected: Txid = storage::read_json(&submitted)?;
            if expected != pending.recovery.txid {
                bail!("replacement submission mismatch");
            }
        } else {
            // Preflight every input before any broadcast, including connector revocation.
            let live = backend.scan_vault(old)?;
            let next_height = backend.chain_tip()?.height.saturating_add(1);
            for input in &pending.recovery.inputs {
                let coin = live
                    .iter()
                    .find(|coin| coin.outpoint == input.outpoint)
                    .context("replacement input is no longer confirmed and unspent")?;
                let valid = coin
                    .confirmation_height
                    .saturating_add(u64::from(PHONE_RECOVERY_BLOCKS));
                if next_height < valid {
                    bail!(
                        "phone recovery is not mature; replacement requires next-block height {valid}"
                    );
                }
            }
        }
        storage::write_json(
            &self.signer_replacement_path(&approval.request_id, "approved")?,
            approval,
        )?;
        storage::write_json(
            &self.signer_replacement_path(&approval.request_id, "backup")?,
            &backup,
        )?;
        // Remember the old script before any broadcast or configuration update. New deposits
        // must not be forgotten when only a mature cohort is moved to the replacement signer.
        self.remember_previous_vault(old)?;
        // An intent permits retry after an accepted broadcast loses its response; it is not a
        // claim of acceptance. Bitcoin still checks the recovery sequence on every submission.
        storage::write_json(&submitted, &pending.recovery.txid)?;
        let controller_revocation_txid = if pending.connectors.is_empty() {
            None
        } else {
            let destination =
                Address::from_str(&new.vault_address)?.require_network(backend.network())?;
            let transaction =
                offline_action::phone_revocation(old, &pending.connectors, &destination, &phone)?;
            Some(broadcast_once(
                backend,
                &self.signer_replacement_path(&approval.request_id, "controllers")?,
                &transaction,
            )?)
        };
        let funding = pending.recovery.sign(old, &phone)?;
        let sweep_txid = broadcast_once(
            backend,
            &self.signer_replacement_path(&approval.request_id, "recovery")?,
            &funding,
        )?;
        storage::write_json(&self.data_dir.join(storage::PHONE_BACKUP_FILE), &backup)?;
        storage::write_json(
            &self.data_dir.join(storage::HWW_PUBLIC_FILE),
            &PublicDeviceFile {
                version: 1,
                kind: "hww-public-key".into(),
                network: new.network.clone(),
                vault_pubkey: new.hww_vault_pubkey.clone(),
            },
        )?;
        storage::write_json(&self.data_dir.join(storage::CONFIG_FILE), new)?;
        let renewed_schedule = renewed_policy
            .as_ref()
            .map(|renewed| self.activate_policy(backend, renewed))
            .transpose()?;
        let result = DeviceRotationResult {
            request_id: approval.request_id.clone(),
            old_vault_address: old.vault_address.clone(),
            new_vault_address: new.vault_address.clone(),
            sweep_txid,
            controller_revocation_txid,
            hot_wallet_sweep_txid: None,
            renewed_schedule,
        };
        storage::write_json(&receipt, &result)?;
        Ok(result)
    }

    fn signer_replacement_path(&self, id: &str, category: &str) -> Result<PathBuf> {
        if id.len() != 64
            || !id
                .bytes()
                .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
        {
            bail!("invalid signer replacement ID");
        }
        Ok(self
            .data_dir
            .join("phone/signer-replacements")
            .join(id)
            .join(format!("{category}.json")))
    }
}
