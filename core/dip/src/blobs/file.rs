//! The store the app ships with: one file per blob under a directory.

use std::fs::{self, File, OpenOptions};
use std::io::{ErrorKind, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use crate::blobs::BlobStore;
use crate::model::BlobHash;

/// What an outboard tree's file is called, over the name of the content it
/// proves. Not 64 hex characters, so it is never mistaken for a blob by
/// [`hashes`](BlobStore::hashes) or by the sweep that reads it.
const OUTBOARD_SUFFIX: &str = ".obao";

/// A blob store over a directory the shell provides.
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

    /// The file a hash lives in.
    fn path(&self, sha256: &BlobHash) -> PathBuf {
        self.directory.join(sha256.as_str())
    }

    /// The file the outboard tree over that hash lives in.
    fn outboard_path(&self, sha256: &BlobHash) -> PathBuf {
        self.directory
            .join(format!("{}{OUTBOARD_SUFFIX}", sha256.as_str()))
    }

    /// The length of a file this store wrote, or `None` if it wrote none.
    fn file_len(path: &Path) -> Option<u64> {
        match fs::metadata(path) {
            Ok(metadata) if metadata.is_file() => Some(metadata.len()),
            _ => None,
        }
    }

    /// Read `len` bytes from `offset` of one of this store's files.
    fn read_at(path: &Path, offset: u64, len: u64) -> Result<Vec<u8>> {
        let mut file = match File::open(path) {
            Ok(file) => file,
            Err(_) => return Ok(Vec::new()),
        };

        // Never read past what is there, and never allocate past it either.
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
}

impl BlobStore for FileBlobStore {
    fn has(&self, sha256: &BlobHash) -> Result<bool> {
        Ok(self.path(sha256).is_file())
    }

    fn len(&self, sha256: &BlobHash) -> Result<Option<u64>> {
        Ok(Self::file_len(&self.path(sha256)))
    }

    fn read(&self, sha256: &BlobHash, offset: u64, len: u64) -> Result<Vec<u8>> {
        Self::read_at(&self.path(sha256), offset, len)
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
        // A missing file is a blob already absent, not an error to retry into.
        for path in [self.path(sha256), self.outboard_path(sha256)] {
            match fs::remove_file(&path) {
                Ok(()) => {}
                Err(error) if error.kind() == ErrorKind::NotFound => {}
                Err(error) => return Err(error).with_context(|| format!("deleting blob {path:?}")),
            }
        }

        Ok(())
    }

    fn hashes(&self) -> Result<Vec<BlobHash>> {
        let entries = fs::read_dir(&self.directory).with_context(|| {
            format!("listing blob store directory {}", self.directory.display())
        })?;

        let mut hashes = Vec::new();

        for entry in entries {
            let name = entry
                .context("reading a blob store directory entry")?
                .file_name();

            // A name that is not a hash was not written by `append`, so it is not the store's.
            if let Some(hash) = name.to_str().and_then(|name| BlobHash::parse(name).ok()) {
                hashes.push(hash);
            }
        }

        Ok(hashes)
    }

    fn outboard_len(&self, sha256: &BlobHash) -> Result<Option<u64>> {
        Ok(Self::file_len(&self.outboard_path(sha256)))
    }

    fn read_outboard(&self, sha256: &BlobHash, offset: u64, len: u64) -> Result<Vec<u8>> {
        Self::read_at(&self.outboard_path(sha256), offset, len)
    }

    fn write_outboard(&self, sha256: &BlobHash, bytes: &[u8]) -> Result<()> {
        let path = self.outboard_path(sha256);

        fs::write(&path, bytes).with_context(|| format!("writing outboard {path:?}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::fixtures::TempDir;

    fn key(bytes: &[u8]) -> BlobHash {
        BlobHash::digest(bytes)
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
    fn a_hash_is_not_a_path() {
        let dir = TempDir::new("blobs");
        let store = FileBlobStore::open(&dir.0).unwrap();

        // Traversal and non-hex names never become a hash: the check is the type, not a guard.
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
