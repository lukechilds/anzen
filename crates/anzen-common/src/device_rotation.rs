//! Phone-key rotation between devices. The signer receives parameters and public keys only:
//! no phone seed, no ciphertext and no transactions. The replacement phone's cloud payload
//! stays on the phone; the signer sees a wrapped transport request for its backup key.

use super::{
    device_channel::DeviceMessage,
    protocol::{self, SoftwareCrypto},
    storage::VaultConfig,
};
use anyhow::{Result, ensure};
use anzen_protocol::{
    policy::Limits,
    wire::{self, RotationApproval, RotationRequest},
};
use bitcoin::hashes::{Hash, sha256};
use serde::{Deserialize, Serialize};

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DevicePhoneRotation {
    pub new_config: VaultConfig,
    pub rotation: RotationRequest,
    pub backup_request: DeviceMessage,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ApprovedDevicePhoneRotation {
    pub request_id: String,
    pub new_config: VaultConfig,
    pub approval: RotationApproval,
    pub backup_response: DeviceMessage,
}

impl DevicePhoneRotation {
    pub fn id(&self) -> Result<String> {
        Ok(sha256::Hash::hash(&serde_json::to_vec(self)?).to_string())
    }

    /// Public checks both devices make: the new configuration is exactly the rotation's new keys
    /// and the enabled policy limits carry over unchanged.
    pub fn validate(&self, old: &VaultConfig) -> Result<()> {
        old.validate_public_policy()?;
        self.new_config.validate_public_policy()?;
        let new = &self.new_config;
        let rotation = &self.rotation;
        ensure!(
            new.network == old.network
                && new.hww_vault_pubkey == old.hww_vault_pubkey
                && new.phone_vault_pubkey != old.phone_vault_pubkey,
            "rotation must change only the phone key on the same network"
        );
        ensure!(
            new.monthly_limit_sats == old.monthly_limit_sats
                && new.emergency_access_limit_sats == old.emergency_access_limit_sats,
            "rotation must preserve the reviewed policy limits"
        );
        ensure!(
            rotation.network == protocol::config_network(old)?
                && rotation.old_keys == protocol::vault_keys(old)?
                && rotation.new_keys() == protocol::vault_keys(new)?,
            "rotation request differs from its public vault configuration"
        );
        ensure!(
            wire::same_limits(rotation.renewed_policy.as_ref(), limits(old)),
            "rotation must renew exactly the enabled policy"
        );
        protocol::protocol(rotation.build(&mut SoftwareCrypto::new()))?;
        Ok(())
    }
}

/// The policy limits recorded in a vault configuration.
pub fn limits(config: &VaultConfig) -> Limits {
    Limits {
        monthly_limit_sats: config.monthly_limit_sats,
        emergency_access_limit_sats: config.emergency_access_limit_sats,
    }
}
