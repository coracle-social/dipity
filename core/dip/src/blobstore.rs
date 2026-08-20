//! The blob bytes: read, appended, and deleted through a store.
//!
//! Metadata — hashes, permissions, progress — lives in SQLite; this is only
//! the file itself. The core ships a file-backed store over the directory the
//! shell provides, the same way it opens the SQLite store in one; the trait is
//! what lets tests run on memory and a shell slot in a custom backing (an
//! encrypted one, say) without touching the sync layer.
//! `docs/sync.md#blob-sync`.

use std::collections::HashMap;
use std::fs::{self, File, OpenOptions};
use std::io::{ErrorKind, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use anyhow::{Context, Result};

/// Where blob bytes go.
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

/// A blob store over a directory the shell provides, one file per blob named
/// by its lowercase hex sha256.
///
/// Blobs are content-addressed and appended as verified groups arrive, so the
/// whole transfer is `create` + `append` until the sync layer marks it done.
/// The directory is the one the shell already owns, alongside where it tells
/// the core to open the database.
pub struct FileBlobStore {
    /// The directory the files live in, created on open.
    directory: PathBuf,
}

impl FileBlobStore {
    /// Open (creating if needed) the directory blob files live in.
    pub fn open(directory: impl AsRef<Path>) -> Result<Self> {
        let directory = directory.as_ref();

        fs::create_dir_all(directory)
            .with_context(|| format!("creating blob store directory {}", directory.display()))?;

        Ok(Self {
            directory: directory.to_path_buf(),
        })
    }

    /// The file a hash lives in. A hash that is not 32 lowercase hex bytes is
    /// an error rather than a path: reach outside this directory and the
    /// store would be writing wherever the caller pointed it.
    fn path(&self, sha256: &str) -> Result<PathBuf> {
        let key = crate::db::sql::hex_key(sha256)?;

        Ok(self.directory.join(key))
    }
}

impl BlobStore for FileBlobStore {
    fn has(&self, sha256: &str) -> Result<bool> {
        Ok(self.path(sha256)?.is_file())
    }

    fn len(&self, sha256: &str) -> Result<Option<u64>> {
        let path = self.path(sha256)?;

        match fs::metadata(&path) {
            Ok(metadata) if metadata.is_file() => Ok(Some(metadata.len())),
            _ => Ok(None),
        }
    }

    fn read(&self, sha256: &str, offset: u64, len: u64) -> Result<Vec<u8>> {
        let path = self.path(sha256)?;
        let mut file = match File::open(&path) {
            Ok(file) => file,
            Err(_) => return Ok(Vec::new()),
        };

        // Never read past what is there, and never allocate past it either:
        // the request may be for the whole rest of the file.
        let file_len = file
            .metadata()
            .context("reading a blob's file metadata")?
            .len();

        let Some(take) = file_len.checked_sub(offset) else {
            return Ok(Vec::new());
        };
        if take == 0 {
            return Ok(Vec::new());
        }

        let take = take.min(len) as usize;
        let mut buffer = vec![0u8; take];
        let mut filled = 0;

        file.seek(SeekFrom::Start(offset))
            .with_context(|| format!("seeking blob {path:?} to {offset}"))?;

        while filled < take {
            match file.read(&mut buffer[filled..]) {
                Ok(0) => break,
                Ok(n) => filled += n,
                Err(error) if error.kind() == ErrorKind::Interrupted => continue,
                Err(error) => return Err(error).context("reading a blob"),
            }
        }

        buffer.truncate(filled);

        Ok(buffer)
    }

    fn append(&self, sha256: &str, bytes: &[u8]) -> Result<()> {
        let path = self.path(sha256)?;

        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .with_context(|| format!("opening blob {path:?} for append"))?;

        file.write_all(bytes)
            .with_context(|| format!("appending to blob {path:?}"))?;

        Ok(())
    }

    fn delete(&self, sha256: &str) -> Result<()> {
        let path = self.path(sha256)?;

        // A missing file is a blob already absent, not an error to retry into.
        match fs::remove_file(&path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error).with_context(|| format!("deleting blob {path:?}")),
        }
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    /// A self-cleaning directory under the system temp dir, unique per test.
    struct TempDir(PathBuf);

    impl TempDir {
        fn new() -> Self {
            let dir = std::env::temp_dir().join(format!(
                "dip-blobs-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));

            fs::create_dir_all(&dir).unwrap();

            Self(dir)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn key(bytes: &[u8]) -> String {
        use sha2::{Digest, Sha256};

        hex::encode(Sha256::digest(bytes))
    }

    #[test]
    fn a_blob_round_trips_through_the_directory() {
        let dir = TempDir::new();
        let store = FileBlobStore::open(&dir.0).unwrap();
        let hash = key(b"the quick brown fox");

        assert!(!store.has(&hash).unwrap());
        assert_eq!(store.len(&hash).unwrap(), None);

        store.append(&hash, b"the quick ").unwrap();
        store.append(&hash, b"brown fox").unwrap();

        assert!(store.has(&hash).unwrap());
        assert_eq!(store.len(&hash).unwrap(), Some(19));
        assert_eq!(store.read(&hash, 0, 100).unwrap(), b"the quick brown fox");
        assert_eq!(store.read(&hash, 4, 5).unwrap(), b"quick");

        store.delete(&hash).unwrap();
        assert!(!store.has(&hash).unwrap());
        // Deleting again is a no-op, as on the memory store.
        store.delete(&hash).unwrap();
    }

    #[test]
    fn read_stops_at_the_end_of_the_file() {
        let dir = TempDir::new();
        let store = FileBlobStore::open(&dir.0).unwrap();
        let hash = key(b"short");

        store.append(&hash, b"short").unwrap();

        assert_eq!(store.read(&hash, 2, 100).unwrap(), b"ort");
        // A read wholly past the end is empty, not an error.
        assert_eq!(store.read(&hash, 100, 10).unwrap(), Vec::<u8>::new());
    }

    #[test]
    fn a_missing_blob_reads_empty() {
        let dir = TempDir::new();
        let store = FileBlobStore::open(&dir.0).unwrap();
        let hash = key(b"never written");

        assert_eq!(store.read(&hash, 0, 100).unwrap(), Vec::<u8>::new());
        assert_eq!(store.len(&hash).unwrap(), None);
    }

    #[test]
    fn a_hash_is_not_a_path() {
        let dir = TempDir::new();
        let store = FileBlobStore::open(&dir.0).unwrap();

        // Traversal and non-hex names are refused outright.
        assert!(store.append("../escape", b"x").is_err());
        assert!(store.append("../../etc/passwd", b"x").is_err());
        assert!(store.append("not-hex", b"x").is_err());
        assert!(store.append(&"ab".repeat(31), b"x").is_err());

        // Nothing escaped the directory.
        assert!(fs::read_dir(&dir.0).unwrap().next().is_none());
    }
}
