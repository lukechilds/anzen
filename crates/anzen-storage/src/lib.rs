//! Device-local storage boundary.
//!
//! Every Anzen application persists through these functions: private files replaced atomically
//! on native platforms, optional OS-keyed encryption for native apps, and a synchronous bridge to
//! browser storage in WASM workers. Nothing here knows what the stored records mean.

#[cfg(target_arch = "wasm32")]
pub mod browser;
pub mod fs;
mod protected;
mod temporary;

pub use protected::with_protected_storage;
pub use temporary::tempdir;

use anyhow::{Context, Result};
#[cfg(not(target_arch = "wasm32"))]
use fs::StoragePath;
use serde::{Serialize, de::DeserializeOwned};
#[cfg(not(target_arch = "wasm32"))]
use std::io::Write;
use std::path::Path;

pub fn write_json<T: Serialize>(path: &Path, value: &T) -> Result<()> {
    let plaintext = zeroize::Zeroizing::new(serde_json::to_vec_pretty(value)?);
    write_private(path, &protected::encode(path, &plaintext)?)
}

/// Replace a file atomically on the same filesystem. The temporary file is private from
/// creation, and is flushed before the rename so interrupted writes cannot truncate live keys
/// or the active schedule. Persist the directory entry as well on Unix.
#[cfg(not(target_arch = "wasm32"))]
pub fn write_private(path: &Path, bytes: &[u8]) -> Result<()> {
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    #[cfg(unix)]
    let mut created_parents = Vec::new();
    #[cfg(unix)]
    for ancestor in parent.ancestors() {
        if ancestor.as_os_str().is_empty() || ancestor.storage_exists() {
            break;
        }
        created_parents.push(
            ancestor
                .parent()
                .filter(|directory| !directory.as_os_str().is_empty())
                .unwrap_or_else(|| Path::new(".")),
        );
    }
    fs::create_dir_all(parent).with_context(|| format!("failed to create {}", parent.display()))?;
    // A new epoch may also create several directories. Flush their containing directories so
    // the staged files do not depend on directory entries that are still only in memory.
    #[cfg(unix)]
    for directory in created_parents.into_iter().rev() {
        fs::File::open(directory)?.sync_all()?;
    }
    let mut temporary = tempfile::NamedTempFile::new_in(parent)?;
    temporary
        .write_all(bytes)
        .with_context(|| format!("failed to write {}", path.display()))?;
    temporary.as_file().sync_all()?;
    temporary
        .persist(path)
        .with_context(|| format!("failed to replace {}", path.display()))?;
    #[cfg(unix)]
    fs::File::open(parent)?.sync_all()?;
    Ok(())
}

pub fn read_json<T: DeserializeOwned>(path: &Path) -> Result<T> {
    let bytes = protected::decode(
        path,
        fs::read(path).with_context(|| format!("failed to read {}", path.display()))?,
    )?;
    serde_json::from_slice(&bytes).with_context(|| format!("invalid JSON in {}", path.display()))
}

#[cfg(target_arch = "wasm32")]
pub fn write_private(path: &Path, bytes: &[u8]) -> Result<()> {
    fs::write(path, bytes).with_context(|| format!("failed to persist {}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn private_files_are_atomically_replaced_and_remain_private() {
        let dir = crate::tempdir().unwrap();
        let path = dir.path().join("state.json");
        write_json(&path, &vec![1_u64; 100]).unwrap();
        let old_handle = fs::File::open(&path).unwrap();
        write_json(&path, &vec![2_u64; 100]).unwrap();
        assert_eq!(read_json::<Vec<u64>>(&path).unwrap(), vec![2; 100]);
        // An open reader still sees the complete old inode, not truncated/partially written JSON.
        assert_eq!(
            serde_json::from_reader::<_, Vec<u64>>(old_handle).unwrap(),
            vec![1; 100]
        );
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 1);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
    }

    #[test]
    fn private_writes_create_nested_state_directories() {
        let dir = crate::tempdir().unwrap();
        let path = dir.path().join("phone/transactions/epoch/schedule.json");
        write_json(&path, &vec![1_u64, 2, 3]).unwrap();
        assert_eq!(read_json::<Vec<u64>>(&path).unwrap(), vec![1, 2, 3]);
    }

    #[test]
    fn failed_replacement_preserves_destination_and_cleans_temporary_file() {
        let dir = crate::tempdir().unwrap();
        let path = dir.path().join("not-a-file");
        fs::create_dir(&path).unwrap();
        write_private(&path.join("retained"), b"unchanged").unwrap();
        assert!(write_private(&path, b"replacement").is_err());
        assert_eq!(fs::read(path.join("retained")).unwrap(), b"unchanged");
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 1);
    }
}
