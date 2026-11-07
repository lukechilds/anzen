use super::super::browser;
use base64::{Engine, engine::general_purpose::STANDARD};
use serde::{Deserialize, de::DeserializeOwned};
use serde_json::{Value, json};
use std::{
    io,
    path::{Path, PathBuf},
};

fn call<T: DeserializeOwned>(request: Value) -> io::Result<T> {
    // Filesystem errors retain their kind so existing safe retry/not-found handling is shared.
    let result: Value = browser::call(&json!({ "type": "storage", "request": request }))
        .map_err(io::Error::other)?;
    if let Some(error) = result.get("error") {
        let kind = match error.get("kind").and_then(Value::as_str) {
            Some("not_found") => io::ErrorKind::NotFound,
            Some("already_exists") => io::ErrorKind::AlreadyExists,
            Some("permission_denied") => io::ErrorKind::PermissionDenied,
            _ => io::ErrorKind::Other,
        };
        return Err(io::Error::new(
            kind,
            error
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or("browser storage failed"),
        ));
    }
    serde_json::from_value(result).map_err(io::Error::other)
}

pub fn read(path: impl AsRef<Path>) -> io::Result<Vec<u8>> {
    let data: String = call(json!({"method":"read", "path":path.as_ref()}))?;
    STANDARD.decode(data).map_err(io::Error::other)
}
pub fn read_to_string(path: impl AsRef<Path>) -> io::Result<String> {
    String::from_utf8(read(path)?)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
}
pub fn write(path: impl AsRef<Path>, bytes: &[u8]) -> io::Result<()> {
    call(json!({"method":"write", "path":path.as_ref(), "data":STANDARD.encode(bytes)}))
}
pub fn create_dir_all(path: impl AsRef<Path>) -> io::Result<()> {
    call(json!({"method":"mkdir", "path":path.as_ref()}))
}
pub fn remove_file(path: impl AsRef<Path>) -> io::Result<()> {
    call(json!({"method":"remove_file", "path":path.as_ref()}))
}
pub fn remove_dir_all(path: impl AsRef<Path>) -> io::Result<()> {
    call(json!({"method":"remove_dir", "path":path.as_ref()}))
}
pub fn rename(from: impl AsRef<Path>, to: impl AsRef<Path>) -> io::Result<()> {
    call(json!({"method":"rename", "path":from.as_ref(), "destination":to.as_ref()}))
}
pub fn exists(path: &Path) -> bool {
    call(json!({"method":"exists", "path":path}))
        .expect("browser storage metadata failed; refusing to guess whether keys exist")
}
pub fn is_file(path: &Path) -> bool {
    call(json!({"method":"is_file", "path":path}))
        .expect("browser storage metadata failed; refusing to guess whether state exists")
}

#[derive(Deserialize)]
pub struct DirEntry {
    path: PathBuf,
    directory: bool,
}
pub struct FileType {
    directory: bool,
}
impl DirEntry {
    pub fn path(&self) -> PathBuf {
        self.path.clone()
    }
    pub fn file_type(&self) -> io::Result<FileType> {
        Ok(FileType {
            directory: self.directory,
        })
    }
}
impl FileType {
    pub fn is_file(&self) -> bool {
        !self.directory
    }
}
pub type ReadDir = std::vec::IntoIter<io::Result<DirEntry>>;
pub fn read_dir(path: impl AsRef<Path>) -> io::Result<ReadDir> {
    let entries: Vec<DirEntry> = call(json!({"method":"list", "path":path.as_ref()}))?;
    Ok(entries.into_iter().map(Ok).collect::<Vec<_>>().into_iter())
}
