//! Replacing an unavailable HWW does not give the new HWW authority over old funds.
//! The surviving phone signs a fixed CSV recovery transfer; the new HWW signs only the
//! renewed vault policy and wraps a new backup key. Neither operation bypasses Bitcoin.

use super::{
    device_channel::DeviceMessage,
    device_rotation::limits,
    protocol::{self, SoftwareCrypto},
    recovery_bundle::RecoveryVaultBundle,
    storage::VaultConfig,
};
use anyhow::{Result, ensure};
use anzen_protocol::{
    policy::Limits,
    wire::{self, PolicyApproval, ReplacementApproval, ReplacementRequest},
};
use bitcoin::hashes::{Hash, sha256};
use serde::{Deserialize, Serialize};

pub const REPLACEMENT_VERSION: u8 = 2;

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeviceSignerReplacement {
    pub version: u8,
    /// Old vault configuration and the old signer's signed limits.
    pub source: RecoveryVaultBundle,
    pub new_config: VaultConfig,
    pub replacement: ReplacementRequest,
    pub backup_request: DeviceMessage,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ApprovedSignerReplacement {
    pub request_id: String,
    pub approval: ReplacementApproval,
    pub backup_response: DeviceMessage,
}

impl DeviceSignerReplacement {
    pub fn id(&self) -> Result<String> {
        Ok(sha256::Hash::hash(&serde_json::to_vec(self)?).to_string())
    }

    /// Checks both the phone and the replacement signer make. Returns the preserved limits,
    /// authenticated by the old signer's own policy signatures.
    pub fn validate(&self) -> Result<Limits> {
        ensure!(
            self.version == REPLACEMENT_VERSION,
            "unsupported signer replacement"
        );
        self.source.validate()?;
        self.new_config.validate_public_policy()?;
        let old = &self.source.config;
        let new = &self.new_config;
        ensure!(
            new.network == old.network
                && new.phone_vault_pubkey == old.phone_vault_pubkey
                && new.hww_vault_pubkey != old.hww_vault_pubkey
                && new.phone_hot_external_descriptor == old.phone_hot_external_descriptor
                && new.phone_hot_internal_descriptor == old.phone_hot_internal_descriptor,
            "signer replacement must preserve the phone and replace only the HWW key"
        );
        ensure!(
            limits(new) == limits(old),
            "signer replacement must preserve the reviewed policy limits"
        );
        let request = &self.replacement;
        ensure!(
            request.network == protocol::config_network(old)?
                && request.old_keys == protocol::vault_keys(old)?
                && request.new_keys() == protocol::vault_keys(new)?,
            "signer replacement request differs from its public vault configuration"
        );
        ensure!(
            wire::same_limits(request.renewed_policy.as_ref(), limits(old)),
            "replacement must renew exactly the enabled policy"
        );
        protocol::protocol(request.build(&mut SoftwareCrypto::new()))?;
        Ok(limits(old))
    }

    /// The renewed policy the replacement signer approved, ready for normal policy activation.
    pub fn renewed_policy(
        &self,
        approval: &ApprovedSignerReplacement,
    ) -> Result<Option<PolicyApproval>> {
        self.validate()?;
        ensure!(
            approval.request_id == self.id()?
                && approval.approval.version == wire::VERSION
                && approval.approval.request == self.replacement,
            "signer replacement approval belongs to another request"
        );
        Ok(match &self.replacement.renewed_policy {
            Some(request) => Some(PolicyApproval {
                version: wire::VERSION,
                request: request.clone(),
                signatures: approval.approval.policy_signatures.clone(),
            }),
            None => {
                ensure!(
                    approval.approval.policy_signatures.is_empty(),
                    "signer changed whether the replacement policy is enabled"
                );
                None
            }
        })
    }
}
