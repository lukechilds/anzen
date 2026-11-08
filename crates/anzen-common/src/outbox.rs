//! Durable copies of messages emitted by a device application, not an approval queue.
//!
//! Only successful application operations may write here. Reading or sharing an old
//! message never approves it; the recipient must run its usual validation/review path.

use super::storage::{self, PublicDeviceFile};
use crate::storage::fs::StoragePath;
use anyhow::{Context, Result, bail};
use bitcoin::hashes::{Hash, sha256};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum OutgoingMessageKind {
    SignerIdentity,
    VaultSetup,
    VaultAccepted,
    PolicyProposal,
    PolicyApproved,
    SweepProposal,
    SweepApproved,
    BackupProposal,
    BackupApproved,
    RecoveryVault,
    OfflineActionProposal,
    OfflineActionApproved,
    RestoreProposal,
    RestoreApproved,
    PhoneRotationProposal,
    PhoneRotationApproved,
    PhoneRotationCompleted,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OutgoingMessage {
    pub kind: OutgoingMessageKind,
    pub body: Value,
}

/// No balances, addresses or message bodies are needed to display the saved-message list.
#[derive(Debug, Serialize)]
pub struct OutboxItem {
    pub id: String,
    pub kind: OutgoingMessageKind,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Record {
    version: u8,
    owner: PublicDeviceFile,
    message: OutgoingMessage,
}

impl Record {
    fn id(&self) -> Result<String> {
        let mut bytes = b"anzen-device-outbox-v1\0".to_vec();
        bytes.extend(serde_json::to_vec(self)?);
        Ok(sha256::Hash::hash(&bytes).to_string())
    }
}

pub fn save(
    directory: &Path,
    owner: PublicDeviceFile,
    kind: OutgoingMessageKind,
    body: Value,
) -> Result<()> {
    if !body.is_object() || serde_json::to_vec(&body)?.len() > 8 * 1024 * 1024 {
        bail!("outgoing protocol message must be an object no larger than 8 MiB");
    }
    let record = Record {
        version: 1,
        owner,
        message: OutgoingMessage { kind, body },
    };
    let id = record.id()?;
    let file = path(directory, &id)?;
    if file.storage_exists() {
        // Idempotent output must not overwrite a damaged or wrong-device record.
        read(directory, &record.owner, &id)?;
        return Ok(());
    }
    storage::write_json(&file, &record)
}

pub fn read(directory: &Path, owner: &PublicDeviceFile, id: &str) -> Result<OutgoingMessage> {
    let record: Record = storage::read_json(&path(directory, id)?)?;
    if record.version != 1
        || record.id()? != id
        || serde_json::to_vec(&record.owner)? != serde_json::to_vec(owner)?
    {
        bail!("saved protocol message does not match its ID or this device");
    }
    Ok(record.message)
}

pub fn list(directory: &Path, owner: &PublicDeviceFile) -> Result<Vec<OutboxItem>> {
    if !directory.storage_exists() {
        return Ok(vec![]);
    }
    let mut ids = Vec::new();
    for entry in crate::storage::fs::read_dir(directory)? {
        let entry = entry?;
        if entry.path().extension().is_some_and(|ext| ext == "json") {
            if !entry.file_type()?.is_file() {
                bail!("saved protocol message is not a regular file");
            }
            ids.push(
                entry
                    .path()
                    .file_stem()
                    .and_then(|stem| stem.to_str())
                    .context("invalid saved protocol message filename")?
                    .to_owned(),
            );
        }
    }
    ids.sort();
    ids.into_iter()
        .map(|id| {
            let message = read(directory, owner, &id)?;
            Ok(OutboxItem {
                id,
                kind: message.kind,
            })
        })
        .collect()
}

fn path(directory: &Path, id: &str) -> Result<PathBuf> {
    if id.len() != 64
        || !id
            .bytes()
            .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
    {
        bail!("invalid saved protocol message ID");
    }
    Ok(directory.join(format!("{id}.json")))
}
