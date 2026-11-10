//! The software hardware-wallet host.
//!
//! `anzen_signer` decides everything this device signs. This crate supplies what that crate
//! deliberately lacks: key storage, persistence of its state, review records, encrypted backup
//! envelopes and the outbox. It depends on no wallet, chain client or PSBT code; CI checks its
//! dependency tree.

use anzen_common::storage::fs::StoragePath;
pub mod application;

/// Offline consumer types; intentionally no chain clients or phone implementation.
pub mod api {
    pub use anzen_common::cloud_backup::CloudRecoveryBackup;
    pub use anzen_common::cloud_restore::RestoreKeyRequest;
    pub use anzen_common::device_channel::DeviceMessage;
    pub use anzen_common::device_rotation::{ApprovedDevicePhoneRotation, DevicePhoneRotation};
    pub use anzen_common::outbox::{OutboxItem, OutgoingMessage, OutgoingMessageKind};
    pub use anzen_common::recovery_bundle::RecoveryVaultBundle;
    pub use anzen_common::storage::with_protected_storage;
    pub use anzen_common::storage::{PublicDeviceFile, VaultConfig};
    pub use anzen_protocol::wire::{
        OfflineAction, OfflineActionApproval, OfflineActionRequest, PolicyApproval, PolicyRequest,
        SweepApproval, SweepRequest,
    };
    pub use bitcoin::Network;
}

use anyhow::Result;
use anzen_common::{
    keys::DeviceKeys,
    protocol,
    storage::{
        DeviceFile, HWW_DEVICE_FILE, HWW_PUBLIC_FILE, InitializedDevice, PublicDeviceFile,
        network_name, validate_supported_network, write_json,
    },
};
use bitcoin::{Network, key::Secp256k1, secp256k1::Message};
use std::path::Path;

/// Create this device's signing key. It never reads another device's files.
pub fn initialize(data_dir: &Path, network: Network) -> Result<InitializedDevice> {
    validate_supported_network(network)?;
    let hww_path = data_dir.join(HWW_DEVICE_FILE);
    if hww_path.storage_exists() {
        anyhow::bail!("HWW already initialized at {}", hww_path.display());
    }
    let hww = DeviceKeys::generate_for_network(&Secp256k1::new(), network)?;
    persist_signer(data_dir, network, &hww)
}

/// Replace this signer's key with a recovery phrase's. Its caller checks the signer is not yet
/// paired; the old key is set aside, never deleted.
pub fn import(data_dir: &Path, network: Network, words: &str) -> Result<InitializedDevice> {
    validate_supported_network(network)?;
    let hww = DeviceKeys::import_for_network(&Secp256k1::new(), words, network)?;
    anzen_common::storage::set_aside(data_dir, "hww")?;
    persist_signer(data_dir, network, &hww)
}

fn persist_signer(
    data_dir: &Path,
    network: Network,
    hww: &DeviceKeys,
) -> Result<InitializedDevice> {
    let hww_path = data_dir.join(HWW_DEVICE_FILE);
    let mnemonic = hww.mnemonic.to_string();
    write_json(
        &data_dir.join(HWW_PUBLIC_FILE),
        &PublicDeviceFile {
            version: 1,
            kind: "hww-public-key".to_owned(),
            network: network_name(network).to_owned(),
            vault_pubkey: hww.vault_pubkey.to_string(),
        },
    )?;
    write_json(
        &hww_path,
        &DeviceFile {
            kind: "hww".to_owned(),
            network: network_name(network).to_owned(),
            mnemonic: mnemonic.clone(),
            vault_key_index: hww.vault_key_index,
            // Only the phone chooses the recovery locktime.
            recovery_locktime: anzen_common::DEFAULT_RECOVERY_LOCKTIME,
        },
    )?;
    Ok(InitializedDevice {
        mnemonic,
        vault_pubkey: hww.vault_pubkey.to_string(),
        vault_key_index: hww.vault_key_index,
        recovery_locktime: anzen_common::DEFAULT_RECOVERY_LOCKTIME,
    })
}

/// The signer key held in software. Hardware wallets implement [`anzen_signer::Keys`] with
/// their secure element instead.
pub(crate) struct SoftwareKeys<'a>(pub &'a DeviceKeys);

impl anzen_signer::Keys for SoftwareKeys<'_> {
    fn vault_key(&mut self) -> anzen_protocol::Result<anzen_protocol::script::XOnlyKey> {
        Ok(protocol::key(self.0.vault_pubkey))
    }

    fn sign(&mut self, message: &[u8; 32]) -> anzen_protocol::Result<[u8; 64]> {
        Ok(Secp256k1::signing_only()
            .sign_schnorr_no_aux_rand(&Message::from_digest(*message), &self.0.vault_keypair)
            .serialize())
    }

    /// BIP327 `DeterministicSign` as the last signer: the nonce is derived from the request and
    /// dropped, so this device keeps no nonce state (see [`anzen_signer::Keys::sign_cooperative`]).
    fn sign_cooperative(
        &mut self,
        session: &anzen_signer::CooperativeSession<'_>,
    ) -> anzen_protocol::Result<anzen_protocol::wire::CooperativeSignature> {
        anzen_common::musig::sign_last(
            &self.0.vault_keypair.secret_bytes(),
            session.keys,
            session.merkle_root,
            session.message,
            session.phone_nonce,
        )
        .map_err(|_| anzen_protocol::Error::SigningFailed)
    }
}
