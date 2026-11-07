//! Independent rotation approval: public parameters plus this signer's key only.
use super::*;

#[derive(Serialize, Deserialize)]
struct RotationReviewRecord {
    state: State,
    request_id: String,
}

#[derive(Serialize, Deserialize)]
struct RotationApprovalRecord {
    old_config: VaultConfig,
    new_state: State,
    approval: ApprovedDevicePhoneRotation,
}

#[derive(Serialize)]
pub struct PhoneRotationReview {
    pub review_id: String,
    pub old_vault_address: String,
    pub new_vault_address: String,
    pub new_phone_public_key: String,
    pub amount_sats: u64,
    pub sweep_fee_sats: u64,
    pub monthly_limit_sats: u64,
    pub emergency_access_limit_sats: u64,
    pub funded_withdrawals: usize,
    pub revokes_connectors: bool,
    pub friend_fingerprints: Vec<String>,
    pub previous_policy: Linkage,
    pub previous_policy_warning: Option<String>,
}

impl SignerApplication {
    pub fn review_phone_rotation(
        &self,
        request: &DevicePhoneRotation,
    ) -> Result<PhoneRotationReview> {
        let (config, state) = self.paired_state()?;
        request.validate(&config)?;
        let hww = self.keys()?;
        let review = protocol::protocol(
            Signer::new(&mut SoftwareCrypto::new(), &mut SoftwareKeys(&hww)).review_rotation(
                &state,
                &request.rotation,
                Some(now()),
            ),
        )?;
        if review.new_vault_address != request.new_config.vault_address {
            bail!("rotation configuration does not match its new keys");
        }
        let friend_fingerprints = self
            .backup_key(&request.new_config, &request.backup_request)?
            .friend_fingerprints()?;
        let record = RotationReviewRecord {
            state,
            request_id: request.id()?,
        };
        let review_id = record_id(&record)?;
        storage::write_json(&self.rotation_path(&review_id, "reviews")?, &record)?;
        Ok(PhoneRotationReview {
            review_id,
            old_vault_address: review.old_vault_address,
            new_vault_address: review.new_vault_address,
            new_phone_public_key: review.new_phone_public_key,
            amount_sats: review.amount_sats,
            sweep_fee_sats: review.sweep_fee_sats,
            monthly_limit_sats: review.monthly_limit_sats,
            emergency_access_limit_sats: review.emergency_access_limit_sats,
            funded_withdrawals: review.funded_withdrawals,
            revokes_connectors: review.revokes_connectors,
            friend_fingerprints,
            previous_policy_warning: linkage_warning(review.previous_policy),
            previous_policy: review.previous_policy,
        })
    }

    pub fn approve_phone_rotation(
        &self,
        review_id: &str,
        request: &DevicePhoneRotation,
    ) -> Result<ApprovedDevicePhoneRotation> {
        let (config, state) = self.paired_state()?;
        let record: RotationReviewRecord =
            storage::read_json(&self.rotation_path(review_id, "reviews")?)?;
        if record_id(&record)? != review_id
            || record.request_id != request.id()?
            || record.state != state
        {
            bail!("rotation or last-approved policy changed since review");
        }
        request.validate(&config)?;
        let hww = self.keys()?;
        let (approval, new_state) = protocol::protocol(
            Signer::new(&mut SoftwareCrypto::new(), &mut SoftwareKeys(&hww)).approve_rotation(
                &state,
                &request.rotation,
                Some(now()),
            ),
        )?;
        let backup_response =
            self.wrap_backup_for_config(&request.new_config, &request.backup_request)?;
        let approved = ApprovedDevicePhoneRotation {
            request_id: request.id()?,
            new_config: request.new_config.clone(),
            approval,
            backup_response,
        };
        storage::write_json(
            &self.rotation_path(&approved.request_id, "approvals")?,
            &RotationApprovalRecord {
                old_config: config,
                new_state,
                approval: approved.clone(),
            },
        )?;
        Ok(approved)
    }

    pub fn reject_phone_rotation(&self, review_id: &str) -> Result<()> {
        self.configuration()?;
        self.remove_review(self.rotation_path(review_id, "reviews")?)
    }

    /// User-confirmed configuration adoption, not a claim of independently verified chain state.
    pub fn adopt_phone_rotation(&self, request_id: &str) -> Result<VaultConfig> {
        let current = self.configuration()?;
        let record: RotationApprovalRecord =
            storage::read_json(&self.rotation_path(request_id, "approvals")?)?;
        if record.approval.request_id != request_id {
            bail!("stored rotation approval ID mismatch");
        }
        let new = &record.approval.new_config;
        if current.vault_descriptor == new.vault_descriptor {
            // A replay after renewal must not overwrite a newer policy approval for this vault.
            return Ok(current);
        }
        if current.vault_descriptor != record.old_config.vault_descriptor {
            bail!("rotation is not from the currently paired vault");
        }
        new.validate_public_policy()?;
        if new.hww_vault_pubkey != self.public_identity()?.vault_pubkey {
            bail!("rotation belongs to another signer");
        }
        storage::write_json(
            &self.rotation_path(request_id, "old-vaults")?,
            &record.old_config,
        )?;
        if let Some(policy) = &record.approval.approval.request.renewed_policy {
            storage::write_json(
                &self.approved_policy_path(new),
                &PolicyApproval {
                    version: anzen_protocol::wire::VERSION,
                    request: policy.clone(),
                    signatures: record.approval.approval.policy_signatures.clone(),
                },
            )?;
        }
        self.save(new, &record.new_state)?;
        Ok(new.clone())
    }

    fn rotation_path(&self, id: &str, category: &str) -> Result<PathBuf> {
        self.review_path(id)?;
        Ok(self
            .data_dir
            .join("hww/rotations")
            .join(category)
            .join(format!("{id}.json")))
    }
}
