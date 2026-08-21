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

use crate::model::BlobHash;

/// Where blob bytes go.
///
/// Every method is keyed on a [`BlobHash`], so an implementation names a file
/// by it without a check of its own: a hash that could reach outside the
/// store's directory does not parse in the first place.
pub trait BlobStore: Send + Sync {
    /// Whether the blob's bytes are held.
    fn has(&self, sha256: &BlobHash) -> Result<bool>;
    /// The length of the held blob, or `None` if it is not held.
    fn len(&self, sha256: &BlobHash) -> Result<Option<u64>>;
    /// Read `len` bytes from `offset`, or as many as are there. A read past
    /// the end is empty rather than an error, and `len` may name the whole
    /// rest of the file.
    fn read(&self, sha256: &BlobHash, offset: u64, len: u64) -> Result<Vec<u8>>;
    /// Append bytes to the blob, creating it if missing.
    fn append(&self, sha256: &BlobHash, bytes: &[u8]) -> Result<()>;
    /// Delete the blob's bytes, if any are held.
    fn delete(&self, sha256: &BlobHash) -> Result<()>;
}

/// A blob store over a directory the shell provides, one file per blob named
/// by its lowercase hex sha256.
///
/// Blobs are content-addressed and appended group by group as they arrive, so
/// the whole transfer is `create` + `append` until the sync layer has the
/// whole file and checks it against its hash.
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

    /// The file a hash lives in. One path component, never a traversal: a
    /// [`BlobHash`] is 64 hex characters or it does not exist.
    fn path(&self, sha256: &BlobHash) -> PathBuf {
        self.directory.join(sha256.as_str())
    }
}

impl BlobStore for FileBlobStore {
    fn has(&self, sha256: &BlobHash) -> Result<bool> {
        Ok(self.path(sha256).is_file())
    }

    fn len(&self, sha256: &BlobHash) -> Result<Option<u64>> {
        match fs::metadata(self.path(sha256)) {
            Ok(metadata) if metadata.is_file() => Ok(Some(metadata.len())),
            _ => Ok(None),
        }
    }

    fn read(&self, sha256: &BlobHash, offset: u64, len: u64) -> Result<Vec<u8>> {
        let path = self.path(sha256);
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

        let take = usize::try_from(take.min(len)).unwrap_or(usize::MAX);
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

    fn append(&self, sha256: &BlobHash, bytes: &[u8]) -> Result<()> {
        let path = self.path(sha256);

        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .with_context(|| format!("opening blob {path:?} for append"))?;

        file.write_all(bytes)
            .with_context(|| format!("appending to blob {path:?}"))?;

        Ok(())
    }

    fn delete(&self, sha256: &BlobHash) -> Result<()> {
        let path = self.path(sha256);

        // A missing file is a blob already absent, not an error to retry into.
        match fs::remove_file(&path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error).with_context(|| format!("deleting blob {path:?}")),
        }
    }
}

/// An in-memory store, for tests and tooling.
///
/// Answers exactly what [`FileBlobStore`] answers, out-of-range reads
/// included: the sync layer is tested against this one and shipped against
/// that one, so a difference between them is a bug that only appears on a
/// phone.
#[derive(Debug, Default)]
pub struct MemoryBlobStore {
    blobs: Mutex<HashMap<BlobHash, Vec<u8>>>,
}

impl BlobStore for MemoryBlobStore {
    fn has(&self, sha256: &BlobHash) -> Result<bool> {
        Ok(self.blobs.lock().unwrap().contains_key(sha256))
    }

    fn len(&self, sha256: &BlobHash) -> Result<Option<u64>> {
        Ok(self
            .blobs
            .lock()
            .unwrap()
            .get(sha256)
            .map(|bytes| bytes.len() as u64))
    }

    fn read(&self, sha256: &BlobHash, offset: u64, len: u64) -> Result<Vec<u8>> {
        let blobs = self.blobs.lock().unwrap();
        let Some(bytes) = blobs.get(sha256) else {
            return Ok(Vec::new());
        };

        // Saturating throughout: a caller asking for the rest of the file
        // passes `u64::MAX`, and adding that to an offset is an overflow.
        let start = usize::try_from(offset)
            .unwrap_or(usize::MAX)
            .min(bytes.len());
        let take = usize::try_from(len)
            .unwrap_or(usize::MAX)
            .min(bytes.len() - start);

        Ok(bytes[start..start + take].to_vec())
    }

    fn append(&self, sha256: &BlobHash, bytes: &[u8]) -> Result<()> {
        self.blobs
            .lock()
            .unwrap()
            .entry(sha256.clone())
            .or_default()
            .extend_from_slice(bytes);

        Ok(())
    }

    fn delete(&self, sha256: &BlobHash) -> Result<()> {
        self.blobs.lock().unwrap().remove(sha256);

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::fixtures::TempDir;

    fn key(bytes: &[u8]) -> BlobHash {
        BlobHash::digest(bytes)
    }

    /// Both implementations, so a test can assert they answer alike.
    fn stores(dir: &TempDir) -> (FileBlobStore, MemoryBlobStore) {
        (
            FileBlobStore::open(&dir.0).unwrap(),
            MemoryBlobStore::default(),
        )
    }

    #[test]
    fn a_blob_round_trips_through_the_directory() {
        let dir = TempDir::new("blobs");
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
        let dir = TempDir::new("blobs");
        let store = FileBlobStore::open(&dir.0).unwrap();
        let hash = key(b"short");

        store.append(&hash, b"short").unwrap();

        assert_eq!(store.read(&hash, 2, 100).unwrap(), b"ort");
        // A read wholly past the end is empty, not an error.
        assert_eq!(store.read(&hash, 100, 10).unwrap(), Vec::<u8>::new());
    }

    #[test]
    fn a_missing_blob_reads_empty() {
        let dir = TempDir::new("blobs");
        let store = FileBlobStore::open(&dir.0).unwrap();
        let hash = key(b"never written");

        assert_eq!(store.read(&hash, 0, 100).unwrap(), Vec::<u8>::new());
        assert_eq!(store.len(&hash).unwrap(), None);
    }

    #[test]
    fn the_two_stores_answer_the_same_reads() {
        let dir = TempDir::new("blobs");
        let (file, memory) = stores(&dir);
        let hash = key(b"short");

        file.append(&hash, b"short").unwrap();
        memory.append(&hash, b"short").unwrap();

        // `u64::MAX` is what a rangeless GET asks for, and it is where an
        // `offset + len` in either implementation would overflow.
        for (offset, len) in [
            (0, u64::MAX),
            (2, u64::MAX),
            (0, 100),
            (4, 5),
            (5, 10),
            (100, 10),
            (u64::MAX, u64::MAX),
        ] {
            assert_eq!(
                file.read(&hash, offset, len).unwrap(),
                memory.read(&hash, offset, len).unwrap(),
                "the stores disagree on {offset}+{len}"
            );
        }

        assert_eq!(file.read(&hash, 0, u64::MAX).unwrap(), b"short");
        assert_eq!(file.len(&hash).unwrap(), memory.len(&hash).unwrap());
    }

    #[test]
    fn a_hash_is_not_a_path() {
        let dir = TempDir::new("blobs");
        let store = FileBlobStore::open(&dir.0).unwrap();

        // Traversal and non-hex names never become a hash, so the store cannot
        // be handed one: the check is the type, not a guard inside `path`.
        for name in ["../escape", "../../etc/passwd", "not-hex", &"ab".repeat(31)] {
            assert!(BlobHash::parse(name).is_err(), "{name} parsed as a hash");
        }

        // Every path the store can build is one component under its directory.
        assert_eq!(
            store.path(&key(b"anything")).parent(),
            Some(dir.0.as_path())
        );
        assert!(fs::read_dir(&dir.0).unwrap().next().is_none());
    }
}
