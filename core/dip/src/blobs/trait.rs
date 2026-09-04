//! The interface the sync layer holds a blob store through.

use anyhow::Result;

use crate::model::BlobHash;

/// Where blob bytes go.
///
/// Every method is keyed on a [`BlobHash`], so an implementation names a file
/// by it without a check of its own: a hash that could reach outside the
/// store's directory does not parse in the first place.
///
/// A blob has two files: its content, and the outboard BLAKE3 tree a serving
/// device proves ranges of that content with
/// ([`blobs::verified`](crate::blobs::verified)). They are addressed by the
/// same hash and deleted together, but only the content is a blob as far as
/// [`hashes`](BlobStore::hashes) and the sweep that reads it are concerned.
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
    /// Delete the blob's bytes, and the outboard tree over them.
    fn delete(&self, sha256: &BlobHash) -> Result<()>;
    /// Every hash the store holds bytes for.
    ///
    /// The store is a cache of what the `blob` table records, so this is what
    /// a sweep compares the table against to find bytes nothing references.
    fn hashes(&self) -> Result<Vec<BlobHash>>;
    /// The length of the held outboard tree, or `None` if there is none.
    fn outboard_len(&self, sha256: &BlobHash) -> Result<Option<u64>>;
    /// Read `len` bytes of the outboard tree from `offset`, the same way
    /// [`read`](BlobStore::read) reads content.
    fn read_outboard(&self, sha256: &BlobHash, offset: u64, len: u64) -> Result<Vec<u8>>;
    /// Replace the blob's outboard tree.
    fn write_outboard(&self, sha256: &BlobHash, bytes: &[u8]) -> Result<()>;
}
