//! The blob bytes: read, appended, and deleted through a trait the shell
//! implements over the directory it owns.
//!
//! Metadata — hashes, permissions, progress — lives in SQLite; this is only
//! the file itself. The shell gives the core a `BlobStore` at startup, the way
//! it gives it the store directory. `docs/sync.md#blob-sync`.

use anyhow::Result;
use std::collections::HashMap;
use std::sync::Mutex;

/// Where blob bytes go. Implemented by each shell over its data directory;
/// tested in-core over memory.
pub trait BlobStore: Send + Sync {
    /// Whether the blob's bytes are held.
    fn has(&self, sha256: &str) -> Result<bool>;
    /// The length of the held blob, or `None` if it is not held.
    fn len(&self, sha256: &str) -> Result<Option<u64>>;
    /// Read `len` bytes from `offset`, or as many as are there.
    fn read(&self, sha256: &str, offset: u64, len: u64) -> Result<Vec<u8>>;
    /// Append bytes to the blob, creating it if missing.
    fn append(&self, sha256: &str, bytes: &[u8]) -> Result<()>;
    /// Delete the blob's bytes, if any are held.
    fn delete(&self, sha256: &str) -> Result<()>;
}

/// An in-memory store, for tests and tooling.
#[derive(Debug, Default)]
pub struct MemoryBlobStore {
    blobs: Mutex<HashMap<String, Vec<u8>>>,
}

impl BlobStore for MemoryBlobStore {
    fn has(&self, sha256: &str) -> Result<bool> {
        Ok(self.blobs.lock().unwrap().contains_key(sha256))
    }

    fn len(&self, sha256: &str) -> Result<Option<u64>> {
        Ok(self
            .blobs
            .lock()
            .unwrap()
            .get(sha256)
            .map(|bytes| bytes.len() as u64))
    }

    fn read(&self, sha256: &str, offset: u64, len: u64) -> Result<Vec<u8>> {
        let blobs = self.blobs.lock().unwrap();
        let Some(bytes) = blobs.get(sha256) else {
            return Ok(Vec::new());
        };

        let start = offset as usize;
        let end = (start + len as usize).min(bytes.len());

        Ok(bytes[start.min(bytes.len())..end].to_vec())
    }

    fn append(&self, sha256: &str, bytes: &[u8]) -> Result<()> {
        self.blobs
            .lock()
            .unwrap()
            .entry(sha256.to_string())
            .or_default()
            .extend_from_slice(bytes);

        Ok(())
    }

    fn delete(&self, sha256: &str) -> Result<()> {
        self.blobs.lock().unwrap().remove(sha256);

        Ok(())
    }
}
