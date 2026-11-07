//! A fresh signer approves only its new policy and backup. It never signs an old-vault input.
use super::*;

#[derive(Serialize, Deserialize)]
struct ReviewRecord {
    request_id: String,
    new_config: VaultConfig,
}

#[derive(Serialize, Deserialize)]
struct ApprovalRecord {
    new_config: VaultConfig,
    state: State,
    approval: ApprovedSignerReplacement,
}

#[derive(Serialize)]
pub struct SignerReplacementReview {
    pub review_id: String,
    pub old_vault_address: String,
    pub new_vault_address: String,
    pub phone_public_key: String,
    pub signer_public_key: String,
    pub recovery_delay_blocks: u16,
    pub input_count: usize,
    pub amount_sats: u64,
    pub recovery_fee_sats: u64,
    pub monthly_limit_sats: u64,
    pub emergency_access_limit_sats: u64,
    pub funded_withdrawals: usize,
    pub friend_fingerprints: Vec<String>,
}

impl SignerApplication {
    pub fn review_signer_replacement(
        &self,
        request: &DeviceSignerReplacement,
    ) -> Result<SignerReplacementReview> {
        self.require_replacement_signer(&request.new_config)?;
        let limits = request.validate()?;
        let hww = self.keys()?;
        let review = protocol::protocol(
            Signer::new(&mut SoftwareCrypto::new(), &mut SoftwareKeys(&hww))
                .review_replacement(&request.replacement, limits),
        )?;
        let friend_fingerprints = self
            .backup_key(&request.new_config, &request.backup_request)?
            .friend_fingerprints()?;
        let record = ReviewRecord {
            request_id: request.id()?,
            new_config: request.new_config.clone(),
        };
        let review_id = record_id(&record)?;
        storage::write_json(&self.replacement_path(&review_id, "reviews")?, &record)?;
        Ok(SignerReplacementReview {
            review_id,
            old_vault_address: review.old_vault_address,
            new_vault_address: review.new_vault_address,
            phone_public_key: request.new_config.phone_vault_pubkey.clone(),
            signer_public_key: request.new_config.hww_vault_pubkey.clone(),
            recovery_delay_blocks: anzen_common::PHONE_RECOVERY_BLOCKS,
            input_count: request.replacement.recovery_inputs.len(),
            amount_sats: review.amount_sats,
            recovery_fee_sats: review.recovery_fee_sats,
            monthly_limit_sats: review.monthly_limit_sats,
            emergency_access_limit_sats: review.emergency_access_limit_sats,
            funded_withdrawals: review.funded_withdrawals,
            friend_fingerprints,
        })
    }

    pub fn approve_signer_replacement(
        &self,
        review_id: &str,
        request: &DeviceSignerReplacement,
    ) -> Result<ApprovedSignerReplacement> {
        self.require_replacement_signer(&request.new_config)?;
        let record: ReviewRecord =
            storage::read_json(&self.replacement_path(review_id, "reviews")?)?;
        if record_id(&record)? != review_id
            || record.request_id != request.id()?
            || serde_json::to_vec(&record.new_config)? != serde_json::to_vec(&request.new_config)?
        {
            bail!("signer replacement differs from the reviewed request");
        }
        let limits = request.validate()?;
        let hww = self.keys()?;
        let (approval, state) =
            protocol::protocol(
                Signer::new(&mut SoftwareCrypto::new(), &mut SoftwareKeys(&hww))
                    .approve_replacement(&request.replacement, limits, Some(now())),
            )?;
        let backup_response =
            self.wrap_backup_for_config(&request.new_config, &request.backup_request)?;
        let approved = ApprovedSignerReplacement {
            request_id: request.id()?,
            approval,
            backup_response,
        };
        storage::write_json(
            &self.replacement_path(&approved.request_id, "approvals")?,
            &ApprovalRecord {
                new_config: request.new_config.clone(),
                state,
                approval: approved.clone(),
            },
        )?;
        Ok(approved)
    }

    pub fn reject_signer_replacement(&self, review_id: &str) -> Result<()> {
        self.public_identity()?;
        self.remove_review(self.replacement_path(review_id, "reviews")?)
    }

    pub fn adopt_signer_replacement(&self, request_id: &str) -> Result<VaultConfig> {
        let record: ApprovalRecord =
            storage::read_json(&self.replacement_path(request_id, "approvals")?)?;
        if record.approval.request_id != request_id {
            bail!("replacement approval ID mismatch");
        }
        let config = record.new_config;
        config.validate_public_policy()?;
        if self.paired() {
            if self.configuration()?.vault_descriptor != config.vault_descriptor {
                bail!("signer already paired to another vault");
            }
            return self.configuration();
        }
        self.require_replacement_signer(&config)?;
        if let Some(policy) = &record.approval.approval.request.renewed_policy {
            storage::write_json(
                &self.approved_policy_path(&config),
                &PolicyApproval {
                    version: anzen_protocol::wire::VERSION,
                    request: policy.clone(),
                    signatures: record.approval.approval.policy_signatures.clone(),
                },
            )?;
        }
        self.save(&config, &record.state)?;
        Ok(config)
    }

    fn require_replacement_signer(&self, config: &VaultConfig) -> Result<()> {
        let identity = self.public_identity()?;
        if identity.vault_pubkey != config.hww_vault_pubkey || identity.network != config.network {
            bail!("replacement vault does not belong to this signer and network");
        }
        if self.paired() {
            bail!("replacement approval requires a fresh, unpaired signer");
        }
        Ok(())
    }

    fn replacement_path(&self, id: &str, category: &str) -> Result<PathBuf> {
        self.review_path(id)?;
        Ok(self
            .data_dir
            .join("hww/signer-replacements")
            .join(category)
            .join(format!("{id}.json")))
    }
}
