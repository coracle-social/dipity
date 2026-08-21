//! The interface the sync layer holds a blob store through.

use anyhow::Result;

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
