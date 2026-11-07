use super::*;

#[derive(Serialize, Deserialize)]
struct PendingAction {
    state: State,
    request: OfflineActionRequest,
}

#[derive(Serialize)]
pub struct OfflineActionReview {
    pub review_id: String,
    pub network: String,
    pub action: OfflineAction,
    pub vault_address: String,
    pub destination: String,
    pub amount_sats: u64,
    pub fee_sats: u64,
    pub input_count: usize,
    /// Required by script, not a claim that the offline signer knows the current chain height.
    pub recovery_delay_blocks: Option<u16>,
}

impl SignerApplication {
    pub fn review_offline_action(
        &self,
        request: &OfflineActionRequest,
    ) -> Result<OfflineActionReview> {
        let (config, state) = self.paired_state()?;
        let hww = self.keys()?;
        let review = protocol::protocol(
            Signer::new(&mut SoftwareCrypto::new(), &mut SoftwareKeys(&hww))
                .review_offline_action(&state, request),
        )?;
        let pending = PendingAction {
            state,
            request: request.clone(),
        };
        let review_id = record_id(&pending)?;
        storage::write_json(&self.action_review_path(&review_id)?, &pending)?;
        Ok(OfflineActionReview {
            review_id,
            network: config.network,
            action: review.action,
            vault_address: review.vault_address,
            destination: review.destination,
            amount_sats: review.amount_sats,
            fee_sats: review.fee_sats,
            input_count: review.input_count,
            recovery_delay_blocks: review.recovery_delay_blocks,
        })
    }

    pub fn approve_offline_action(&self, review_id: &str) -> Result<OfflineActionApproval> {
        let (_, state) = self.paired_state()?;
        let pending: PendingAction = storage::read_json(&self.action_review_path(review_id)?)?;
        if record_id(&pending)? != review_id || pending.state != state {
            bail!("offline action or vault changed since review");
        }
        let hww = self.keys()?;
        protocol::protocol(
            Signer::new(&mut SoftwareCrypto::new(), &mut SoftwareKeys(&hww))
                .approve_offline_action(&state, &pending.request),
        )
    }

    pub fn reject_offline_action(&self, review_id: &str) -> Result<()> {
        self.configuration()?;
        self.remove_review(self.action_review_path(review_id)?)
    }

    fn action_review_path(&self, review_id: &str) -> Result<PathBuf> {
        self.review_path(review_id)?;
        Ok(self
            .data_dir
            .join("hww/action-reviews")
            .join(format!("{review_id}.json")))
    }
}
