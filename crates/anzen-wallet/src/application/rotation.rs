//! Phone-key rotation, coordinated here and never in the UI or the cold signer.
//!
//! A working phone rotates to a freshly generated key; a restored phone rotates away from the
//! recovered key it holds for its fresh device key. Both send the signer the same parameters
//! and follow the same durable sequence: record the approval, broadcast each transaction once,
//! then install the replacement configuration.
use super::*;
use crate::{
    ceremony::{self as policy_ceremony, PolicyLimits},
    cosigning, recovery,
};
use anzen_common::storage::fs::StoragePath;
use anzen_common::{
    cloud_backup::{
        self, CloudBackupDraft, DeviceBackupKeyRequest, DeviceBackupKeyResponse, RecoveryPayload,
    },
    device_channel,
    keys::DeviceKeys,
    policy::VaultPolicy,
    protocol::{self, SoftwareCrypto},
    storage::{DeviceFile, PHONE_BACKUP_FILE},
};
use anzen_protocol::{
    script::Scripts,
    spend,
    tx::SpendPath,
    wire::{self, PolicyRequest, RotationRequest},
};
use bitcoin::{Transaction, secp256k1::XOnlyPublicKey};

/// Where the key being retired lives.
#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
enum RotationSource {
    /// This phone's own key, replaced by a generated key.
    Current,
    /// A key recovered from the cloud backup, replaced by this phone's own key.
    Restored,
}

#[derive(Serialize, Deserialize)]
struct PendingRotation {
    source: RotationSource,
    old_config: VaultConfig,
    request: DevicePhoneRotation,
    backup_draft: CloudBackupDraft,
    /// Receive addresses the renewed policy pays, revealed from the replacement key.
    renewed_destinations: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeviceRotationResult {
    pub request_id: String,
    pub old_vault_address: String,
    pub new_vault_address: String,
    pub sweep_txid: Txid,
    pub controller_revocation_txid: Option<Txid>,
    #[serde(default)]
    pub hot_wallet_sweep_txid: Option<Txid>,
    pub renewed_schedule: Option<Schedule>,
}

const PENDING_KEY_FILE: &str = "phone/rotation/pending-key.json";

impl WalletApplication {
    /// Rotate a working phone to a fresh key. A key generated for an unfinished rotation is
    /// reused, so repeating the proposal never loses track of it.
    pub fn propose_rotation<F>(
        &self,
        backend: &dyn HotWalletBackend,
        vanity: Option<super::super::VanityEngine>,
        report_progress: F,
    ) -> Result<DevicePhoneRotation>
    where
        F: Fn(u64) + Sync,
    {
        self.check_backend(backend)?;
        let old_config = self.configuration()?;
        let network = old_config.bitcoin_network()?;
        let pending_key = self.data_dir.join(PENDING_KEY_FILE);
        let new_phone = if pending_key.storage_exists() {
            let file: DeviceFile = storage::read_json(&pending_key)?;
            let phone = DeviceKeys::parse_for_network_at_index(
                &bitcoin::key::Secp256k1::new(),
                &file.mnemonic,
                network,
                file.vault_key_index,
            )?;
            if vanity.is_some() {
                let address = VaultPolicy::new_for_network(
                    phone.vault_pubkey,
                    XOnlyPublicKey::from_str(&old_config.hww_vault_pubkey)?,
                    network,
                )?
                .address
                .to_string();
                if !address.starts_with(super::super::vanity_address_prefix(network)?) {
                    bail!(
                        "a non-vanity phone rotation is already pending; finish it before starting a vanity rotation"
                    );
                }
            }
            phone
        } else {
            let phone = match vanity {
                Some(engine) => {
                    super::super::grind_vanity_phone_key(
                        network,
                        XOnlyPublicKey::from_str(&old_config.hww_vault_pubkey)?,
                        super::super::VANITY_SUFFIX,
                        engine,
                        report_progress,
                    )?
                    .phone
                }
                None => DeviceKeys::generate_for_network(&bitcoin::key::Secp256k1::new(), network)?,
            };
            storage::write_json(
                &pending_key,
                &DeviceFile {
                    kind: "pending-phone-rotation".to_owned(),
                    network: old_config.network.clone(),
                    mnemonic: phone.mnemonic.to_string(),
                    vault_key_index: phone.vault_key_index,
                },
            )?;
            phone
        };
        let schedule = super::super::active_schedule(&self.data_dir)?;
        let mut addresses = HotWallet::ephemeral(&new_phone)?;
        let old_phone = load_device_keys(&self.data_dir, PHONE_DEVICE_FILE)?;
        self.build_rotation(
            backend,
            RotationSource::Current,
            old_config,
            &old_phone,
            &new_phone,
            &mut addresses,
            |spent| recovery::allowances_reported_complete(schedule.as_ref(), spent),
        )
    }

    pub fn propose_restored_rotation(
        &self,
        backend: &dyn HotWalletBackend,
    ) -> Result<DevicePhoneRotation> {
        self.check_backend(backend)?;
        if storage::config_exists(&self.data_dir) {
            bail!("restore and rotate into a fresh phone without an existing vault");
        }
        let bundle: RecoveryVaultBundle =
            storage::read_json(&self.data_dir.join("phone/recovery-bundle.json"))
                .context("import the signer's signed recovery policy before rotating")?;
        bundle.validate()?;
        let old_config = self.recovery_source()?;
        if serde_json::to_vec(&old_config)? != serde_json::to_vec(&bundle.config)? {
            bail!("recovery source changed since importing the signed policy");
        }
        let new_phone = load_device_keys(&self.data_dir, PHONE_DEVICE_FILE)?;
        let mut addresses = HotWallet::open_or_create(&self.data_dir)?;
        let last_approved = bundle.last_approved_policy;
        let old_phone = self.restored_keys(&old_config)?;
        self.build_rotation(
            backend,
            RotationSource::Restored,
            old_config,
            &old_phone,
            &new_phone,
            &mut addresses,
            |spent| allowances_complete(last_approved.as_ref(), spent),
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn build_rotation(
        &self,
        backend: &dyn HotWalletBackend,
        source: RotationSource,
        old_config: VaultConfig,
        old_phone: &DeviceKeys,
        new_phone: &DeviceKeys,
        addresses: &mut HotWallet,
        previous_complete: impl FnOnce(&[VaultUtxo]) -> Result<bool>,
    ) -> Result<DevicePhoneRotation> {
        let network = backend.network();
        let policy = VaultPolicy::new_for_network(
            new_phone.vault_pubkey,
            XOnlyPublicKey::from_str(&old_config.hww_vault_pubkey)?,
            network,
        )?;
        let new_config = recovery::rotated_config(&old_config, new_phone, &policy)?;
        let inputs = backend.scan_vault(&old_config)?;
        if inputs.is_empty() {
            bail!("vault has no confirmed UTXOs to sweep");
        }
        let connectors = backend.scan_connectors(&old_config)?;
        let spent = inputs
            .iter()
            .chain(&connectors)
            .cloned()
            .collect::<Vec<_>>();
        let previous_allowances_complete = previous_complete(&spent)?;
        let mut crypto = SoftwareCrypto::new();
        let old_scripts = protocol::protocol(Scripts::new(
            &mut crypto,
            &protocol::vault_keys(&old_config)?,
        ))?;
        let new_scripts = protocol::protocol(Scripts::new(
            &mut crypto,
            &protocol::vault_keys(&new_config)?,
        ))?;
        let sweep = protocol::protocol(spend::vault_sweep(
            &mut crypto,
            &old_scripts,
            &protocol::inputs(&inputs),
            SpendPath::Cooperative,
            &new_scripts.vault.script_pubkey,
        ))?;
        let sweep_nonces = cosigning::Phone {
            data_dir: &self.data_dir,
            keys: old_phone,
        }
        .start(&cosigning::spend_jobs(&old_scripts, &sweep)?)?;
        let renewed_policy = self.renewal_request(&new_config, &sweep, addresses, new_phone)?;
        let renewed_destinations = renewed_policy.as_ref().map_or(0, |request| {
            request.allowance_destinations.len()
                + usize::from(request.emergency_destination.is_some())
        });
        let (backup_draft, backup_request) =
            self.prepare_rotation_backup(&new_config, new_phone)?;
        let request = DevicePhoneRotation {
            rotation: RotationRequest {
                version: wire::VERSION,
                network: protocol::config_network(&old_config)?,
                old_keys: protocol::vault_keys(&old_config)?,
                new_phone: protocol::key(new_phone.vault_pubkey),
                sweep_inputs: protocol::inputs(&inputs),
                revoke_connectors: protocol::inputs(&connectors),
                renewed_policy,
                previous_allowances_complete,
                sweep_nonces,
            },
            new_config,
            backup_request,
        };
        request.validate(&old_config)?;
        storage::write_json(
            &self.rotation_path(&request.id()?, "proposal")?,
            &PendingRotation {
                source,
                old_config,
                request: request.clone(),
                backup_draft,
                renewed_destinations,
            },
        )?;
        Ok(request)
    }

    /// A renewed policy funded by the output of `funding`, paying `addresses`.
    pub(super) fn renewal_request(
        &self,
        config: &VaultConfig,
        funding: &anzen_protocol::spend::Spend,
        addresses: &mut HotWallet,
        new_phone: &DeviceKeys,
    ) -> Result<Option<PolicyRequest>> {
        if config.monthly_limit_sats == 0 && config.emergency_access_limit_sats == 0 {
            return Ok(None);
        }
        let output = VaultUtxo {
            outpoint: protocol::bitcoin_outpoint(anzen_protocol::tx::OutPoint::new(
                funding.txid,
                0,
            )),
            txout: bitcoin::TxOut {
                value: bitcoin::Amount::from_sat(funding.amount_sats),
                script_pubkey: bitcoin::ScriptBuf::from_bytes(
                    funding.transaction.output[0].script_pubkey.clone(),
                ),
            },
            confirmation_height: 0,
        };
        Ok(Some(policy_ceremony::propose(
            config,
            &[output],
            &[],
            PolicyLimits {
                monthly_limit_sats: config.monthly_limit_sats,
                emergency_access_limit_sats: config.emergency_access_limit_sats,
            },
            addresses,
            false,
            cosigning::Phone {
                data_dir: &self.data_dir,
                keys: new_phone,
            },
        )?))
    }

    /// Broadcast an approved rotation and install the replacement key and vault. Every step is
    /// recorded before the next, so a retry resumes rather than creating a different spend.
    pub fn activate_rotation(
        &self,
        backend: &dyn HotWalletBackend,
        approval: &ApprovedDevicePhoneRotation,
    ) -> Result<DeviceRotationResult> {
        self.check_backend(backend)?;
        let pending: PendingRotation =
            storage::read_json(&self.rotation_path(&approval.request_id, "proposal")?)?;
        let receipt = self.rotation_path(&approval.request_id, "result")?;
        if receipt.storage_exists() {
            return storage::read_json(&receipt);
        }
        let old_config = &pending.old_config;
        let (old_phone, new_phone) = match pending.source {
            RotationSource::Current => {
                // Retain the retiring key before anything moves; a retry after the device file
                // was replaced still signs the old-vault sweep with it.
                let retained = self.rotation_path(&approval.request_id, "old-key")?;
                if !retained.storage_exists() {
                    storage::write_json(
                        &retained,
                        &storage::load_device(&self.data_dir, PHONE_DEVICE_FILE)?,
                    )?;
                }
                let pending_key: DeviceFile =
                    storage::read_json(&self.data_dir.join(PENDING_KEY_FILE)).or_else(|_| {
                        storage::read_json(&self.rotation_path(&approval.request_id, "new-key")?)
                    })?;
                storage::write_json(
                    &self.rotation_path(&approval.request_id, "new-key")?,
                    &pending_key,
                )?;
                (
                    device_keys(&storage::read_json(&retained)?)?,
                    device_keys(&pending_key)?,
                )
            }
            RotationSource::Restored => (
                self.restored_keys(old_config)?,
                load_device_keys(&self.data_dir, PHONE_DEVICE_FILE)?,
            ),
        };
        let finalized = crate::device_rotation::finalize(
            &self.data_dir,
            &pending.request,
            old_config,
            approval,
            &old_phone,
        )?;
        let policy = VaultPolicy::new_for_network(
            new_phone.vault_pubkey,
            XOnlyPublicKey::from_str(&old_config.hww_vault_pubkey)?,
            backend.network(),
        )?;
        let new_config = recovery::rotated_config(old_config, &new_phone, &policy)?;
        if serde_json::to_vec(&new_config)? != serde_json::to_vec(&approval.new_config)? {
            bail!("rotation does not belong to this phone's new keys and hot descriptors");
        }
        // Verify the renewed policy completely before the first broadcast: a sweep into the new
        // vault must never leave it without the policy the user approved.
        if let Some(renewed) = &finalized.renewed_policy {
            policy_ceremony::ApprovedPolicy::verify(
                &self.data_dir,
                &new_config,
                renewed,
                &new_phone,
            )?;
        }
        if pending.source == RotationSource::Restored
            && storage::config_exists(&self.data_dir)
            && serde_json::to_vec(&self.configuration()?)? != serde_json::to_vec(&new_config)?
        {
            bail!("refusing to replace an unrelated existing vault");
        }
        let response: DeviceBackupKeyResponse = device_channel::open(
            &new_phone,
            XOnlyPublicKey::from_str(&new_config.hww_vault_pubkey)?,
            &format!("cloud-backup-response/{}", new_config.vault_address),
            &approval.backup_response,
        )?;
        if response.backup_id
            != sha256::Hash::hash(&serde_json::to_vec(&pending.backup_draft)?).to_string()
        {
            bail!("rotation backup approval mismatch");
        }
        let backup = cloud_backup::complete_backup(pending.backup_draft, response.wrapped_key)?;
        // Durable approved request and backup precede any broadcast. Retrying follows these
        // exact transaction IDs rather than scanning already-spent old inputs or creating a new spend.
        storage::write_json(
            &self.rotation_path(&approval.request_id, "approved")?,
            approval,
        )?;
        storage::write_json(
            &self.rotation_path(&approval.request_id, "backup")?,
            &backup,
        )?;
        let controller_revocation_txid = finalized
            .revocation
            .as_ref()
            .map(|transaction| {
                self.broadcast_rotation_once(
                    backend,
                    &approval.request_id,
                    "controllers",
                    transaction,
                )
            })
            .transpose()?;
        let sweep_txid =
            self.broadcast_rotation_once(backend, &approval.request_id, "sweep", &finalized.sweep)?;
        let hot_wallet_sweep_txid = match pending.source {
            RotationSource::Current => {
                self.sweep_old_hot_wallet(backend, &approval.request_id, &new_phone)?
            }
            RotationSource::Restored => None,
        };
        if pending.source == RotationSource::Current {
            self.install_rotated_key(&approval.request_id, old_config, sweep_txid)?;
        }
        storage::write_json(&self.data_dir.join(storage::PHONE_BACKUP_FILE), &backup)?;
        storage::write_json(
            &self.data_dir.join(storage::HWW_PUBLIC_FILE),
            &PublicDeviceFile {
                version: 1,
                kind: "hww-public-key".into(),
                network: new_config.network.clone(),
                vault_pubkey: new_config.hww_vault_pubkey.clone(),
            },
        )?;
        storage::write_json(&self.data_dir.join(storage::CONFIG_FILE), &new_config)?;
        if pending.source == RotationSource::Current {
            // Reveal the replacement wallet's policy addresses so its database recognizes them.
            let mut wallet = HotWallet::open_or_create(&self.data_dir)?;
            for _ in 0..pending.renewed_destinations {
                wallet.next_receive_address()?;
            }
        }
        let renewed_schedule = finalized
            .renewed_policy
            .as_ref()
            .map(|renewed| self.activate_policy(backend, renewed))
            .transpose()?;
        let result = DeviceRotationResult {
            request_id: approval.request_id.clone(),
            old_vault_address: old_config.vault_address.clone(),
            new_vault_address: new_config.vault_address,
            sweep_txid,
            controller_revocation_txid,
            hot_wallet_sweep_txid,
            renewed_schedule,
        };
        storage::write_json(&receipt, &result)?;
        let pending_key = self.data_dir.join(PENDING_KEY_FILE);
        if pending_key.storage_exists() {
            anzen_common::storage::fs::remove_file(pending_key)?;
        }
        Ok(result)
    }

    /// Move the retiring key's spendable hot-wallet funds to the replacement wallet.
    fn sweep_old_hot_wallet(
        &self,
        backend: &dyn HotWalletBackend,
        id: &str,
        new_phone: &DeviceKeys,
    ) -> Result<Option<Txid>> {
        let recorded = self.rotation_path(id, "hot-sweep-transaction")?;
        let transaction: Option<Transaction> = if recorded.storage_exists() {
            storage::read_json(&recorded)?
        } else if storage::load_device(&self.data_dir, PHONE_DEVICE_FILE)?.mnemonic
            == new_phone.mnemonic.to_string()
        {
            None
        } else {
            let destination = HotWallet::ephemeral(new_phone)?.next_change_address()?;
            let mut old_wallet = HotWallet::open_or_create(&self.data_dir)?;
            backend.sync_hot_wallet(&mut old_wallet)?;
            let transaction = old_wallet.build_sweep(destination.script_pubkey())?;
            storage::write_json(&recorded, &transaction)?;
            transaction
        };
        transaction
            .map(|transaction| self.broadcast_rotation_once(backend, id, "hot-sweep", &transaction))
            .transpose()
    }

    /// Archive the retiring key and its epoch, then install the replacement key.
    fn install_rotated_key(
        &self,
        id: &str,
        old_config: &VaultConfig,
        sweep_txid: Txid,
    ) -> Result<()> {
        let new_key: DeviceFile = storage::read_json(&self.rotation_path(id, "new-key")?)?;
        let archive = self
            .data_dir
            .join("history")
            .join(format!("rotation-{sweep_txid}"));
        storage::write_json(&archive.join(storage::CONFIG_FILE), old_config)?;
        let old_key: DeviceFile = storage::read_json(&self.rotation_path(id, "old-key")?)?;
        storage::write_json(&archive.join(PHONE_DEVICE_FILE), &old_key)?;
        let old_backup = self.data_dir.join(PHONE_BACKUP_FILE);
        if old_backup.storage_exists() && !archive.join(PHONE_BACKUP_FILE).storage_exists() {
            let backup: cloud_backup::CloudRecoveryBackup = storage::read_json(&old_backup)?;
            storage::write_json(&archive.join(PHONE_BACKUP_FILE), &backup)?;
        }
        for relative in [
            policy_ceremony::SCHEDULE_FILE,
            "phone/transactions",
            "phone/hot-wallet.sqlite-wal",
            "phone/hot-wallet.sqlite-shm",
            "phone/hot-wallet.sqlite",
        ] {
            let source = self.data_dir.join(relative);
            if !source.storage_exists() {
                continue;
            }
            let destination = archive.join(relative);
            if let Some(parent) = destination.parent() {
                anzen_common::storage::fs::create_dir_all(parent)?;
            }
            anzen_common::storage::fs::rename(&source, &destination).with_context(|| {
                format!("failed to archive obsolete state {}", source.display())
            })?;
        }
        storage::write_json(&self.data_dir.join(PHONE_DEVICE_FILE), &new_key)
    }

    fn broadcast_rotation_once(
        &self,
        backend: &dyn HotWalletBackend,
        id: &str,
        name: &str,
        transaction: &Transaction,
    ) -> Result<Txid> {
        broadcast_once(backend, &self.rotation_path(id, name)?, transaction)
    }

    pub(super) fn prepare_rotation_backup(
        &self,
        config: &VaultConfig,
        phone: &DeviceKeys,
    ) -> Result<(CloudBackupDraft, DeviceMessage)> {
        let payload = zeroize::Zeroizing::new(RecoveryPayload::new(config, phone)?);
        let (draft, key) = cloud_backup::prepare_backup(&payload)?;
        let message = device_channel::seal(
            phone,
            XOnlyPublicKey::from_str(&config.hww_vault_pubkey)?,
            &format!("cloud-backup-request/{}", config.vault_address),
            &DeviceBackupKeyRequest {
                backup_id: sha256::Hash::hash(&serde_json::to_vec(&draft)?).to_string(),
                symmetric_key: *key,
            },
        )?;
        Ok((draft, message))
    }

    fn rotation_path(&self, id: &str, category: &str) -> Result<PathBuf> {
        if id.len() != 64
            || !id
                .bytes()
                .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
        {
            bail!("invalid rotation request ID");
        }
        Ok(self
            .data_dir
            .join("phone/device-rotations")
            .join(id)
            .join(format!("{category}.json")))
    }
}

fn device_keys(file: &DeviceFile) -> Result<DeviceKeys> {
    DeviceKeys::parse_for_network_at_index(
        &bitcoin::key::Secp256k1::new(),
        &file.mnemonic,
        file.bitcoin_network()?,
        file.vault_key_index,
    )
}

/// Whether the last approved policy had a monthly chain with no live output left to spend.
fn allowances_complete(last: Option<&PolicyApproval>, spent: &[VaultUtxo]) -> Result<bool> {
    let Some(approval) = last else {
        return Ok(false);
    };
    let (_, graph) = policy_ceremony::graph(&approval.request)?;
    let chain = graph.anchor().monthly_chain;
    Ok(!chain.is_empty()
        && !spent
            .iter()
            .any(|utxo| chain.contains(&protocol::outpoint(utxo.outpoint))))
}

pub(super) fn broadcast_once(
    backend: &dyn HotWalletBackend,
    path: &std::path::Path,
    transaction: &Transaction,
) -> Result<Txid> {
    let expected = transaction.compute_txid();
    if path.storage_exists() {
        let accepted: Txid = storage::read_json(path)?;
        if accepted != expected {
            bail!("rotation receipt mismatch");
        }
        return Ok(accepted);
    }
    let accepted = backend.broadcast(transaction)?;
    if accepted != expected {
        bail!("unexpected rotation transaction ID");
    }
    storage::write_json(path, &accepted)?;
    Ok(accepted)
}
