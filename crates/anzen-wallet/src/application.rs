//! Application operations shared by the CLI, native wallet and browser host.
//!
//! A caller supplies transport/platform capabilities, not wallet decisions. This facade never
//! loads the other device's private key. The path is chosen by the trusted host, not a web request.

use super::{HotWallet, HotWalletBackend, api::*};
use anzen_common::storage::fs::StoragePath;
mod backup;
mod offline_action;
mod recovered_wallet;
mod restore;
mod rotation;
use crate::ceremony;
use anyhow::{Context, Result, bail};
use anzen_common::{
    outbox,
    storage::{self, PHONE_DEVICE_FILE, load_device_keys, network_name},
};
use bitcoin::{
    Address, Psbt, Txid,
    hashes::{Hash, sha256},
};
pub use offline_action::RecoverySnapshot;
pub use restore::RestoredPhoneSummary;
pub use rotation::DeviceRotationResult;
use serde::{Deserialize, Serialize};
use std::{path::PathBuf, str::FromStr};

pub struct WalletApplication {
    data_dir: PathBuf,
    allow_mainnet: bool,
}

/// Version-one application commands, shared by native bindings and the local test transport.
#[derive(Deserialize)]
#[serde(tag = "method", rename_all = "snake_case", deny_unknown_fields)]
pub enum WalletRequest {
    Outbox,
    ReadOutbox {
        id: String,
    },
    Initialize {
        network: Network,
    },
    /// Replace a phone's keys with a recovery phrase's, before vault setup.
    ImportRecoveryPhrase {
        network: Network,
        words: String,
    },
    /// The words that restore this phone's keys.
    RecoveryPhrase,
    Identity,
    ImportRecoveryVault {
        config: VaultConfig,
    },
    ImportRecoveryBundle {
        bundle: Box<RecoveryVaultBundle>,
    },
    RecoverySource,
    RecoverySnapshot,
    ProposeOfflineAction {
        action: OfflineAction,
        destination: String,
    },
    ActivateOfflineAction {
        approval: Box<OfflineActionApproval>,
    },
    ProposeBackup,
    StoreBackup {
        message: Box<DeviceMessage>,
    },
    ExportBackup,
    ProposeRestore {
        backup: Box<CloudRecoveryBackup>,
    },
    StoreRestoredKey {
        message: Box<DeviceMessage>,
    },
    RestoredPhone,
    ReviewRecoveredWallet,
    ConfirmRecoveredWallet {
        review_id: String,
    },
    RejectRecoveredWallet {
        review_id: String,
    },
    ProposeRestoredRotation,
    /// Rotate this working phone to a fresh key, preserving the vault policy.
    ProposeRotation {
        /// Grind a vanity vault address for the new key with this engine.
        #[serde(default)]
        vanity: Option<super::VanityEngine>,
    },
    #[serde(alias = "activate_restored_rotation")]
    ActivateRotation {
        approval: Box<ApprovedDevicePhoneRotation>,
    },
    Session,
    PairSigner {
        signer: PublicDeviceFile,
    },
    /// The setup tap: pair with the signer and ask it to protect the phone's recovery backup.
    SetupVault {
        signer: PublicDeviceFile,
    },
    Configuration,
    WalletSnapshot,
    BackgroundWalletSnapshot,
    VaultSnapshot,
    WatchTargets,
    ReceiveAddress,
    Send {
        destination: String,
        amount_sats: u64,
    },
    ReviewPayment {
        destination: String,
        amount_sats: u64,
    },
    ConfirmPayment {
        review_id: String,
    },
    RejectPayment {
        review_id: String,
    },
    ProposePolicy {
        monthly_limit_sats: u64,
        emergency_access_limit_sats: u64,
    },
    RefreshPolicy,
    ActivatePolicy {
        approval: Box<PolicyApproval>,
    },
    Monthly {
        step: u8,
        action: TransactionKind,
    },
    ApplySoftLimit {
        step: u8,
        limit_sats: u64,
    },
    InitiateEmergency,
    WithdrawEmergency,
    CancelEmergency,
    ProposeSweep {
        destination: String,
    },
    ActivateSweep {
        approval: Box<SweepApproval>,
    },
}

impl WalletRequest {
    fn outgoing_kind(&self) -> Option<OutgoingMessageKind> {
        use OutgoingMessageKind as K;
        Some(match self {
            Self::PairSigner { .. } | Self::SetupVault { .. } => K::VaultSetup,
            Self::ProposePolicy { .. } | Self::RefreshPolicy => K::PolicyProposal,
            Self::ProposeSweep { .. } => K::SweepProposal,
            Self::ProposeBackup => K::BackupProposal,
            Self::ProposeRestore { .. } => K::RestoreProposal,
            Self::ProposeOfflineAction { .. } => K::OfflineActionProposal,
            Self::ProposeRestoredRotation | Self::ProposeRotation { .. } => {
                K::PhoneRotationProposal
            }
            Self::ActivateRotation { .. } => K::PhoneRotationCompleted,
            _ => return None,
        })
    }

    pub fn requires_chain(&self) -> bool {
        !matches!(
            self,
            Self::Outbox
                | Self::ReadOutbox { .. }
                | Self::Initialize { .. }
                | Self::ImportRecoveryPhrase { .. }
                | Self::RecoveryPhrase
                | Self::Identity
                | Self::ImportRecoveryVault { .. }
                | Self::ImportRecoveryBundle { .. }
                | Self::RecoverySource
                | Self::ProposeBackup
                | Self::StoreBackup { .. }
                | Self::ExportBackup
                | Self::ProposeRestore { .. }
                | Self::StoreRestoredKey { .. }
                | Self::RestoredPhone
                | Self::RejectRecoveredWallet { .. }
                | Self::Session
                | Self::PairSigner { .. }
                | Self::SetupVault { .. }
                | Self::Configuration
                | Self::WatchTargets
                | Self::ReceiveAddress
                | Self::RejectPayment { .. }
        )
    }
}

/// Deliberately contains no vault data: opening a wallet in public must not fetch its savings.
#[derive(Debug, Serialize)]
pub struct WalletSnapshot {
    pub network: String,
    /// Set when a background refresh skipped a long block-by-block catch-up: how many blocks
    /// the balances are behind. An explicit sync catches up.
    pub behind_blocks: Option<u64>,
    pub confirmed_sats: u64,
    pub total_sats: u64,
    pub available_sats: u64,
    pub pending_sats: u64,
    pub immature_sats: u64,
    pub activity: Vec<super::activity::WalletActivity>,
}

#[derive(Debug, Serialize)]
pub struct VaultSetup {
    pub config: VaultConfig,
    pub backup: DeviceMessage,
}

#[derive(Debug, Serialize)]
pub struct PaymentReview {
    pub review_id: String,
    pub network: String,
    pub destination: String,
    pub amount_sats: u64,
    pub fee_sats: u64,
    pub total_sats: u64,
}

#[derive(Serialize, Deserialize)]
struct PendingPayment {
    identity: PublicDeviceFile,
    destination: String,
    amount_sats: u64,
    fee_sats: u64,
    psbt: String,
}

#[derive(Debug, Serialize)]
pub struct VaultSnapshot {
    pub config: VaultConfig,
    pub balance_sats: u64,
    pub utxo_count: usize,
    pub chain_height: u64,
    pub median_time: u64,
    /// When the signer could first move savings alone: the tip median time past at which the
    /// oldest coin's time lock opens. Renewing the vault moves every coin and restarts it.
    pub hww_recovery_opens_at: Option<u64>,
    pub schedule: Option<Schedule>,
    pub recovery_backup_stored: bool,
    pub recovered_wallet_available: bool,
}

impl WalletApplication {
    pub fn new(data_dir: PathBuf, allow_mainnet: bool) -> Self {
        Self {
            data_dir,
            allow_mainnet,
        }
    }

    /// The directory chosen by the trusted host for this phone's state.
    pub fn data_dir(&self) -> &std::path::Path {
        &self.data_dir
    }

    pub fn execute(
        &self,
        request: WalletRequest,
        backend: Option<&dyn HotWalletBackend>,
    ) -> Result<serde_json::Value> {
        let chain = || backend.context("this wallet operation requires a chain connection");
        let outgoing_kind = request.outgoing_kind();
        let result = match request {
            WalletRequest::Outbox => serde_json::json!({"messages": self.outbox()?}),
            WalletRequest::ReadOutbox { id } => serde_json::to_value(self.read_outbox(&id)?)?,
            WalletRequest::Initialize { network } => {
                serde_json::to_value(self.initialize(network)?)?
            }
            WalletRequest::ImportRecoveryPhrase { network, words } => {
                serde_json::to_value(self.import_recovery_phrase(network, &words)?)?
            }
            WalletRequest::RecoveryPhrase => {
                serde_json::json!({ "words": self.recovery_phrase()? })
            }
            WalletRequest::Identity => serde_json::to_value(self.public_identity()?)?,
            WalletRequest::ImportRecoveryVault { config } => {
                self.import_recovery_vault(&config)?;
                serde_json::to_value(config)?
            }
            WalletRequest::RecoverySource => serde_json::to_value(self.recovery_source()?)?,
            WalletRequest::RecoverySnapshot => {
                serde_json::to_value(self.recovery_snapshot(chain()?)?)?
            }
            WalletRequest::ImportRecoveryBundle { bundle } => {
                self.import_recovery_bundle(&bundle)?;
                serde_json::to_value(bundle.config)?
            }
            WalletRequest::ProposeOfflineAction {
                action,
                destination,
            } => serde_json::to_value(self.propose_offline_action(
                chain()?,
                action,
                &destination,
            )?)?,
            WalletRequest::ActivateOfflineAction { approval } => {
                serde_json::json!({"txid": self.activate_offline_action(chain()?, &approval)?})
            }
            WalletRequest::ProposeBackup => serde_json::to_value(self.propose_backup()?)?,
            WalletRequest::StoreBackup { message } => {
                self.store_backup(&message)?;
                serde_json::json!({"stored": true})
            }
            WalletRequest::ExportBackup => serde_json::to_value(self.export_backup()?)?,
            WalletRequest::ProposeRestore { backup } => {
                serde_json::to_value(self.propose_restore(&backup)?)?
            }
            WalletRequest::StoreRestoredKey { message } => {
                serde_json::to_value(self.store_restored_key(&message)?)?
            }
            WalletRequest::RestoredPhone => serde_json::to_value(self.restored_phone()?)?,
            WalletRequest::ReviewRecoveredWallet => {
                serde_json::to_value(self.review_recovered_wallet(chain()?)?)?
            }
            WalletRequest::ConfirmRecoveredWallet { review_id } => {
                serde_json::json!({"txid": self.confirm_recovered_wallet(chain()?, &review_id)?})
            }
            WalletRequest::RejectRecoveredWallet { review_id } => {
                self.reject_recovered_wallet(&review_id)?;
                serde_json::json!({"rejected": true})
            }
            WalletRequest::ProposeRestoredRotation => {
                serde_json::to_value(self.propose_restored_rotation(chain()?)?)?
            }
            WalletRequest::ProposeRotation { vanity } => {
                serde_json::to_value(self.propose_rotation(chain()?, vanity, |_| {})?)?
            }
            WalletRequest::ActivateRotation { approval } => {
                serde_json::to_value(self.activate_rotation(chain()?, &approval)?)?
            }
            WalletRequest::Session => serde_json::json!({
                "identity": if self.data_dir.join(PHONE_DEVICE_FILE).storage_exists() { Some(self.public_identity()?) } else { None },
                "paired": storage::config_exists(&self.data_dir),
                "backup_stored": self.data_dir.join(storage::PHONE_BACKUP_FILE).storage_exists(),
            }),
            WalletRequest::PairSigner { signer } => {
                serde_json::to_value(self.pair_signer(&signer)?)?
            }
            WalletRequest::SetupVault { signer } => {
                serde_json::to_value(self.setup_vault(&signer)?)?
            }
            WalletRequest::Configuration => serde_json::to_value(self.configuration()?)?,
            WalletRequest::WalletSnapshot => serde_json::to_value(self.wallet_snapshot(chain()?)?)?,
            WalletRequest::BackgroundWalletSnapshot => {
                serde_json::to_value(self.background_wallet_snapshot(chain()?)?)?
            }
            WalletRequest::VaultSnapshot => serde_json::to_value(self.vault_snapshot(chain()?)?)?,
            WalletRequest::WatchTargets => serde_json::to_value(self.watch_targets()?)?,
            WalletRequest::ReceiveAddress => {
                serde_json::json!({ "address": self.receive_address()? })
            }
            WalletRequest::Send {
                destination,
                amount_sats,
            } => serde_json::json!({ "txid": self.send(chain()?, &destination, amount_sats)? }),
            WalletRequest::ReviewPayment {
                destination,
                amount_sats,
            } => serde_json::to_value(self.review_payment(chain()?, &destination, amount_sats)?)?,
            WalletRequest::ConfirmPayment { review_id } => {
                serde_json::json!({ "txid": self.confirm_payment(chain()?, &review_id)? })
            }
            WalletRequest::RejectPayment { review_id } => {
                self.public_identity()?;
                let path = self.payment_path(&review_id)?;
                if path.storage_exists() {
                    anzen_common::storage::fs::remove_file(path)?;
                }
                serde_json::json!({"rejected": true})
            }
            WalletRequest::ProposePolicy {
                monthly_limit_sats,
                emergency_access_limit_sats,
            } => serde_json::to_value(self.propose_policy(
                chain()?,
                PolicyLimits {
                    monthly_limit_sats,
                    emergency_access_limit_sats,
                },
            )?)?,
            WalletRequest::RefreshPolicy => serde_json::to_value(self.refresh_policy(chain()?)?)?,
            WalletRequest::ActivatePolicy { approval } => {
                serde_json::to_value(self.activate_policy(chain()?, &approval)?)?
            }
            WalletRequest::Monthly { step, action } => {
                serde_json::json!({ "txid": self.monthly(chain()?, step, action)? })
            }
            WalletRequest::ApplySoftLimit { step, limit_sats } => {
                serde_json::json!({ "txid": self.apply_soft_limit(chain()?, step, limit_sats)? })
            }
            WalletRequest::InitiateEmergency => {
                serde_json::json!({ "txid": self.initiate_emergency(chain()?)? })
            }
            WalletRequest::WithdrawEmergency => {
                serde_json::json!({ "txid": self.withdraw_emergency(chain()?)? })
            }
            WalletRequest::CancelEmergency => {
                serde_json::json!({ "txid": self.cancel_emergency(chain()?)? })
            }
            WalletRequest::ProposeSweep { destination } => {
                serde_json::to_value(self.propose_sweep(chain()?, &destination)?)?
            }
            WalletRequest::ActivateSweep { approval } => {
                serde_json::to_value(self.activate_sweep(chain()?, &approval)?)?
            }
        };
        if let Some(kind) = outgoing_kind {
            outbox::save(
                &self.data_dir.join("phone/outbox"),
                self.public_identity()?,
                kind,
                result.clone(),
            )
            .context("wallet operation completed, but its outgoing message could not be saved")?;
        }
        Ok(result)
    }

    pub fn outbox(&self) -> Result<Vec<OutboxItem>> {
        outbox::list(
            &self.data_dir.join("phone/outbox"),
            &self.public_identity()?,
        )
    }

    pub fn read_outbox(&self, id: &str) -> Result<OutgoingMessage> {
        outbox::read(
            &self.data_dir.join("phone/outbox"),
            &self.public_identity()?,
            id,
        )
    }

    pub fn initialize(&self, network: Network) -> Result<PublicDeviceFile> {
        self.check_access(network)?;
        super::initialize(&self.data_dir, network)?;
        self.public_identity()
    }

    pub fn import_recovery_phrase(
        &self,
        network: Network,
        words: &str,
    ) -> Result<PublicDeviceFile> {
        self.check_access(network)?;
        super::import_phone(&self.data_dir, network, words)?;
        self.public_identity()
    }

    pub fn recovery_phrase(&self) -> Result<String> {
        self.public_identity()?;
        Ok(storage::load_device(&self.data_dir, PHONE_DEVICE_FILE)?.mnemonic)
    }

    /// Grind a phone key whose vault address with `signer` begins with the vanity prefix.
    pub fn initialize_vanity<F>(
        &self,
        signer: &PublicDeviceFile,
        engine: super::VanityEngine,
        report_progress: F,
    ) -> Result<super::VanityInitialization>
    where
        F: Fn(u64) + Sync,
    {
        self.check_access(signer.bitcoin_network()?)?;
        super::initialize_vanity(&self.data_dir, signer, engine, report_progress)
    }

    pub fn public_identity(&self) -> Result<PublicDeviceFile> {
        let phone = load_device_keys(&self.data_dir, PHONE_DEVICE_FILE)?;
        self.check_access(phone.network)?;
        Ok(PublicDeviceFile {
            version: 1,
            kind: "phone-public-key".into(),
            network: network_name(phone.network).into(),
            vault_pubkey: phone.vault_pubkey.to_string(),
        })
    }

    /// Public-key exchange only. The cold signer separately validates and accepts this config.
    pub fn pair_signer(&self, signer: &PublicDeviceFile) -> Result<VaultConfig> {
        let network = self.public_identity()?.bitcoin_network()?;
        crate::pairing::initialize_vault_with_public_hww(&self.data_dir, network, signer)
    }

    /// Everything vault setup needs from the signer, for one exchange: the vault to accept and
    /// the phone's first recovery backup to protect. Repeating it with the same signer, after an
    /// interrupted tap, sends the same vault again; a different signer is refused.
    pub fn setup_vault(&self, signer: &PublicDeviceFile) -> Result<VaultSetup> {
        if self
            .data_dir
            .join(storage::PHONE_BACKUP_FILE)
            .storage_exists()
        {
            bail!("this vault is already set up");
        }
        let config = if storage::config_exists(&self.data_dir) {
            let config = self.configuration()?;
            if config.hww_vault_pubkey != signer.vault_pubkey || config.network != signer.network {
                bail!("this vault is being set up with a different signer");
            }
            config
        } else {
            self.pair_signer(signer)?
        };
        Ok(VaultSetup {
            backup: self.propose_backup()?,
            config,
        })
    }

    pub fn configuration(&self) -> Result<VaultConfig> {
        let config = storage::load_config(&self.data_dir)?;
        self.check_access(config.bitcoin_network()?)?;
        config.validate_public_policy()?;
        let phone = self.public_identity()?;
        if phone.vault_pubkey != config.phone_vault_pubkey || phone.network != config.network {
            bail!("vault configuration does not match this phone");
        }
        Ok(config)
    }

    pub fn wallet_snapshot(&self, backend: &dyn HotWalletBackend) -> Result<WalletSnapshot> {
        self.check_backend(backend)?;
        let mut wallet = HotWallet::open_or_create(&self.data_dir)?;
        backend.sync_hot_wallet(&mut wallet)?;
        Ok(Self::snapshot(&wallet, backend.network(), None))
    }

    /// The automatic refresh behind live updates. Regtest syncs through Bitcoin Core by walking
    /// every block since the last sync (slow for a never-synced wallet on a long chain), and the device can do nothing else meanwhile. So a
    /// background refresh that far behind returns the last synced state instead, and an
    /// explicit sync catches up.
    pub fn background_wallet_snapshot(
        &self,
        backend: &dyn HotWalletBackend,
    ) -> Result<WalletSnapshot> {
        const MAX_BACKGROUND_BLOCKS: u64 = 1_000;
        self.check_backend(backend)?;
        let mut wallet = HotWallet::open_or_create(&self.data_dir)?;
        if backend.network() == Network::Regtest && !wallet.starts_at_tip()? {
            let synced = u64::from(wallet.wallet.latest_checkpoint().height());
            let behind = backend.chain_tip()?.height.saturating_sub(synced);
            if behind > MAX_BACKGROUND_BLOCKS {
                return Ok(Self::snapshot(&wallet, backend.network(), Some(behind)));
            }
        }
        backend.sync_hot_wallet(&mut wallet)?;
        Ok(Self::snapshot(&wallet, backend.network(), None))
    }

    fn snapshot(
        wallet: &HotWallet,
        network: Network,
        behind_blocks: Option<u64>,
    ) -> WalletSnapshot {
        let balance = wallet.wallet.balance();
        WalletSnapshot {
            network: network_name(network).into(),
            behind_blocks,
            confirmed_sats: balance.confirmed.to_sat(),
            total_sats: balance.total().to_sat(),
            available_sats: balance.trusted_spendable().to_sat(),
            pending_sats: balance.untrusted_pending.to_sat(),
            immature_sats: balance.immature.to_sat(),
            activity: super::activity::wallet_activity(&wallet.wallet),
        }
    }

    pub fn vault_snapshot(&self, backend: &dyn HotWalletBackend) -> Result<VaultSnapshot> {
        self.check_backend(backend)?;
        let config = self.configuration()?;
        let tip = backend.chain_tip()?;
        let utxos = backend.scan_vault(&config)?;
        let oldest = utxos.iter().min_by_key(|utxo| utxo.confirmation_height);
        let hww_recovery_opens_at = match oldest {
            Some(oldest) => Some(
                crate::recovery::RecoveryClock::read(backend, &tip, std::slice::from_ref(oldest))?
                    .opens_at(oldest)?,
            ),
            None => None,
        };
        let balance_sats = utxos.iter().try_fold(0u64, |sum, utxo| {
            sum.checked_add(utxo.txout.value.to_sat())
                .context("vault balance overflow")
        })?;
        let schedule = if self.data_dir.join(ceremony::SCHEDULE_FILE).storage_exists() {
            Some(super::load_schedule(&self.data_dir)?)
        } else {
            None
        };
        Ok(VaultSnapshot {
            hww_recovery_opens_at,
            config,
            balance_sats,
            utxo_count: utxos.len(),
            chain_height: tip.height,
            median_time: tip.median_time,
            schedule,
            recovery_backup_stored: self
                .data_dir
                .join(storage::PHONE_BACKUP_FILE)
                .storage_exists(),
            recovered_wallet_available: self.restored_phone().is_ok(),
        })
    }

    /// Public scripts whose chain activity should refresh this wallet. Reads local state only.
    pub fn watch_targets(&self) -> Result<crate::watch::WatchTargets> {
        self.public_identity()?;
        let wallet = HotWallet::open_or_create(&self.data_dir)?;
        if !storage::config_exists(&self.data_dir) {
            return crate::watch::targets(&wallet, &[]);
        }
        let config = self.configuration()?;
        let vault = Address::from_str(&config.vault_address)?
            .require_network(config.bitcoin_network()?)?
            .script_pubkey();
        let controller = super::controller_policy(&config)?.address.script_pubkey();
        crate::watch::targets(&wallet, &[&vault, &controller])
    }

    pub fn receive_address(&self) -> Result<String> {
        self.public_identity()?;
        Ok(HotWallet::open_or_create(&self.data_dir)?
            .next_receive_address()?
            .to_string())
    }

    pub fn propose_sweep(
        &self,
        backend: &dyn HotWalletBackend,
        destination: &str,
    ) -> Result<SweepRequest> {
        self.check_backend(backend)?;
        self.configuration()?;
        let destination = Address::from_str(destination)?.require_network(backend.network())?;
        super::create_cooperative_sweep(&self.data_dir, backend, &destination)
    }

    pub fn activate_sweep(
        &self,
        backend: &dyn HotWalletBackend,
        approval: &SweepApproval,
    ) -> Result<SweepResult> {
        self.check_backend(backend)?;
        self.configuration()?;
        super::broadcast_cooperative_sweep(&self.data_dir, backend, approval)
    }

    pub fn review_payment(
        &self,
        backend: &dyn HotWalletBackend,
        destination: &str,
        amount_sats: u64,
    ) -> Result<PaymentReview> {
        self.check_backend(backend)?;
        let destination = Address::from_str(destination)?.require_network(backend.network())?;
        let mut wallet = HotWallet::open_or_create(&self.data_dir)?;
        backend.sync_hot_wallet(&mut wallet)?;
        let (psbt, fee_sats) = wallet.prepare_payment(destination.script_pubkey(), amount_sats)?;
        let pending = PendingPayment {
            identity: self.public_identity()?,
            destination: destination.to_string(),
            amount_sats,
            fee_sats,
            psbt: psbt.to_string(),
        };
        let review_id = sha256::Hash::hash(&serde_json::to_vec(&pending)?).to_string();
        storage::write_json(&self.payment_path(&review_id)?, &pending)?;
        Ok(PaymentReview {
            review_id,
            network: pending.identity.network,
            destination: pending.destination,
            amount_sats,
            fee_sats,
            total_sats: amount_sats
                .checked_add(fee_sats)
                .context("payment total overflow")?,
        })
    }

    /// Sign and broadcast exactly what was reviewed. A retry returns the same txid, never a new
    /// payment. A conflicting/spent input fails at the backend instead of selecting new coins.
    pub fn confirm_payment(&self, backend: &dyn HotWalletBackend, review_id: &str) -> Result<Txid> {
        self.check_backend(backend)?;
        let pending: PendingPayment = storage::read_json(&self.payment_path(review_id)?)
            .context("review the payment before confirming it")?;
        if sha256::Hash::hash(&serde_json::to_vec(&pending)?).to_string() != review_id
            || serde_json::to_vec(&self.public_identity()?)?
                != serde_json::to_vec(&pending.identity)?
        {
            bail!("payment or wallet identity changed since review");
        }
        let mut wallet = HotWallet::open_or_create(&self.data_dir)?;
        let transaction = wallet.sign_payment(Psbt::from_str(&pending.psbt)?)?;
        let txid = backend.broadcast(&transaction)?;
        if txid != transaction.compute_txid() {
            bail!("backend returned an unexpected payment transaction ID");
        }
        Ok(txid)
    }

    fn payment_path(&self, review_id: &str) -> Result<PathBuf> {
        if review_id.len() != 64
            || !review_id
                .bytes()
                .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
        {
            bail!("invalid payment review ID");
        }
        Ok(self
            .data_dir
            .join("phone/payment-reviews")
            .join(format!("{review_id}.json")))
    }

    /// The UI calls this only after explicit payment confirmation. Rust checks all inputs again.
    pub fn send(
        &self,
        backend: &dyn HotWalletBackend,
        destination: &str,
        amount_sats: u64,
    ) -> Result<Txid> {
        self.check_backend(backend)?;
        let address = Address::from_str(destination)?.require_network(backend.network())?;
        if amount_sats == 0 {
            bail!("payment amount must be greater than zero");
        }
        let mut wallet = HotWallet::open_or_create(&self.data_dir)?;
        backend.sync_hot_wallet(&mut wallet)?;
        let (transaction, _) = wallet.build_payment(address.script_pubkey(), amount_sats)?;
        let expected = transaction.compute_txid();
        let actual = backend.broadcast(&transaction)?;
        if actual != expected {
            bail!("backend returned an unexpected payment transaction ID");
        }
        Ok(actual)
    }

    pub fn propose_policy(
        &self,
        backend: &dyn HotWalletBackend,
        limits: PolicyLimits,
    ) -> Result<PolicyRequest> {
        self.check_backend(backend)?;
        self.configuration()?;
        super::propose_policy(&self.data_dir, backend, limits)
    }

    pub fn refresh_policy(&self, backend: &dyn HotWalletBackend) -> Result<PolicyRequest> {
        let config = self.configuration()?;
        self.propose_policy(
            backend,
            PolicyLimits {
                monthly_limit_sats: config.monthly_limit_sats,
                emergency_access_limit_sats: config.emergency_access_limit_sats,
            },
        )
    }

    pub fn activate_policy(
        &self,
        backend: &dyn HotWalletBackend,
        approval: &PolicyApproval,
    ) -> Result<Schedule> {
        self.check_backend(backend)?;
        self.configuration()?;
        super::activate_policy(&self.data_dir, backend, approval)
    }

    pub fn monthly(
        &self,
        backend: &dyn HotWalletBackend,
        step: u8,
        action: TransactionKind,
    ) -> Result<Txid> {
        self.check_backend(backend)?;
        self.configuration()?;
        Ok(super::broadcast_monthly(&self.data_dir, backend, step, action)?.transaction_txid)
    }

    pub fn apply_soft_limit(
        &self,
        backend: &dyn HotWalletBackend,
        step: u8,
        limit_sats: u64,
    ) -> Result<Option<Txid>> {
        self.check_backend(backend)?;
        self.configuration()?;
        super::apply_soft_limit(&self.data_dir, backend, step, limit_sats)
    }

    pub fn initiate_emergency(&self, backend: &dyn HotWalletBackend) -> Result<Txid> {
        self.check_backend(backend)?;
        self.configuration()?;
        Ok(super::initiate_emergency_access(&self.data_dir, backend)?.transaction_txid)
    }

    pub fn withdraw_emergency(&self, backend: &dyn HotWalletBackend) -> Result<Txid> {
        self.check_backend(backend)?;
        self.configuration()?;
        super::withdraw_emergency_access(&self.data_dir, backend)
    }

    pub fn cancel_emergency(&self, backend: &dyn HotWalletBackend) -> Result<Txid> {
        self.check_backend(backend)?;
        self.configuration()?;
        super::cancel_emergency_access(&self.data_dir, backend)
    }

    fn check_access(&self, network: Network) -> Result<()> {
        storage::validate_supported_network(network)?;
        if network == Network::Bitcoin && !self.allow_mainnet {
            bail!("mainnet requires explicit dangerously-enable-mainnet opt-in");
        }
        Ok(())
    }

    fn check_backend(&self, backend: &dyn HotWalletBackend) -> Result<()> {
        let network = self.public_identity()?.bitcoin_network()?;
        if network != backend.network() {
            bail!("chain backend does not match this phone's network");
        }
        Ok(())
    }
}
