//! Opaque encrypted-object storage for the local cloud simulation, outside both device roots.
//! Only the playground administrator can access it; a signer capability is not cloud access.
use super::*;
use anzen_wallet::api::CloudRecoveryBackup;
use bitcoin::hashes::{Hash, sha256};
use std::{io::Write, str::FromStr};

#[derive(Deserialize)]
#[serde(tag = "method", rename_all = "snake_case", deny_unknown_fields)]
pub enum Request {
    Save { backup: Box<CloudRecoveryBackup> },
    List,
    Load { id: String },
}

pub async fn control(
    State(host): State<Shared>,
    headers: HeaderMap,
    Json(request): Json<Request>,
) -> ApiResult {
    require_admin(&host, &headers).map_err(failure)?;
    let result = tokio::task::spawn_blocking(move || -> Result<Value> {
        let directory = host.directory.join("cloud");
        match request {
            Request::Save { backup } => {
                if backup.version != 1 || backup.kind != "vault-cloud-recovery" {
                    bail!("unsupported encrypted cloud backup");
                }
                let bytes = serde_json::to_vec(&backup)?;
                let id = sha256::Hash::hash(&bytes).to_string();
                std::fs::create_dir_all(&directory)?;
                let path = directory.join(format!("{id}.json"));
                let mut temporary = tempfile::NamedTempFile::new_in(&directory)?;
                temporary.write_all(&bytes)?;
                temporary.as_file().sync_all()?;
                if let Err(error) = temporary.persist_noclobber(&path) {
                    if error.error.kind() != std::io::ErrorKind::AlreadyExists
                        || std::fs::read(&path)? != bytes
                    {
                        bail!(
                            "could not safely store the encrypted cloud backup: {}",
                            error.error
                        );
                    }
                }
                Ok(json!({"id": id}))
            }
            Request::List => {
                let mut ids = vec![];
                if directory.exists() {
                    for entry in std::fs::read_dir(directory)? {
                        let path = entry?.path();
                        if path
                            .extension()
                            .is_some_and(|extension| extension == "json")
                        {
                            if let Some(id) = path.file_stem().and_then(|name| name.to_str()) {
                                if sha256::Hash::from_str(id).is_ok() {
                                    ids.push(id.to_owned());
                                }
                            }
                        }
                    }
                }
                ids.sort();
                Ok(json!({"backups": ids}))
            }
            Request::Load { id } => {
                let id = sha256::Hash::from_str(&id)?.to_string();
                let bytes = std::fs::read(directory.join(format!("{id}.json")))?;
                if sha256::Hash::hash(&bytes).to_string() != id {
                    bail!("cloud object failed its content hash check");
                }
                Ok(serde_json::from_slice(&bytes)?)
            }
        }
    })
    .await
    .map_err(failure)?
    .map_err(failure)?;
    Ok(Json(result))
}
