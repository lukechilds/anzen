//! Offline application facade: explicit pairing, validated review, then approval of that review.
//! The transport cannot swap a request between the review and approval calls, and every
//! signature comes from `anzen_signer` rebuilding the request's transactions itself.

use super::{SoftwareKeys, api::*};
use anzen_common::storage::fs::StoragePath;
mod backup;
mod offline_action;
mod restore;
mod rotation;
use anyhow::{Context, Result, bail};
use anzen_common::{
    keys::DeviceKeys,
    outbox,
    protocol::{self, SoftwareCrypto},
    storage::{self, HWW_DEVICE_FILE, HWW_PUBLIC_FILE, load_device_keys},
};
use anzen_protocol::policy::Linkage;
use anzen_signer::{Signer, State};
pub use backup::{BackupReview, SetupAccepted};
use bitcoin::hashes::{Hash, sha256};
pub use offline_action::OfflineActionReview;
pub use restore::RestoreReview;
pub use rotation::PhoneRotationReview;
use serde::{Deserialize, Serialize};
use std::{path::PathBuf, str::FromStr};

/// The public vault this signer accepted. Separate from the phone's configuration file so a
/// combined simulation directory cannot let one device's state stand in for the other's.
const SIGNER_CONFIG_FILE: &str = "hww/vault-config.json";
const SIGNER_STATE_FILE: &str = "hww/signer-state.json";

pub struct SignerApplication {
    data_dir: PathBuf,
    allow_mainnet: bool,
}

#[derive(Deserialize)]
#[serde(tag = "method", rename_all = "snake_case", deny_unknown_fields)]
pub enum SignerRequest {
    Outbox,
    ReadOutbox {
        id: String,
    },
    Initialize {
        network: Network,
    },
    Identity,
    ExportVault,
    ExportRecoveryBundle,
    ReviewPhoneRotation {
        request: Box<DevicePhoneRotation>,
    },
    ApprovePhoneRotation {
        review_id: String,
        request: Box<DevicePhoneRotation>,
    },
    RejectPhoneRotation {
        review_id: String,
    },
    AdoptPhoneRotation {
        request_id: String,
    },
    ReviewOfflineAction {
        request: Box<OfflineActionRequest>,
    },
    ApproveOfflineAction {
        review_id: String,
    },
    RejectOfflineAction {
        review_id: String,
    },
    ReviewBackup {
        message: Box<DeviceMessage>,
    },
    ApproveBackup {
        review_id: String,
        message: Box<DeviceMessage>,
    },
    RejectBackup {
        review_id: String,
    },
    ReviewRestore {
        request: Box<RestoreKeyRequest>,
    },
    ApproveRestore {
        review_id: String,
    },
    RejectRestore {
        review_id: String,
    },
    Session,
    AcceptVault {
        config: VaultConfig,
    },
    /// The setup tap: accept the vault and protect the phone's first recovery backup.
    AcceptSetup {
        config: VaultConfig,
        backup: Box<DeviceMessage>,
    },
    ReviewPolicy {
        request: Box<PolicyRequest>,
    },
    ApprovePolicy {
        review_id: String,
    },
    RejectPolicy {
        review_id: String,
    },
    /// Signer-local: forget the last approved policy after the user confirms it was never
    /// activated. A phone cannot trigger this.
    ForgetApprovedPolicy,
    ReviewSweep {
        request: Box<SweepRequest>,
    },
    ApproveSweep {
        review_id: String,
    },
    RejectSweep {
        review_id: String,
    },
}

#[derive(Debug, Serialize)]
pub struct PolicyReview {
    pub review_id: String,
    pub network: String,
    pub vault_address: String,
    pub vault_input_sats: u64,
    pub monthly_limit_sats: u64,
    pub funded_withdrawals: usize,
    pub funding_warning: Option<String>,
    pub emergency_access_limit_sats: u64,
    pub emergency_delay_seconds: Option<u32>,
    pub rollover_fee_sats: u64,
    pub transaction_count: usize,
    pub allowance_destinations: Vec<String>,
    pub emergency_destination: Option<String>,
    pub previous_policy: Linkage,
    pub previous_policy_warning: Option<String>,
}

#[derive(Serialize, Deserialize)]
struct PendingPolicy {
    state: State,
    request: PolicyRequest,
}

#[derive(Serialize, Deserialize)]
struct PendingSweep {
    state: State,
    request: SweepRequest,
}

#[derive(Debug, Serialize)]
pub struct SweepReview {
    pub review_id: String,
    pub network: String,
    pub destination: String,
    pub amount_sats: u64,
    pub fee_sats: u64,
    pub input_count: usize,
}

/// A warning the signer shows when it cannot itself confirm the previous policy is finished.
pub(crate) fn linkage_warning(linkage: Linkage) -> Option<String> {
    (linkage == Linkage::MonthlyReportedComplete).then(|| {
        "The phone reports that every monthly withdrawal of the previous policy was used. Approve only if that is true; otherwise the remaining withdrawals stay usable alongside this policy.".to_owned()
    })
}

impl SignerApplication {
    pub fn new(data_dir: PathBuf, allow_mainnet: bool) -> Self {
        Self {
            data_dir,
            allow_mainnet,
        }
    }

    pub fn execute(&self, request: SignerRequest) -> Result<serde_json::Value> {
        let outgoing_kind = match &request {
            SignerRequest::Initialize { .. } | SignerRequest::Identity => {
                Some(OutgoingMessageKind::SignerIdentity)
            }
            SignerRequest::AcceptVault { .. } | SignerRequest::AcceptSetup { .. } => {
                Some(OutgoingMessageKind::VaultAccepted)
            }
            SignerRequest::ExportRecoveryBundle => Some(OutgoingMessageKind::RecoveryVault),
            SignerRequest::ApprovePolicy { .. } => Some(OutgoingMessageKind::PolicyApproved),
            SignerRequest::ApproveSweep { .. } => Some(OutgoingMessageKind::SweepApproved),
            SignerRequest::ApproveBackup { .. } => Some(OutgoingMessageKind::BackupApproved),
            SignerRequest::ApproveRestore { .. } => Some(OutgoingMessageKind::RestoreApproved),
            SignerRequest::ApproveOfflineAction { .. } => {
                Some(OutgoingMessageKind::OfflineActionApproved)
            }
            SignerRequest::ApprovePhoneRotation { .. } => {
                Some(OutgoingMessageKind::PhoneRotationApproved)
            }
            _ => None,
        };
        let result = match request {
            SignerRequest::Outbox => serde_json::json!({"messages": self.outbox()?}),
            SignerRequest::ReadOutbox { id } => serde_json::to_value(self.read_outbox(&id)?)?,
            SignerRequest::Initialize { network } => {
                serde_json::to_value(self.initialize(network)?)?
            }
            SignerRequest::Identity => serde_json::to_value(self.public_identity()?)?,
            SignerRequest::ReviewPhoneRotation { request } => {
                serde_json::to_value(self.review_phone_rotation(&request)?)?
            }
            SignerRequest::ApprovePhoneRotation { review_id, request } => {
                serde_json::to_value(self.approve_phone_rotation(&review_id, &request)?)?
            }
            SignerRequest::RejectPhoneRotation { review_id } => {
                self.reject_phone_rotation(&review_id)?;
                serde_json::json!({"rejected": true})
            }
            SignerRequest::AdoptPhoneRotation { request_id } => {
                serde_json::to_value(self.adopt_phone_rotation(&request_id)?)?
            }
            SignerRequest::ExportVault => serde_json::to_value(self.configuration()?)?,
            SignerRequest::ExportRecoveryBundle => {
                serde_json::to_value(self.export_recovery_bundle()?)?
            }
            SignerRequest::ReviewRestore { request } => {
                serde_json::to_value(self.review_restore(&request)?)?
            }
            SignerRequest::ApproveRestore { review_id } => {
                serde_json::to_value(self.approve_restore(&review_id)?)?
            }
            SignerRequest::RejectRestore { review_id } => {
                self.reject_restore(&review_id)?;
                serde_json::json!({"rejected": true})
            }
            SignerRequest::ReviewOfflineAction { request } => {
                serde_json::to_value(self.review_offline_action(&request)?)?
            }
            SignerRequest::ApproveOfflineAction { review_id } => {
                serde_json::to_value(self.approve_offline_action(&review_id)?)?
            }
            SignerRequest::RejectOfflineAction { review_id } => {
                self.reject_offline_action(&review_id)?;
                serde_json::json!({"rejected": true})
            }
            SignerRequest::ReviewBackup { message } => {
                serde_json::to_value(self.review_backup(&message)?)?
            }
            SignerRequest::ApproveBackup { review_id, message } => {
                serde_json::to_value(self.approve_backup(&review_id, &message)?)?
            }
            SignerRequest::RejectBackup { review_id } => {
                self.reject_backup(&review_id)?;
                serde_json::json!({ "rejected": true })
            }
            SignerRequest::Session => serde_json::json!({
                "identity": if self.data_dir.join(HWW_DEVICE_FILE).storage_exists() { Some(self.public_identity()?) } else { None },
                "config": if self.paired() { Some(self.configuration()?) } else { None },
            }),
            SignerRequest::AcceptVault { config } => {
                self.accept_vault(&config)?;
                serde_json::json!({ "accepted": true })
            }
            SignerRequest::AcceptSetup { config, backup } => {
                serde_json::to_value(self.accept_setup(&config, &backup)?)?
            }
            SignerRequest::ReviewPolicy { request } => {
                serde_json::to_value(self.review_policy(&request)?)?
            }
            SignerRequest::ApprovePolicy { review_id } => {
                serde_json::to_value(self.approve_policy(&review_id)?)?
            }
            SignerRequest::RejectPolicy { review_id } => {
                self.reject_policy(&review_id)?;
                serde_json::json!({ "rejected": true })
            }
            SignerRequest::ForgetApprovedPolicy => {
                self.forget_approved_policy()?;
                serde_json::json!({ "forgotten": true })
            }
            SignerRequest::ReviewSweep { request } => {
                serde_json::to_value(self.review_sweep(&request)?)?
            }
            SignerRequest::ApproveSweep { review_id } => {
                serde_json::to_value(self.approve_sweep(&review_id)?)?
            }
            SignerRequest::RejectSweep { review_id } => {
                self.configuration()?;
                self.remove_review(self.sweep_review_path(&review_id)?)?;
                serde_json::json!({ "rejected": true })
            }
        };
        if let Some(kind) = outgoing_kind {
            outbox::save(
                &self.data_dir.join("hww/outbox"),
                self.public_identity()?,
                kind,
                result.clone(),
            )
            .context("signer operation completed, but its outgoing message could not be saved")?;
        }
        Ok(result)
    }

    pub fn outbox(&self) -> Result<Vec<OutboxItem>> {
        outbox::list(&self.data_dir.join("hww/outbox"), &self.public_identity()?)
    }

    pub fn read_outbox(&self, id: &str) -> Result<OutgoingMessage> {
        outbox::read(
            &self.data_dir.join("hww/outbox"),
            &self.public_identity()?,
            id,
        )
    }

    pub fn initialize(&self, network: Network) -> Result<PublicDeviceFile> {
        self.check_access(network)?;
        super::initialize(&self.data_dir, network)?;
        self.public_identity()
    }

    pub fn public_identity(&self) -> Result<PublicDeviceFile> {
        let identity = storage::load_public_device(&self.data_dir, HWW_PUBLIC_FILE)?;
        identity.validate_kind("hww-public-key")?;
        self.check_access(identity.bitcoin_network()?)?;
        let hww = self.keys()?;
        if identity.vault_pubkey != hww.vault_pubkey.to_string()
            || identity.bitcoin_network()? != hww.network
        {
            bail!("public identity does not match this signer");
        }
        Ok(identity)
    }

    /// Call after the user compares the vault address on both devices. This never signs.
    pub fn accept_vault(&self, config: &VaultConfig) -> Result<()> {
        config.validate_public_policy()?;
        let identity = self.public_identity()?;
        if config.network != identity.network || config.hww_vault_pubkey != identity.vault_pubkey {
            bail!("vault does not belong to this signer and network");
        }
        if self.paired() {
            if serde_json::to_vec(&self.configuration()?)? == serde_json::to_vec(config)? {
                return Ok(());
            }
            bail!("signer already paired; use an explicitly approved rotation to change vaults");
        }
        if config.monthly_limit_sats != 0 || config.emergency_access_limit_sats != 0 {
            bail!("initial pairing cannot approve a spending policy");
        }
        let hww = self.keys()?;
        let state = protocol::protocol(
            Signer::new(&mut SoftwareCrypto::new(), &mut SoftwareKeys(&hww)).pair(
                protocol::config_network(config)?,
                protocol::vault_keys(config)?.phone,
            ),
        )?;
        self.save(config, &state)
    }

    pub fn review_policy(&self, request: &PolicyRequest) -> Result<PolicyReview> {
        let (config, state) = self.paired_state()?;
        let hww = self.keys()?;
        let review = protocol::protocol(
            Signer::new(&mut SoftwareCrypto::new(), &mut SoftwareKeys(&hww)).review_policy(
                &state,
                request,
                Some(now()),
            ),
        )?;
        let pending = PendingPolicy {
            state,
            request: request.clone(),
        };
        let review_id = record_id(&pending)?;
        storage::write_json(&self.review_path(&review_id)?, &pending)?;
        let months = anzen_protocol::consts::MONTHS_PER_ROLLOVER;
        Ok(PolicyReview {
            review_id,
            network: config.network,
            vault_address: review.vault_address,
            vault_input_sats: review.vault_input_sats,
            monthly_limit_sats: review.monthly_limit_sats,
            funded_withdrawals: review.funded_withdrawals,
            funding_warning: (review.monthly_limit_sats > 0 && review.funded_withdrawals < months)
                .then(|| format!(
                    "Only {} of {months} monthly withdrawals can be funded. The policy funds the earliest withdrawals first.",
                    review.funded_withdrawals
                )),
            emergency_access_limit_sats: review.emergency_access_limit_sats,
            emergency_delay_seconds: review.emergency_delay_seconds,
            rollover_fee_sats: review.rollover_fee_sats,
            transaction_count: review.transaction_count,
            allowance_destinations: review.allowance_destinations,
            emergency_destination: review.emergency_destination,
            previous_policy_warning: linkage_warning(review.previous_policy),
            previous_policy: review.previous_policy,
        })
    }

    /// Approval refers to the stored, validated request, not a replacement object from the UI.
    pub fn approve_policy(&self, review_id: &str) -> Result<PolicyApproval> {
        let (mut config, state) = self.paired_state()?;
        let pending: PendingPolicy = storage::read_json(&self.review_path(review_id)?)
            .context("review this policy on the signer before approving it")?;
        if record_id(&pending)? != review_id {
            bail!("stored policy differs from the reviewed request");
        }
        if pending.state != state {
            bail!("vault changed since policy review; review the request again");
        }
        let hww = self.keys()?;
        let (approval, next) = protocol::protocol(
            Signer::new(&mut SoftwareCrypto::new(), &mut SoftwareKeys(&hww)).approve_policy(
                &state,
                &pending.request,
                Some(now()),
            ),
        )?;
        // Public signed evidence is retained independently of the phone. An offline signer
        // cannot tell whether the phone subsequently activated it; never call it active state.
        storage::write_json(&self.approved_policy_path(&config), &approval)?;
        config.monthly_limit_sats = next.limits.monthly_limit_sats;
        config.emergency_access_limit_sats = next.limits.emergency_access_limit_sats;
        self.save(&config, &next)?;
        Ok(approval)
    }

    pub fn reject_policy(&self, review_id: &str) -> Result<()> {
        self.configuration()?;
        self.remove_review(self.review_path(review_id)?)
    }

    /// Clear the previous-policy anchor. Only the signer's own user can do this, after they
    /// confirm the last approved policy was never activated.
    pub fn forget_approved_policy(&self) -> Result<()> {
        let (config, mut state) = self.paired_state()?;
        state.anchor = None;
        state.anchor_approved_at = None;
        self.save(&config, &state)
    }

    pub fn review_sweep(&self, request: &SweepRequest) -> Result<SweepReview> {
        let (config, state) = self.paired_state()?;
        let hww = self.keys()?;
        let review = protocol::protocol(
            Signer::new(&mut SoftwareCrypto::new(), &mut SoftwareKeys(&hww))
                .review_sweep(&state, request),
        )?;
        let pending = PendingSweep {
            state,
            request: request.clone(),
        };
        let review_id = record_id(&pending)?;
        storage::write_json(&self.sweep_review_path(&review_id)?, &pending)?;
        Ok(SweepReview {
            review_id,
            network: config.network,
            destination: review.destination,
            amount_sats: review.amount_sats,
            fee_sats: review.fee_sats,
            input_count: review.input_count,
        })
    }

    pub fn approve_sweep(&self, review_id: &str) -> Result<SweepApproval> {
        let (_, state) = self.paired_state()?;
        let pending: PendingSweep = storage::read_json(&self.sweep_review_path(review_id)?)
            .context("review this sweep before approving it")?;
        if record_id(&pending)? != review_id || pending.state != state {
            bail!("sweep or vault changed since review");
        }
        let hww = self.keys()?;
        protocol::protocol(
            Signer::new(&mut SoftwareCrypto::new(), &mut SoftwareKeys(&hww))
                .approve_sweep(&state, &pending.request),
        )
    }

    fn sweep_review_path(&self, review_id: &str) -> Result<PathBuf> {
        self.review_path(review_id)?; // Validate the opaque identifier before using it in a path.
        Ok(self
            .data_dir
            .join("hww/sweep-reviews")
            .join(format!("{review_id}.json")))
    }

    pub fn export_recovery_bundle(&self) -> Result<RecoveryVaultBundle> {
        let config = self.configuration()?;
        let path = self.approved_policy_path(&config);
        let last_approved_policy: Option<PolicyApproval> = if path.storage_exists() {
            Some(storage::read_json(&path)?)
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

    fn approved_policy_path(&self, config: &VaultConfig) -> PathBuf {
        self.data_dir.join("hww/approved-policies").join(format!(
            "{}.json",
            sha256::Hash::hash(config.vault_descriptor.as_bytes())
        ))
    }

    fn paired(&self) -> bool {
        self.data_dir.join(SIGNER_CONFIG_FILE).storage_exists()
    }

    /// The accepted public vault configuration, with limits from the last approved policy.
    fn configuration(&self) -> Result<VaultConfig> {
        Ok(self.paired_state()?.0)
    }

    fn paired_state(&self) -> Result<(VaultConfig, State)> {
        let config: VaultConfig = storage::read_json(&self.data_dir.join(SIGNER_CONFIG_FILE))
            .context("pair this signer with a vault first")?;
        config.validate_public_policy()?;
        let identity = self.public_identity()?;
        if config.network != identity.network || config.hww_vault_pubkey != identity.vault_pubkey {
            bail!("vault does not belong to this signer and network");
        }
        let state: State = storage::read_json(&self.data_dir.join(SIGNER_STATE_FILE))?;
        if state.keys != protocol::vault_keys(&config)?
            || state.network != protocol::config_network(&config)?
            || state.limits.monthly_limit_sats != config.monthly_limit_sats
            || state.limits.emergency_access_limit_sats != config.emergency_access_limit_sats
        {
            bail!("signer state does not match its accepted vault");
        }
        Ok((config, state))
    }

    fn save(&self, config: &VaultConfig, state: &State) -> Result<()> {
        storage::write_json(&self.data_dir.join(SIGNER_STATE_FILE), state)?;
        storage::write_json(&self.data_dir.join(SIGNER_CONFIG_FILE), config)
    }

    fn keys(&self) -> Result<DeviceKeys> {
        load_device_keys(&self.data_dir, HWW_DEVICE_FILE)
    }

    fn review_path(&self, review_id: &str) -> Result<PathBuf> {
        if review_id.len() != 64
            || !review_id
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
        {
            bail!("invalid policy review ID");
        }
        Ok(self
            .data_dir
            .join("hww/reviews")
            .join(format!("{review_id}.json")))
    }

    fn remove_review(&self, path: PathBuf) -> Result<()> {
        match anzen_common::storage::fs::remove_file(path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error.into()),
        }
    }

    fn check_access(&self, network: Network) -> Result<()> {
        storage::validate_supported_network(network)?;
        if network == Network::Bitcoin && !self.allow_mainnet {
            bail!("mainnet requires explicit dangerously-enable-mainnet opt-in");
        }
        Ok(())
    }
}

/// Stored review records are addressed by the hash of their exact contents.
fn record_id<T: Serialize>(record: &T) -> Result<String> {
    Ok(sha256::Hash::hash(&serde_json::to_vec(record)?).to_string())
}

/// This device's clock, used only to reject reports the protocol's delays make impossible.
fn now() -> u64 {
    u64::try_from(chrono::Utc::now().timestamp()).unwrap_or(0)
}
