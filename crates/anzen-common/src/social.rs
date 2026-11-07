//! Versioned cloud-recovery envelopes and OpenPGP friend-key wrapping.
//!
//! The sensitive phone mnemonic and public vault descriptor are encrypted once with a random
//! symmetric key. The HWW and every configured recovery friend receive independent encrypted
//! copies of that same key. Friends are therefore 1-of-N recovery contacts; this is deliberately
//! not a threshold-sharing scheme.

use super::{
    crypto::{self, EncryptedBlob},
    keys::DeviceKeys,
    storage::VaultConfig,
};
use anyhow::{Context, Result, bail};
use base64::{Engine, engine::general_purpose::STANDARD};
use pgp::{
    composed::{
        ArmorOptions, Deserializable, KeyType, Message, MessageBuilder, SecretKeyParamsBuilder,
        SignedPublicKey, SignedSecretKey, SubkeyParamsBuilder,
    },
    crypto::{ecc_curve::ECCCurve, sym::SymmetricKeyAlgorithm},
    types::{KeyDetails, Password, PublicKeyTrait},
};
use rand::thread_rng;
use serde::{Deserialize, Serialize};

const PAYLOAD_PURPOSE: &str = "cloud/vault-recovery-payload/v1";
const HWW_KEY_PURPOSE: &str = "cloud/vault-recovery-key/hww/v1";
const FRIEND_MANIFEST_PURPOSE: &str = "cloud/vault-recovery-friends/v1";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, zeroize::Zeroize)]
pub struct RecoveryPayload {
    pub version: u8,
    pub kind: String,
    pub network: String,
    pub phone_mnemonic: String,
    #[serde(default)]
    pub phone_vault_key_index: u32,
    pub phone_vault_pubkey: String,
    pub vault_descriptor: String,
    pub vault_address: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FriendKeyWrapper {
    pub fingerprint: String,
    pub public_key_armored: String,
    pub encrypted_symmetric_key: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CloudRecoveryBackup {
    pub version: u8,
    pub kind: String,
    pub encrypted_payload: EncryptedBlob,
    pub hww_encrypted_symmetric_key: EncryptedBlob,
    pub friends: Vec<FriendKeyWrapper>,
    pub encrypted_friend_manifest: EncryptedBlob,
}

/// Only the random wrapping key crosses the device channel. The seed ciphertext stays on the
/// phone/cloud side: a stolen signer plus its protocol transcript must not reveal the phone seed.
#[derive(Clone, Serialize, Deserialize, zeroize::Zeroize, zeroize::ZeroizeOnDrop)]
#[serde(deny_unknown_fields)]
pub struct DeviceBackupKeyRequest {
    pub backup_id: String,
    pub symmetric_key: [u8; 32],
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub friend_public_keys: Vec<String>,
}

impl DeviceBackupKeyRequest {
    pub fn friend_fingerprints(&self) -> Result<Vec<String>> {
        recovery_contact_fingerprints(&self.friend_public_keys)
    }
}

/// Shared validation for device backup proposals and locally authenticated recovery metadata.
pub fn recovery_contact_fingerprints(public_keys: &[String]) -> Result<Vec<String>> {
    if public_keys.len() > 32 {
        bail!("a device backup supports at most 32 recovery contacts");
    }
    let mut fingerprints = Vec::new();
    for key in public_keys {
        if key.len() > 64 * 1024 {
            bail!("recovery contact public key exceeds 64 KiB");
        }
        fingerprints.push(friend_fingerprint(key.as_bytes())?);
    }
    fingerprints.sort();
    if fingerprints.windows(2).any(|pair| pair[0] == pair[1]) {
        bail!("duplicate recovery contact in backup request");
    }
    Ok(fingerprints)
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeviceBackupKeyResponse {
    pub backup_id: String,
    pub wrapped_key: EncryptedBlob,
}

#[derive(Serialize, Deserialize)]
pub struct CloudBackupDraft {
    pub encrypted_payload: EncryptedBlob,
    pub friends: Vec<FriendKeyWrapper>,
    pub encrypted_friend_manifest: EncryptedBlob,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GeneratedFriendKey {
    pub fingerprint: String,
    pub public_key_armored: String,
    pub private_key_armored: String,
}

impl RecoveryPayload {
    /// Reconstruct only the on-chain recovery policy from a self-contained backup. Spending
    /// limits are not encoded by this descriptor: callers must report them as unavailable.
    pub fn recovery_config(&self) -> Result<VaultConfig> {
        use super::{HWW_RECOVERY_BLOCKS, PHONE_RECOVERY_BLOCKS, policy::VaultPolicy, storage};
        use bitcoin::{key::Secp256k1, secp256k1::XOnlyPublicKey};
        use miniscript::ForEachKey;
        let network = storage::parse_network_name(&self.network)?;
        storage::validate_supported_network(network)?;
        let secp = Secp256k1::new();
        let phone = DeviceKeys::parse_for_network_at_index(
            &secp,
            &self.phone_mnemonic,
            network,
            self.phone_vault_key_index,
        )?;
        let policy = VaultPolicy::from_descriptor_for_network(&self.vault_descriptor, network)?;
        let mut keys = std::collections::BTreeSet::new();
        policy.descriptor.for_each_key(|key| {
            keys.insert(key.to_string());
            true
        });
        let (external, internal) = phone.hot_descriptors(&secp)?;
        for key in keys {
            let Ok(hww) = key.parse::<XOnlyPublicKey>() else {
                continue;
            };
            let config = VaultConfig {
                version: 1,
                network: self.network.clone(),
                phone_vault_pubkey: phone.vault_pubkey.to_string(),
                hww_vault_pubkey: hww.to_string(),
                phone_hot_external_descriptor: external.clone(),
                phone_hot_internal_descriptor: internal.clone(),
                vault_descriptor: self.vault_descriptor.clone(),
                vault_address: self.vault_address.clone(),
                phone_recovery_blocks: PHONE_RECOVERY_BLOCKS,
                hww_recovery_blocks: HWW_RECOVERY_BLOCKS,
                // Unknown, not evidence that the old spending policy was disabled.
                monthly_limit_sats: 0,
                emergency_access_limit_sats: 0,
            };
            if config.validate_public_policy().is_ok() {
                self.validate_against(&config)?;
                return Ok(config);
            }
        }
        bail!("backup descriptor is not the fixed Anzen policy for its phone key")
    }

    pub fn new(config: &VaultConfig, phone: &DeviceKeys) -> Result<Self> {
        if phone.vault_pubkey.to_string() != config.phone_vault_pubkey {
            bail!("phone key does not match the configured vault policy");
        }
        Ok(Self {
            version: 1,
            kind: "vault-recovery-payload".to_owned(),
            network: config.network.clone(),
            phone_mnemonic: phone.mnemonic.to_string(),
            phone_vault_key_index: phone.vault_key_index,
            phone_vault_pubkey: phone.vault_pubkey.to_string(),
            vault_descriptor: config.vault_descriptor.clone(),
            vault_address: config.vault_address.clone(),
        })
    }

    pub fn validate_against(&self, config: &VaultConfig) -> Result<()> {
        if self.version != 1
            || self.kind != "vault-recovery-payload"
            || self.network != config.network
            || self.phone_vault_pubkey != config.phone_vault_pubkey
            || self.vault_descriptor != config.vault_descriptor
            || self.vault_address != config.vault_address
        {
            bail!("recovery payload does not match the configured vault");
        }
        let phone = DeviceKeys::parse_for_network_at_index(
            &bitcoin::key::Secp256k1::new(),
            &self.phone_mnemonic,
            config.bitcoin_network()?,
            self.phone_vault_key_index,
        )?;
        if phone.vault_pubkey.to_string() != self.phone_vault_pubkey {
            bail!("recovery payload mnemonic does not match its phone public key");
        }
        Ok(())
    }
}

pub fn create_backup(
    payload: &RecoveryPayload,
    hww_seed: &[u8],
    friend_public_keys: &[String],
) -> Result<CloudRecoveryBackup> {
    let (draft, key) = prepare_backup(payload, friend_public_keys)?;
    complete_backup(draft, wrap_backup_key(hww_seed, &key)?)
}

pub fn prepare_backup(
    payload: &RecoveryPayload,
    friend_public_keys: &[String],
) -> Result<(CloudBackupDraft, zeroize::Zeroizing<[u8; 32]>)> {
    let symmetric_key = crypto::random_key();
    let payload_bytes = zeroize::Zeroizing::new(serde_json::to_vec(payload)?);
    let encrypted_payload = crypto::encrypt(&*symmetric_key, PAYLOAD_PURPOSE, &payload_bytes)?;
    let mut friends = Vec::with_capacity(friend_public_keys.len());
    for public_key in friend_public_keys {
        friends.push(wrap_for_friend(
            public_key.as_bytes(),
            symmetric_key.as_ref(),
        )?);
    }
    friends.sort_by(|left, right| left.fingerprint.cmp(&right.fingerprint));
    friends.dedup_by(|left, right| left.fingerprint == right.fingerprint);
    let encrypted_friend_manifest = encrypt_friend_manifest(symmetric_key.as_ref(), &friends)?;
    Ok((
        CloudBackupDraft {
            encrypted_payload,
            friends,
            encrypted_friend_manifest,
        },
        symmetric_key,
    ))
}

pub fn wrap_backup_key(hww_seed: &[u8], key: &[u8; 32]) -> Result<EncryptedBlob> {
    let wrapped = crypto::encrypt(hww_seed, HWW_KEY_PURPOSE, key)?;
    if crypto::decrypt(hww_seed, HWW_KEY_PURPOSE, &wrapped)?.as_slice() != key {
        bail!("backup key wrapping did not round-trip");
    }
    Ok(wrapped)
}

pub fn complete_backup(
    draft: CloudBackupDraft,
    hww_encrypted_symmetric_key: EncryptedBlob,
) -> Result<CloudRecoveryBackup> {
    if hww_encrypted_symmetric_key.version != 1
        || hww_encrypted_symmetric_key.purpose != HWW_KEY_PURPOSE
        || hww_encrypted_symmetric_key.ciphertext.len() != 48
    {
        bail!("invalid wrapped backup key");
    }
    Ok(CloudRecoveryBackup {
        version: 1,
        kind: "vault-cloud-recovery".to_owned(),
        encrypted_payload: draft.encrypted_payload,
        hww_encrypted_symmetric_key,
        friends: draft.friends,
        encrypted_friend_manifest: draft.encrypted_friend_manifest,
    })
}

pub fn add_friend(
    backup: &mut CloudRecoveryBackup,
    hww_seed: &[u8],
    public_key: &[u8],
) -> Result<String> {
    validate_backup(backup)?;
    let symmetric_key = crypto::decrypt(
        hww_seed,
        HWW_KEY_PURPOSE,
        &backup.hww_encrypted_symmetric_key,
    )?;
    if symmetric_key.len() != 32 {
        bail!("cloud backup contains an invalid symmetric key");
    }
    validate_friend_manifest(backup, &symmetric_key)?;
    let wrapper = wrap_for_friend(public_key, &symmetric_key)?;
    if backup
        .friends
        .iter()
        .any(|friend| friend.fingerprint == wrapper.fingerprint)
    {
        bail!(
            "recovery friend {} is already configured",
            wrapper.fingerprint
        );
    }
    let fingerprint = wrapper.fingerprint.clone();
    backup.friends.push(wrapper);
    backup
        .friends
        .sort_by(|left, right| left.fingerprint.cmp(&right.fingerprint));
    backup.encrypted_friend_manifest = encrypt_friend_manifest(&symmetric_key, &backup.friends)?;
    Ok(fingerprint)
}

pub fn decrypt_with_hww(backup: &CloudRecoveryBackup, hww_seed: &[u8]) -> Result<RecoveryPayload> {
    let symmetric_key = unwrap_backup_key(hww_seed, &backup.hww_encrypted_symmetric_key)?;
    decrypt_with_key(backup, &symmetric_key)
}

/// Used by an offline signer: the encrypted phone payload is deliberately not an argument.
pub fn unwrap_backup_key(
    hww_seed: &[u8],
    wrapped: &EncryptedBlob,
) -> Result<zeroize::Zeroizing<[u8; 32]>> {
    let plaintext = crypto::decrypt(hww_seed, HWW_KEY_PURPOSE, wrapped)?;
    let key: [u8; 32] = plaintext
        .as_slice()
        .try_into()
        .context("backup wrapper contains an invalid symmetric key")?;
    Ok(zeroize::Zeroizing::new(key))
}

/// Authenticate the complete cloud envelope before returning the descriptor-bound payload.
pub fn decrypt_with_key(backup: &CloudRecoveryBackup, key: &[u8; 32]) -> Result<RecoveryPayload> {
    validate_backup(backup)?;
    validate_friend_manifest(backup, key)?;
    decrypt_payload(backup, key)
}

pub fn decrypt_with_friend(
    backup: &CloudRecoveryBackup,
    private_key: &[u8],
) -> Result<RecoveryPayload> {
    validate_backup(backup)?;
    let key = unwrap_friend_key(backup, private_key)?;
    decrypt_with_key(backup, &key)
}

pub fn unwrap_friend_key(
    backup: &CloudRecoveryBackup,
    private_key: &[u8],
) -> Result<zeroize::Zeroizing<[u8; 32]>> {
    let (secret, _) = SignedSecretKey::from_reader_single(private_key)
        .context("failed to parse recovery friend's OpenPGP private key")?;
    let fingerprint = secret.fingerprint().to_string();
    let wrapper = backup
        .friends
        .iter()
        .find(|friend| friend.fingerprint == fingerprint)
        .with_context(|| format!("cloud backup has no wrapper for friend {fingerprint}"))?;
    unwrap_friend_wrapper(wrapper, private_key)
}

/// Contact-side recovery receives only this wrapper, never the encrypted phone payload.
pub fn unwrap_friend_wrapper(
    wrapper: &FriendKeyWrapper,
    private_key: &[u8],
) -> Result<zeroize::Zeroizing<[u8; 32]>> {
    let (secret, _) = SignedSecretKey::from_reader_single(private_key)
        .context("failed to parse recovery friend's OpenPGP private key")?;
    secret
        .verify()
        .context("recovery friend's OpenPGP private key failed self-signature validation")?;
    let fingerprint = secret.fingerprint().to_string();
    if wrapper.fingerprint != fingerprint
        || friend_fingerprint(wrapper.public_key_armored.as_bytes())? != fingerprint
    {
        bail!("friend wrapper belongs to a different OpenPGP key");
    }
    let encrypted_key = STANDARD
        .decode(&wrapper.encrypted_symmetric_key)
        .context("friend key wrapper is not valid base64")?;
    let mut message = Message::from_bytes(std::io::Cursor::new(encrypted_key))
        .context("friend key wrapper is not a valid OpenPGP message")?
        .decrypt(&Password::empty(), &secret)
        .context("friend OpenPGP key could not decrypt the recovery key")?;
    if message.is_compressed() {
        message = message
            .decompress()
            .context("failed to decompress friend recovery key message")?;
    }
    let symmetric_key = zeroize::Zeroizing::new(
        message
            .as_data_vec()
            .context("friend recovery key message contained no data")?,
    );
    Ok(zeroize::Zeroizing::new(
        symmetric_key
            .as_slice()
            .try_into()
            .context("friend recovery wrapper contains an invalid symmetric key")?,
    ))
}

pub fn friend_public_keys(backup: &CloudRecoveryBackup) -> Vec<String> {
    backup
        .friends
        .iter()
        .map(|friend| friend.public_key_armored.clone())
        .collect()
}

pub fn friend_fingerprint(public_key: &[u8]) -> Result<String> {
    let (public, _) = SignedPublicKey::from_reader_single(public_key)
        .context("failed to parse recovery friend's OpenPGP public key")?;
    public
        .verify()
        .context("recovery friend's OpenPGP public key failed self-signature validation")?;
    if !public
        .public_subkeys
        .iter()
        .any(|subkey| subkey.is_encryption_key())
    {
        bail!("recovery friend's OpenPGP key has no encryption-capable subkey");
    }
    Ok(public.fingerprint().to_string())
}

pub fn generate_friend_key(name: &str) -> Result<GeneratedFriendKey> {
    if name.trim().is_empty() {
        bail!("recovery friend name must not be empty");
    }
    let mut encryption_subkey = SubkeyParamsBuilder::default();
    encryption_subkey
        .key_type(KeyType::ECDH(ECCCurve::Curve25519))
        .can_sign(false)
        .can_encrypt(true)
        .can_authenticate(false);
    let mut params = SecretKeyParamsBuilder::default();
    params
        .key_type(KeyType::Ed25519Legacy)
        .can_certify(true)
        .can_sign(false)
        .can_encrypt(false)
        .primary_user_id(name.trim().to_owned())
        .subkeys(vec![encryption_subkey.build()?]);
    let secret = params
        .build()?
        .generate(thread_rng())?
        .sign(&mut thread_rng(), &Password::empty())?;
    let public = SignedPublicKey::from(secret.clone());
    let fingerprint = secret.fingerprint().to_string();
    Ok(GeneratedFriendKey {
        fingerprint,
        public_key_armored: public.to_armored_string(ArmorOptions::default())?,
        private_key_armored: secret.to_armored_string(ArmorOptions::default())?,
    })
}

fn wrap_for_friend(public_key: &[u8], symmetric_key: &[u8]) -> Result<FriendKeyWrapper> {
    let (public, _) = SignedPublicKey::from_reader_single(public_key)
        .context("failed to parse recovery friend's OpenPGP public key")?;
    public
        .verify()
        .context("recovery friend's OpenPGP public key failed self-signature validation")?;
    let encryption_subkey = public
        .public_subkeys
        .iter()
        .find(|subkey| subkey.is_encryption_key())
        .context("recovery friend's OpenPGP key has no encryption-capable subkey")?;
    let mut builder = MessageBuilder::from_bytes("vault-recovery-key", symmetric_key.to_vec())
        .seipd_v1(thread_rng(), SymmetricKeyAlgorithm::AES256);
    builder
        .encrypt_to_key(thread_rng(), encryption_subkey)
        .context("failed to wrap recovery key for friend")?;
    let encrypted = builder
        .to_vec(thread_rng())
        .context("failed to serialize friend recovery key wrapper")?;
    Ok(FriendKeyWrapper {
        fingerprint: public.fingerprint().to_string(),
        public_key_armored: public.to_armored_string(ArmorOptions::default())?,
        encrypted_symmetric_key: STANDARD.encode(encrypted),
    })
}

fn decrypt_payload(backup: &CloudRecoveryBackup, symmetric_key: &[u8]) -> Result<RecoveryPayload> {
    let plaintext = crypto::decrypt(symmetric_key, PAYLOAD_PURPOSE, &backup.encrypted_payload)?;
    serde_json::from_slice(&plaintext).context("decrypted cloud recovery payload is invalid")
}

fn encrypt_friend_manifest(
    symmetric_key: &[u8],
    friends: &[FriendKeyWrapper],
) -> Result<EncryptedBlob> {
    crypto::encrypt(
        symmetric_key,
        FRIEND_MANIFEST_PURPOSE,
        &serde_json::to_vec(friends)?,
    )
}

fn validate_friend_manifest(backup: &CloudRecoveryBackup, symmetric_key: &[u8]) -> Result<()> {
    let authenticated = crypto::decrypt(
        symmetric_key,
        FRIEND_MANIFEST_PURPOSE,
        &backup.encrypted_friend_manifest,
    )?;
    let expected = serde_json::to_vec(&backup.friends)?;
    if authenticated.as_slice() != expected {
        bail!("cloud recovery friend list failed authentication");
    }
    Ok(())
}

fn validate_backup(backup: &CloudRecoveryBackup) -> Result<()> {
    if backup.version != 1 || backup.kind != "vault-cloud-recovery" {
        bail!("unsupported cloud recovery backup");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn device_backup_contacts_are_bounded_and_empty_requests_keep_the_old_wire_shape() {
        let mut request = DeviceBackupKeyRequest {
            backup_id: "0".repeat(64),
            symmetric_key: [0; 32],
            friend_public_keys: vec![],
        };
        let wire = serde_json::to_value(&request).unwrap();
        assert!(wire.get("friend_public_keys").is_none());
        let legacy: DeviceBackupKeyRequest = serde_json::from_value(wire).unwrap();
        assert!(legacy.friend_fingerprints().unwrap().is_empty());
        let friend = generate_friend_key("Alice").unwrap();
        request
            .friend_public_keys
            .push(friend.public_key_armored.clone());
        assert_eq!(
            request.friend_fingerprints().unwrap(),
            vec![friend.fingerprint]
        );
        request.friend_public_keys.push(friend.public_key_armored);
        assert!(
            request
                .friend_fingerprints()
                .unwrap_err()
                .to_string()
                .contains("duplicate")
        );
        request.friend_public_keys = vec!["a".repeat(64 * 1024 + 1)];
        assert!(
            request
                .friend_fingerprints()
                .unwrap_err()
                .to_string()
                .contains("64 KiB")
        );
        request.friend_public_keys = vec![String::new(); 33];
        assert!(
            request
                .friend_fingerprints()
                .unwrap_err()
                .to_string()
                .contains("32 recovery contacts")
        );
    }
    struct Initialized {
        config: crate::storage::VaultConfig,
    }

    /// A paired vault's public configuration and both device keys, without any device app.
    fn fixture() -> (Initialized, DeviceKeys, DeviceKeys) {
        let secp = bitcoin::key::Secp256k1::new();
        let phone = DeviceKeys::generate(&secp).unwrap();
        let hww = DeviceKeys::generate(&secp).unwrap();
        (
            Initialized {
                config: config_for(&phone, &hww),
            },
            phone,
            hww,
        )
    }

    fn config_for(phone: &DeviceKeys, hww: &DeviceKeys) -> crate::storage::VaultConfig {
        let policy = crate::policy::VaultPolicy::new(phone.vault_pubkey, hww.vault_pubkey).unwrap();
        let (external, internal) = phone
            .hot_descriptors(&bitcoin::key::Secp256k1::new())
            .unwrap();
        crate::storage::VaultConfig {
            version: 1,
            network: "regtest".into(),
            phone_vault_pubkey: phone.vault_pubkey.to_string(),
            hww_vault_pubkey: hww.vault_pubkey.to_string(),
            phone_hot_external_descriptor: external,
            phone_hot_internal_descriptor: internal,
            vault_descriptor: policy.descriptor_string(),
            vault_address: policy.address.to_string(),
            phone_recovery_blocks: crate::PHONE_RECOVERY_BLOCKS,
            hww_recovery_blocks: crate::HWW_RECOVERY_BLOCKS,
            monthly_limit_sats: 0,
            emergency_access_limit_sats: 0,
        }
    }

    #[test]
    fn hww_and_each_friend_decrypt_the_same_authenticated_payload() {
        let (initialized, phone, hww) = fixture();
        let alice = generate_friend_key("Alice <alice@example.test>").unwrap();
        let bob = generate_friend_key("Bob <bob@example.test>").unwrap();
        let payload = RecoveryPayload::new(&initialized.config, &phone).unwrap();
        let backup = create_backup(
            &payload,
            &hww.seed,
            &[alice.public_key_armored, bob.public_key_armored],
        )
        .unwrap();

        assert_eq!(decrypt_with_hww(&backup, &hww.seed).unwrap(), payload);
        assert_eq!(
            decrypt_with_friend(&backup, alice.private_key_armored.as_bytes()).unwrap(),
            payload
        );
        assert_eq!(
            decrypt_with_friend(&backup, bob.private_key_armored.as_bytes()).unwrap(),
            payload
        );
    }

    #[test]
    fn encrypted_recovery_payload_preserves_a_vanity_vault_key_index() {
        use crate::policy::VaultPolicy;

        let secp = bitcoin::key::Secp256k1::new();
        let (_, phone, hww) = fixture();
        let phone = phone.with_vault_key_index(&secp, 42).unwrap();
        let config = config_for(&phone, &hww);
        let backup = create_backup(
            &RecoveryPayload::new(&config, &phone).unwrap(),
            &hww.seed,
            &[],
        )
        .unwrap();

        let recovered = decrypt_with_hww(&backup, &hww.seed).unwrap();
        assert_eq!(recovered.phone_vault_key_index, 42);
        recovered.validate_against(&config).unwrap();
        let reconstructed = recovered.recovery_config().unwrap();
        assert_eq!(reconstructed.vault_descriptor, config.vault_descriptor);
        assert_eq!(reconstructed.vault_address, config.vault_address);
        assert_eq!(reconstructed.hww_vault_pubkey, config.hww_vault_pubkey);
        assert_eq!(reconstructed.phone_vault_pubkey, config.phone_vault_pubkey);
        assert_eq!(reconstructed.monthly_limit_sats, 0); // Unknown from this payload.

        let mut wrong_key = recovered.clone();
        wrong_key.phone_vault_key_index = 0;
        assert!(wrong_key.recovery_config().is_err());
        let mut wrong_network = recovered.clone();
        wrong_network.network = "bitcoin".into();
        assert!(wrong_network.recovery_config().is_err());

        // Even a valid descriptor/address pair must use the fixed vault construction.
        // Recompute the address/checksum so this tests policy validation, not a stale checksum.
        let modified = recovered
            .vault_descriptor
            .split('#')
            .next()
            .unwrap()
            .replace("older(61200)", "older(10)");
        let weakened =
            VaultPolicy::from_descriptor_for_network(&modified, bitcoin::Network::Regtest).unwrap();
        let mut wrong_policy = recovered.clone();
        wrong_policy.vault_descriptor = weakened.descriptor.to_string();
        wrong_policy.vault_address = weakened.address.to_string();
        assert!(wrong_policy.recovery_config().is_err());
    }

    #[test]
    fn an_unconfigured_friend_cannot_decrypt_the_backup() {
        let (initialized, phone, hww) = fixture();
        let stranger = generate_friend_key("Mallory").unwrap();
        let backup = create_backup(
            &RecoveryPayload::new(&initialized.config, &phone).unwrap(),
            &hww.seed,
            &[],
        )
        .unwrap();
        assert!(decrypt_with_friend(&backup, stranger.private_key_armored.as_bytes()).is_err());
    }

    #[test]
    fn friend_list_tampering_fails_authentication() {
        let (initialized, phone, hww) = fixture();
        let alice = generate_friend_key("Alice").unwrap();
        let backup = create_backup(
            &RecoveryPayload::new(&initialized.config, &phone).unwrap(),
            &hww.seed,
            &[alice.public_key_armored],
        )
        .unwrap();

        let mut deleted = backup.clone();
        deleted.friends.clear();
        assert!(decrypt_with_hww(&deleted, &hww.seed).is_err());

        let mut inserted = backup.clone();
        inserted.friends.push(inserted.friends[0].clone());
        assert!(decrypt_with_hww(&inserted, &hww.seed).is_err());

        let mut modified = backup;
        modified.friends[0].fingerprint.push('0');
        assert!(decrypt_with_hww(&modified, &hww.seed).is_err());
    }
}
