//! The store tests and tooling run on: blobs in a map.

use std::collections::HashMap;
use std::sync::Mutex;

use anyhow::Result;

use crate::blobs::BlobStore;
use crate::model::BlobHash;

/// An in-memory store, for tests and tooling.
///
/// Answers exactly what [`FileBlobStore`] answers, out-of-range reads
/// included: the sync layer is tested against this one and shipped against
/// that one, so a difference between them is a bug that only appears on a
/// phone.
///
/// [`FileBlobStore`]: crate::blobs::FileBlobStore
#[derive(Debug, Default)]
pub struct MemoryBlobStore {
    blobs: Mutex<HashMap<BlobHash, Vec<u8>>>,
    outboards: Mutex<HashMap<BlobHash, Vec<u8>>>,
}

impl MemoryBlobStore {
    /// Read a range out of one of the two maps, the way the file store reads
    /// a range out of one of its two files.
    fn read_from(
        map: &Mutex<HashMap<BlobHash, Vec<u8>>>,
        sha256: &BlobHash,
        offset: u64,
        len: u64,
    ) -> Vec<u8> {
        let held = map.lock().unwrap();
        let Some(bytes) = held.get(sha256) else {
            return Vec::new();
        };

        // Saturating throughout: a caller asking for the rest of the file
        // passes `u64::MAX`, and adding that to an offset is an overflow.
        let start = usize::try_from(offset)
            .unwrap_or(usize::MAX)
            .min(bytes.len());
        let take = usize::try_from(len)
            .unwrap_or(usize::MAX)
            .min(bytes.len() - start);

        bytes[start..start + take].to_vec()
    }
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
        Ok(Self::read_from(&self.blobs, sha256, offset, len))
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
        self.outboards.lock().unwrap().remove(sha256);

        Ok(())
    }

    fn hashes(&self) -> Result<Vec<BlobHash>> {
        Ok(self.blobs.lock().unwrap().keys().cloned().collect())
    }

    fn outboard_len(&self, sha256: &BlobHash) -> Result<Option<u64>> {
        Ok(self
            .outboards
            .lock()
            .unwrap()
            .get(sha256)
            .map(|bytes| bytes.len() as u64))
    }

    fn read_outboard(&self, sha256: &BlobHash, offset: u64, len: u64) -> Result<Vec<u8>> {
        Ok(Self::read_from(&self.outboards, sha256, offset, len))
    }

    fn write_outboard(&self, sha256: &BlobHash, bytes: &[u8]) -> Result<()> {
        self.outboards
            .lock()
            .unwrap()
            .insert(sha256.clone(), bytes.to_vec());

        Ok(())
    }
}
