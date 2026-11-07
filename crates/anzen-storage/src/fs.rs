//! Device-local filesystem boundary. Native behavior remains the ordinary filesystem;
//! browser implementations delegate to their device's durable storage adapter.

#[cfg(not(target_arch = "wasm32"))]
pub use std::fs::*;

#[cfg(target_arch = "wasm32")]
mod browser;
#[cfg(target_arch = "wasm32")]
pub use browser::*;

use std::{io, path::Path};

/// Explicit names prevent accidentally calling `Path::exists` on browser builds, where
/// Rust's standard filesystem API is unavailable. Paths identify device-local records.
pub trait StoragePath {
    fn storage_exists(&self) -> bool;
    fn storage_is_file(&self) -> bool;
    fn storage_read_dir(&self) -> io::Result<ReadDir>;
}

impl StoragePath for Path {
    fn storage_exists(&self) -> bool {
        #[cfg(not(target_arch = "wasm32"))]
        {
            self.exists()
        }
        #[cfg(target_arch = "wasm32")]
        {
            browser::exists(self)
        }
    }

    fn storage_is_file(&self) -> bool {
        #[cfg(not(target_arch = "wasm32"))]
        {
            self.is_file()
        }
        #[cfg(target_arch = "wasm32")]
        {
            browser::is_file(self)
        }
    }

    fn storage_read_dir(&self) -> io::Result<ReadDir> {
        read_dir(self)
    }
}
