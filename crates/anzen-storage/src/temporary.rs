//! Temporary policy validation workspaces. These use the same platform storage boundary
//! as the caller; browser paths never fall through to an unavailable OS filesystem.

#[cfg(not(target_arch = "wasm32"))]
pub use tempfile::tempdir;

#[cfg(target_arch = "wasm32")]
pub struct TempDir(std::path::PathBuf);

#[cfg(target_arch = "wasm32")]
impl TempDir {
    pub fn path(&self) -> &std::path::Path {
        &self.0
    }
}

#[cfg(target_arch = "wasm32")]
impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = super::fs::remove_dir_all(&self.0);
    }
}

#[cfg(target_arch = "wasm32")]
pub fn tempdir() -> std::io::Result<TempDir> {
    use rand::RngCore;
    let mut nonce = [0u8; 16];
    rand::rngs::OsRng.fill_bytes(&mut nonce);
    let id: String = nonce.iter().map(|byte| format!("{byte:02x}")).collect();
    let path = std::path::PathBuf::from(format!("/temporary/{id}"));
    super::fs::create_dir_all(&path)?;
    Ok(TempDir(path))
}
