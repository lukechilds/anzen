//! Delayed sweeping after social recovery. No signer is consulted.
use super::delayed_recovery::RecoveryKind;
use super::*;

impl WalletApplication {
    pub fn review_contact_sweep(
        &self,
        backend: &dyn HotWalletBackend,
        destination: &str,
    ) -> Result<ContactSweepReview> {
        let recovered = self.contact_recovery_state()?;
        self.prepare_delayed_recovery(
            backend,
            recovered.config,
            destination,
            RecoveryKind::Contact,
        )
    }

    pub fn confirm_contact_sweep(
        &self,
        backend: &dyn HotWalletBackend,
        review_id: &str,
    ) -> Result<Txid> {
        let record = self.delayed_recovery_record(RecoveryKind::Contact, review_id)?;
        let state = self.contact_recovery_state()?;
        if record.recipient_public_key != state.recipient_public_key
            || serde_json::to_vec(&record.config)? != serde_json::to_vec(&state.config)?
        {
            bail!("contact sweep or recovery source changed since review");
        }
        self.submit_delayed_recovery(backend, RecoveryKind::Contact, review_id, record, || {
            self.restored_keys(&state.config)
        })
    }

    pub fn reject_contact_sweep(&self, review_id: &str) -> Result<()> {
        self.reject_delayed_recovery(RecoveryKind::Contact, review_id)
    }
}
