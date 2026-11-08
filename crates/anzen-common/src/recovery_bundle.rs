//! Public recovery context. An offline signer can attest its last approved policy, not
//! whether that policy was broadcast or is still the live epoch.

use super::{
    protocol::{self, SoftwareCrypto},
    storage::VaultConfig,
};
use anyhow::{Context, Result, bail};
use anzen_protocol::{
    policy::Limits,
    wire::{self, PolicyApproval},
};
use serde::{Deserialize, Serialize};

pub const BUNDLE_VERSION: u8 = 2;

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RecoveryVaultBundle {
    pub version: u8,
    pub config: VaultConfig,
    /// The signer's approval of the last policy, including its signatures.
    pub last_approved_policy: Option<PolicyApproval>,
}

impl RecoveryVaultBundle {
    /// Check the configuration and that the recorded limits carry the HWW's own signatures.
    pub fn validate(&self) -> Result<()> {
        self.config.validate_public_policy()?;
        if self.version != BUNDLE_VERSION {
            bail!("unsupported recovery vault bundle");
        }
        match &self.last_approved_policy {
            Some(approval) => {
                if self.limits() != approval.request.limits {
                    bail!("recovery limits differ from the signed policy");
                }
                verify_signer_approval(&self.config, approval)?;
            }
            None if self.limits()
                != (Limits {
                    monthly_limit_sats: 0,
                    emergency_access_limit_sats: 0,
                }) =>
            {
                bail!("enabled recovery policy needs its signed approval record");
            }
            None => {}
        }
        Ok(())
    }

    pub fn limits(&self) -> Limits {
        Limits {
            monthly_limit_sats: self.config.monthly_limit_sats,
            emergency_access_limit_sats: self.config.emergency_access_limit_sats,
        }
    }
}

/// Rebuild an approved policy for `config` and verify every HWW partial signature in it.
pub fn verify_signer_approval(config: &VaultConfig, approval: &PolicyApproval) -> Result<()> {
    let request = &approval.request;
    if approval.version != wire::VERSION
        || request.network != protocol::config_network(config)?
        || request.keys != protocol::vault_keys(config)?
    {
        bail!("signed policy belongs to another vault");
    }
    let mut crypto = SoftwareCrypto::new();
    let (scripts, graph) = protocol::protocol(request.build(&mut crypto))?;
    let jobs = protocol::protocol(graph.cooperative_signing_jobs(&mut crypto))?;
    if jobs.len() != approval.signatures.len() || jobs.len() != request.phone_nonces.len() {
        bail!("signed policy does not cover every presigned transaction");
    }
    for ((job, signature), nonce) in jobs
        .iter()
        .zip(&approval.signatures)
        .zip(&request.phone_nonces)
    {
        super::musig::verify_signer_half(
            &request.keys,
            &scripts.vault.merkle_root,
            &job.sighash,
            nonce,
            signature,
        )
        .context("signed policy contains an invalid HWW signature")?;
    }
    Ok(())
}
