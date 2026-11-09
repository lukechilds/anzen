//! The phone's half of every joint vault signature, and the nonces it keeps in between.
//!
//! # Nonce handling
//!
//! A cooperative vault spend is one MuSig2 (BIP327) signature from both devices. Each signer
//! contributes a secret nonce, and a secret nonce must sign exactly once: if the phone signs two
//! different sessions with the same secret nonce, the equations they produce reveal its vault key.
//! An attacker holding the signer's key could then add the phone's key and spend the savings at
//! once instead of waiting out the signer's time lock.
//!
//! The signer avoids the problem by signing last and deriving its nonce from the request
//! ([`anzen_common::musig::sign_last`]), so it keeps nothing. The phone signs first, so it must
//! hold its secret nonces from the request until the signer answers. These rules keep that safe:
//!
//! 1. Every request gets fresh random nonces ([`start`]), and each session signs at most once.
//!    Answers that arrive later for the same nonces, such as a resent request or a retried
//!    activation, are never signed again: [`finish`] returns the signatures it already made.
//! 2. A session signs only the exact messages, keys and nonces it was started for, and nothing
//!    else.
//! 3. The signer's halves are verified before the phone signs anything, so only the real signer
//!    can make the phone produce a signature.
//! 4. [`finish`] computes the signatures in memory, then replaces the session's secret nonces with
//!    the finished signatures in one atomic write, before any signature is returned, stored
//!    elsewhere or broadcast. A crash before that write has released nothing and leaves the
//!    session as it was; a crash after it leaves only finished signatures, which retries reuse.
//!    Once finished, the secret nonces are gone.
//! 5. Sessions live in the phone's device-only storage and are never part of the cloud backup or
//!    any exported message. A copy of a pending session restored onto a second device could sign
//!    twice, so hosts keep the data directory out of device backups: the iOS host marks it
//!    excluded from backup and the Android host uses `noBackupFilesDir`.

use anyhow::{Context, Result, bail, ensure};
use anzen_common::{
    keys::DeviceKeys,
    musig,
    protocol::{self, SoftwareCrypto, key},
    storage::{self, fs::StoragePath},
};
use anzen_protocol::{
    hex,
    policy::PolicyGraph,
    script::{Scripts, VaultKeys},
    spend::Spend,
    wire::{CooperativeSignature, PubNonce},
};
use bitcoin::hashes::{Hash, sha256};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use zeroize::Zeroizing;

/// One joint signature: the message both devices sign and the vault it belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Job {
    pub keys: VaultKeys,
    #[serde(with = "hex32")]
    pub merkle_root: [u8; 32],
    #[serde(with = "hex32")]
    pub message: [u8; 32],
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "kebab-case", deny_unknown_fields)]
enum Session {
    /// Started: each job's public nonce and the secret nonce that may sign it, once.
    Pending {
        jobs: Vec<Job>,
        nonces: Vec<PubNonce>,
        secret_nonces: Vec<String>,
    },
    /// Finished: the secret nonces are gone; only the finished signatures remain.
    Finished {
        jobs: Vec<Job>,
        nonces: Vec<PubNonce>,
        signatures: Vec<String>,
    },
}

/// Start a session for `jobs`: fresh nonces, stored until [`finish`]. Returns the public nonces
/// the request carries to the signer.
pub fn start(data_dir: &Path, phones: &[&DeviceKeys], jobs: &[Job]) -> Result<Vec<PubNonce>> {
    let mut nonces = Vec::with_capacity(jobs.len());
    let mut secret_nonces = Vec::with_capacity(jobs.len());
    for job in jobs {
        let phone = phone_for(phones, job)?;
        let (secret, public) = musig::phone_nonce(
            &phone.vault_keypair.secret_bytes(),
            &job.keys,
            &job.merkle_root,
            &job.message,
        )?;
        let secret = Zeroizing::new(secret);
        secret_nonces.push(hex::encode(&*secret));
        nonces.push(public);
    }
    let path = session_path(data_dir, &nonces)?;
    ensure!(
        !path.storage_exists(),
        "a signing session with these nonces already exists"
    );
    storage::write_json(
        &path,
        &Session::Pending {
            jobs: jobs.to_vec(),
            nonces: nonces.clone(),
            secret_nonces,
        },
    )?;
    Ok(nonces)
}

/// Finish the session that `nonces` started with the signer's halves, returning the final BIP340
/// signature for each job. Finishing an already finished session returns its signatures again.
pub fn finish(
    data_dir: &Path,
    phones: &[&DeviceKeys],
    jobs: &[Job],
    nonces: &[PubNonce],
    signer: &[CooperativeSignature],
) -> Result<Vec<[u8; 64]>> {
    ensure!(
        jobs.len() == nonces.len() && jobs.len() == signer.len(),
        "the signer did not answer every joint signature"
    );
    let path = session_path(data_dir, nonces)?;
    let session: Session = storage::read_json(&path).context(
        "this phone has no signing session for that request; propose it again from this phone",
    )?;
    match session {
        Session::Finished {
            jobs: stored,
            nonces: stored_nonces,
            signatures,
        } => {
            ensure!(
                stored == jobs && stored_nonces == nonces,
                "approval does not match the finished signing session"
            );
            signatures.iter().map(|text| decode::<64>(text)).collect()
        }
        Session::Pending {
            jobs: stored,
            nonces: stored_nonces,
            secret_nonces,
        } => {
            ensure!(
                stored == jobs && stored_nonces == nonces && secret_nonces.len() == jobs.len(),
                "approval does not match the signing session this phone started"
            );
            // Check every half first: an invalid answer must not consume the session.
            for ((job, nonce), half) in jobs.iter().zip(nonces).zip(signer) {
                musig::verify_signer_half(&job.keys, &job.merkle_root, &job.message, nonce, half)?;
            }
            let mut signatures = Vec::with_capacity(jobs.len());
            for (((job, nonce), half), secret_nonce) in
                jobs.iter().zip(nonces).zip(signer).zip(&secret_nonces)
            {
                let phone = phone_for(phones, job)?;
                let secret_nonce = Zeroizing::new(decode::<64>(secret_nonce)?);
                signatures.push(musig::complete(
                    &phone.vault_keypair.secret_bytes(),
                    &secret_nonce,
                    &job.keys,
                    &job.merkle_root,
                    &job.message,
                    nonce,
                    half,
                )?);
            }
            // The atomic replacement that destroys the secret nonces. Nothing leaves this
            // function until it has succeeded.
            storage::write_json(
                &path,
                &Session::Finished {
                    jobs: jobs.to_vec(),
                    nonces: nonces.to_vec(),
                    signatures: signatures
                        .iter()
                        .map(|signature| hex::encode(signature))
                        .collect(),
                },
            )
            .context("could not record the finished signatures; nothing was signed")?;
            Ok(signatures)
        }
    }
}

/// The phone key and session store a request's nonces come from.
#[derive(Clone, Copy)]
pub struct Phone<'a> {
    pub data_dir: &'a Path,
    pub keys: &'a DeviceKeys,
}

impl Phone<'_> {
    pub fn start(&self, jobs: &[Job]) -> Result<Vec<PubNonce>> {
        start(self.data_dir, &[self.keys], jobs)
    }

    pub fn finish(
        &self,
        jobs: &[Job],
        nonces: &[PubNonce],
        signer: &[CooperativeSignature],
    ) -> Result<Vec<[u8; 64]>> {
        finish(self.data_dir, &[self.keys], jobs, nonces, signer)
    }
}

/// The joint signatures of every vault input of a policy graph, in signing-job order.
pub fn policy_jobs(scripts: &Scripts, graph: &PolicyGraph) -> Result<Vec<Job>> {
    let jobs = protocol::protocol(graph.cooperative_signing_jobs(&mut SoftwareCrypto::new()))?;
    Ok(jobs
        .iter()
        .map(|job| vault_job(scripts, job.sighash))
        .collect())
}

/// The joint signatures of every input of a cooperative spend.
pub fn spend_jobs(scripts: &Scripts, spend: &Spend) -> Result<Vec<Job>> {
    let messages = protocol::protocol(spend.key_path_sighashes(&mut SoftwareCrypto::new()))?;
    Ok(messages
        .into_iter()
        .map(|message| vault_job(scripts, message))
        .collect())
}

fn vault_job(scripts: &Scripts, message: [u8; 32]) -> Job {
    Job {
        keys: scripts.vault.keys,
        merkle_root: scripts.vault.merkle_root,
        message,
    }
}

fn phone_for<'a>(phones: &[&'a DeviceKeys], job: &Job) -> Result<&'a DeviceKeys> {
    phones
        .iter()
        .copied()
        .find(|phone| key(phone.vault_pubkey) == job.keys.phone)
        .context("this phone does not hold the key for that vault")
}

/// Sessions are named by their public nonces, which are random and unique to one request.
fn session_path(data_dir: &Path, nonces: &[PubNonce]) -> Result<PathBuf> {
    if nonces.is_empty() {
        bail!("a signing session needs at least one joint signature");
    }
    let mut bytes = Vec::with_capacity(nonces.len() * 66);
    for nonce in nonces {
        bytes.extend_from_slice(&nonce.0);
    }
    Ok(data_dir
        .join("phone/cosigning")
        .join(format!("{}.json", sha256::Hash::hash(&bytes))))
}

fn decode<const N: usize>(text: &str) -> Result<[u8; N]> {
    hex::decode_array(text).map_err(|_| anyhow::anyhow!("corrupt signing session"))
}

mod hex32 {
    use serde::{Deserialize, Deserializer, Serializer, de::Error};

    pub fn serialize<S: Serializer>(bytes: &[u8; 32], serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&anzen_protocol::hex::encode(bytes))
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<[u8; 32], D::Error> {
        let text = String::deserialize(deserializer)?;
        anzen_protocol::hex::decode_array(&text).map_err(D::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> (tempfile::TempDir, DeviceKeys, DeviceKeys, Vec<Job>) {
        let directory = tempfile::tempdir().unwrap();
        let secp = bitcoin::key::Secp256k1::new();
        let phone = DeviceKeys::generate(&secp).unwrap();
        let hww = DeviceKeys::generate(&secp).unwrap();
        let keys = VaultKeys {
            phone: key(phone.vault_pubkey),
            hww: key(hww.vault_pubkey),
            recovery_locktime: anzen_common::DEFAULT_RECOVERY_LOCKTIME,
        };
        let root = Scripts::new(&mut SoftwareCrypto::new(), &keys)
            .unwrap()
            .vault
            .merkle_root;
        let jobs = (0..3)
            .map(|index| Job {
                keys,
                merkle_root: root,
                message: [index; 32],
            })
            .collect();
        (directory, phone, hww, jobs)
    }

    fn answer(hww: &DeviceKeys, jobs: &[Job], nonces: &[PubNonce]) -> Vec<CooperativeSignature> {
        jobs.iter()
            .zip(nonces)
            .map(|(job, nonce)| {
                musig::sign_last(
                    &hww.vault_keypair.secret_bytes(),
                    &job.keys,
                    &job.merkle_root,
                    &job.message,
                    nonce,
                )
                .unwrap()
            })
            .collect()
    }

    #[test]
    fn finishing_destroys_the_secret_nonces_and_retries_reuse_the_signatures() {
        let (directory, phone, hww, jobs) = fixture();
        let nonces = start(directory.path(), &[&phone], &jobs).unwrap();
        let path = session_path(directory.path(), &nonces).unwrap();
        assert!(
            std::fs::read_to_string(&path)
                .unwrap()
                .contains("secret_nonces")
        );
        let halves = answer(&hww, &jobs, &nonces);
        let signatures = finish(directory.path(), &[&phone], &jobs, &nonces, &halves).unwrap();
        let stored = std::fs::read_to_string(&path).unwrap();
        assert!(!stored.contains("secret_nonces"));
        assert!(stored.contains("finished"));
        assert_eq!(
            finish(directory.path(), &[&phone], &jobs, &nonces, &halves).unwrap(),
            signatures
        );
    }

    #[test]
    fn a_session_never_signs_anything_it_was_not_started_for() {
        let (directory, phone, hww, jobs) = fixture();
        let nonces = start(directory.path(), &[&phone], &jobs).unwrap();
        let halves = answer(&hww, &jobs, &nonces);
        // A forged half is refused and does not consume the session.
        let mut forged = halves.clone();
        forged[1].partial.0[0] ^= 1;
        assert!(finish(directory.path(), &[&phone], &jobs, &nonces, &forged).is_err());
        let path = session_path(directory.path(), &nonces).unwrap();
        assert!(
            std::fs::read_to_string(&path)
                .unwrap()
                .contains("secret_nonces")
        );
        // Different messages under the same nonces are refused.
        let mut other = jobs.clone();
        other[0].message = [9; 32];
        let other_halves = answer(&hww, &other, &nonces);
        assert!(finish(directory.path(), &[&phone], &other, &nonces, &other_halves).is_err());
        // Nonces this phone never issued have no session.
        let mut unknown = nonces.clone();
        unknown.swap(0, 1);
        assert!(finish(directory.path(), &[&phone], &jobs, &unknown, &halves).is_err());
        finish(directory.path(), &[&phone], &jobs, &nonces, &halves).unwrap();
    }
}
