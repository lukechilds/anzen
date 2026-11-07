//! Recover the old phone's ordinary wallet separately from rotating its vault.
use super::*;
use anzen_common::storage::fs::StoragePath;

#[derive(Serialize, Deserialize)]
struct RecoveredPayment {
    source: VaultConfig,
    identity: PublicDeviceFile,
    psbt: String,
    destination: String,
    amount_sats: u64,
    fee_sats: u64,
}

impl WalletApplication {
    pub fn review_recovered_wallet(&self, backend: &dyn HotWalletBackend) -> Result<PaymentReview> {
        self.check_backend(backend)?;
        let source = self.recovery_source()?;
        let keys = self.restored_keys(&source)?;
        let mut old_wallet = HotWallet::ephemeral(&keys)?;
        backend.sync_hot_wallet(&mut old_wallet)?;
        let destination = self.receive_address()?;
        let address = Address::from_str(&destination)?.require_network(backend.network())?;
        let psbt = old_wallet
            .prepare_sweep(address.script_pubkey())?
            .context("the recovered hot wallet has no spendable funds; immature coins must wait")?;
        let amount_sats = psbt.unsigned_tx.output.iter().try_fold(0u64, |total, o| {
            if o.script_pubkey != address.script_pubkey() {
                bail!("recovered wallet transfer has an unexpected output");
            }
            total
                .checked_add(o.value.to_sat())
                .context("recovered amount overflow")
        })?;
        let pending = RecoveredPayment {
            source,
            identity: self.public_identity()?,
            fee_sats: psbt.fee()?.to_sat(),
            psbt: psbt.to_string(),
            destination,
            amount_sats,
        };
        let review_id = sha256::Hash::hash(&serde_json::to_vec(&pending)?).to_string();
        storage::write_json(
            &self.recovered_payment_path(&review_id, "review")?,
            &pending,
        )?;
        Ok(PaymentReview {
            review_id,
            network: pending.identity.network,
            destination: pending.destination,
            amount_sats,
            fee_sats: pending.fee_sats,
            total_sats: amount_sats
                .checked_add(pending.fee_sats)
                .context("recovered total overflow")?,
        })
    }

    pub fn confirm_recovered_wallet(
        &self,
        backend: &dyn HotWalletBackend,
        review_id: &str,
    ) -> Result<Txid> {
        self.check_backend(backend)?;
        let pending: RecoveredPayment =
            storage::read_json(&self.recovered_payment_path(review_id, "review")?)?;
        if sha256::Hash::hash(&serde_json::to_vec(&pending)?).to_string() != review_id
            || serde_json::to_vec(&self.public_identity()?)?
                != serde_json::to_vec(&pending.identity)?
        {
            bail!("recovered wallet transfer changed since review");
        }
        let psbt = Psbt::from_str(&pending.psbt)?;
        let expected = psbt.unsigned_tx.compute_txid();
        let receipt = self.recovered_payment_path(review_id, "broadcast")?;
        if receipt.storage_exists() {
            let previous: Txid = storage::read_json(&receipt)?;
            if previous != expected {
                bail!("recovered wallet receipt mismatch");
            }
            return Ok(previous);
        }
        let signed = self.recovered_payment_path(review_id, "signed")?;
        let transaction: bitcoin::Transaction = if signed.storage_exists() {
            storage::read_json(&signed)?
        } else {
            let keys = self.restored_keys(&pending.source)?;
            let transaction = HotWallet::ephemeral(&keys)?.sign_payment(psbt)?;
            // Retain the exact transaction before the network call. A lost response is not
            // evidence that submission failed; retries must not create a different payment.
            storage::write_json(&signed, &transaction)?;
            transaction
        };
        if transaction.compute_txid() != expected {
            bail!("signed recovered wallet transfer changed since review");
        }
        let accepted = backend.broadcast(&transaction)?;
        if accepted != expected {
            bail!("unexpected recovered wallet transaction ID");
        }
        storage::write_json(&receipt, &accepted)?;
        Ok(accepted)
    }

    pub fn reject_recovered_wallet(&self, review_id: &str) -> Result<()> {
        self.public_identity()?;
        if self
            .recovered_payment_path(review_id, "signed")?
            .storage_exists()
            || self
                .recovered_payment_path(review_id, "broadcast")?
                .storage_exists()
        {
            bail!("cannot cancel a recovered wallet transfer after submission has started");
        }
        match anzen_common::storage::fs::remove_file(
            self.recovered_payment_path(review_id, "review")?,
        ) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e.into()),
        }
    }

    fn recovered_payment_path(&self, id: &str, kind: &str) -> Result<PathBuf> {
        if id.len() != 64
            || !id
                .bytes()
                .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
        {
            bail!("invalid recovered wallet review ID");
        }
        Ok(self
            .data_dir
            .join("phone/recovered-wallet")
            .join(id)
            .join(format!("{kind}.json")))
    }
}
