//! BLAKE3 verified streaming over a blob's bytes, through the `bao` crate.
//!
//! A blob's address is the SHA-256 of the whole file, so it says nothing until
//! the last byte lands: a peer can feed a transfer rubbish for a megabyte and
//! only be caught at the end, and a transfer that drops has no trustworthy
//! prefix to resume from. The BLAKE3 root the `imeta` tag carries
//! (`docs/nips/imeta-blake3.md`) is a merkle root instead, so any range of the
//! file can be proved against it on its own.
//!
//! Bao is that proof format. A serving device keeps an outboard tree beside
//! the content — the merkle nodes, with the bytes left where they are — and
//! answers a range with a [slice](slice): the subtree hashes on the path to
//! those bytes, then the bytes. A fetching device [verifies](verify) the slice
//! against the root alone, which the event id already commits to, so nothing
//! about the tree has to be trusted or fetched first.
//!
//! Verification is per [`CHUNK_BYTES`], and a group of bytes is a whole number
//! of chunks, so every byte that reaches the disk has been checked and the
//! bytes already there are where the next session resumes.

use std::io::{self, Cursor, Read, Seek, SeekFrom};

use anyhow::{Context, Result, anyhow};
use bao::decode::SliceDecoder;
use bao::encode::{Encoder, SliceExtractor};

use crate::blobs::BlobStore;
use crate::model::BlobHash;

/// The BLAKE3 root of a whole file: what an `imeta` tag carries, and the only
/// thing a slice is checked against.
pub type Root = bao::Hash;

/// The bytes one BLAKE3 chunk covers, which is the granularity a slice proves
/// and the alignment a range is worth asking on.
///
/// Bao's own, and not ours to pick: the tree is defined over 1 KiB chunks, so
/// a range that starts inside one pays for the whole chunk anyway.
pub const CHUNK_BYTES: u64 = 1024;

/// A root as an `imeta` tag spells it.
pub fn root(value: &str) -> Result<Root> {
    Root::from_hex(value).map_err(|_| anyhow!("{value} is not a BLAKE3 root"))
}

/// Build the outboard tree over the bytes the store holds, returning the root
/// they hash to.
///
/// The tree is derived from the content, so it is written rather than fetched:
/// a device that has the whole file can always compute it, and one that cannot
/// serves the file unverified instead.
pub fn build(store: &dyn BlobStore, sha256: &BlobHash) -> Result<Root> {
    let mut encoder = Encoder::new_outboard(Cursor::new(Vec::new()));

    io::copy(&mut StoreCursor::content(store, sha256), &mut encoder)
        .with_context(|| format!("hashing blob {sha256} into an outboard tree"))?;

    let root = encoder
        .finalize()
        .with_context(|| format!("finalizing the outboard tree over blob {sha256}"))?;

    store.write_outboard(sha256, &encoder.into_inner().into_inner())?;

    Ok(root)
}

/// The slice proving `len` bytes from `start`, for a peer to check against the
/// root.
///
/// It is larger than the bytes it carries — one 64-byte parent node per level
/// of the path down to them — which is what the fetcher meters and the sender
/// does not charge for.
pub fn slice(store: &dyn BlobStore, sha256: &BlobHash, start: u64, len: u64) -> Result<Vec<u8>> {
    let mut extractor = SliceExtractor::new_outboard(
        StoreCursor::content(store, sha256),
        StoreCursor::outboard(store, sha256),
        start,
        len,
    );
    let mut slice = Vec::new();

    extractor
        .read_to_end(&mut slice)
        .with_context(|| format!("extracting {len} bytes from {start} of blob {sha256}"))?;

    Ok(slice)
}

/// The content bytes a slice carries, once the slice verifies against `root`.
///
/// An error is a peer that sent bytes belonging to some other file, or none:
/// the decoder checks every chunk against the tree on the way past, so there
/// is no unverified byte in the answer.
pub fn verify(slice: &[u8], root: &Root, start: u64, len: u64) -> Result<Vec<u8>> {
    let mut decoder = SliceDecoder::new(slice, root, start, len);
    let mut content = Vec::new();

    decoder
        .read_to_end(&mut content)
        .context("verifying a blob slice against its BLAKE3 root")?;

    Ok(content)
}

/// A `Read + Seek` view of one of a blob's two files.
///
/// The store answers ranged reads rather than handing out file handles, since
/// an implementation may hold bytes anywhere; bao wants a reader it can seek.
/// Every read is one call into the store, and bao makes them a chunk at a
/// time, so a slice costs a handful of small reads rather than the whole file
/// in memory.
struct StoreCursor<'a> {
    /// Where the bytes are.
    store: &'a dyn BlobStore,
    /// Which blob's.
    sha256: BlobHash,
    /// Whether this reads the outboard tree rather than the content.
    outboard: bool,
    /// The offset the next read starts at.
    position: u64,
}

impl<'a> StoreCursor<'a> {
    /// A cursor over the blob's content.
    fn content(store: &'a dyn BlobStore, sha256: &BlobHash) -> Self {
        Self {
            store,
            sha256: sha256.clone(),
            outboard: false,
            position: 0,
        }
    }

    /// A cursor over the outboard tree beside it.
    fn outboard(store: &'a dyn BlobStore, sha256: &BlobHash) -> Self {
        Self {
            store,
            sha256: sha256.clone(),
            outboard: true,
            position: 0,
        }
    }

    /// The length of the file this cursor reads, zero when it is not held.
    fn len(&self) -> io::Result<u64> {
        let held = match self.outboard {
            true => self.store.outboard_len(&self.sha256),
            false => self.store.len(&self.sha256),
        };

        held.map(Option::unwrap_or_default).map_err(io_error)
    }
}

impl Read for StoreCursor<'_> {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        let wanted = buffer.len() as u64;
        let bytes = match self.outboard {
            true => self
                .store
                .read_outboard(&self.sha256, self.position, wanted),
            false => self.store.read(&self.sha256, self.position, wanted),
        }
        .map_err(io_error)?;

        buffer[..bytes.len()].copy_from_slice(&bytes);
        self.position += bytes.len() as u64;

        Ok(bytes.len())
    }
}

impl Seek for StoreCursor<'_> {
    fn seek(&mut self, to: SeekFrom) -> io::Result<u64> {
        // A seek past the end is legal and reads empty, which is how the
        // store answers an offset past the end of the file anyway.
        self.position = match to {
            SeekFrom::Start(offset) => offset,
            SeekFrom::Current(offset) => self.position.saturating_add_signed(offset),
            SeekFrom::End(offset) => self.len()?.saturating_add_signed(offset),
        };

        Ok(self.position)
    }
}

/// A store error as the reader traits carry it.
fn io_error(error: anyhow::Error) -> io::Error {
    io::Error::other(format!("{error:#}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::blobs::MemoryBlobStore;

    /// Big enough to span several chunks, so a slice has a real proof in it.
    fn blob(store: &MemoryBlobStore, len: usize) -> (BlobHash, Vec<u8>) {
        let bytes: Vec<u8> = (0..len).map(|index| (index % 251) as u8).collect();
        let hash = BlobHash::digest(&bytes);

        store.append(&hash, &bytes).unwrap();

        (hash, bytes)
    }

    #[test]
    fn a_slice_verifies_against_the_root_and_yields_its_bytes() {
        let store = MemoryBlobStore::default();
        let (hash, bytes) = blob(&store, 40 * 1024);
        let root = build(&store, &hash).unwrap();

        // The root is BLAKE3 over the whole file, whatever built it.
        assert_eq!(root, blake3_root(&bytes));

        for (start, len) in [
            (0, 16 * 1024),
            (16 * 1024, 16 * 1024),
            (32 * 1024, 8 * 1024),
        ] {
            let proof = slice(&store, &hash, start, len).unwrap();
            let verified = verify(&proof, &root, start, len).unwrap();

            assert_eq!(verified, bytes[start as usize..(start + len) as usize]);
            assert!(proof.len() > verified.len(), "a slice carries its proof");
        }
    }

    #[test]
    fn a_tampered_slice_does_not_verify() {
        let store = MemoryBlobStore::default();
        let (hash, _) = blob(&store, 40 * 1024);
        let root = build(&store, &hash).unwrap();
        let mut proof = slice(&store, &hash, 0, 16 * 1024).unwrap();
        let last = proof.len() - 1;

        proof[last] ^= 1;

        assert!(verify(&proof, &root, 0, 16 * 1024).is_err());
    }

    /// The proof for one file does not verify against another's root, which is
    /// what stops a peer answering with bytes it holds for something else.
    #[test]
    fn a_slice_of_another_blob_does_not_verify() {
        let store = MemoryBlobStore::default();
        let (hash, _) = blob(&store, 40 * 1024);
        let root = build(&store, &hash).unwrap();

        let other = MemoryBlobStore::default();
        let (other_hash, _) = blob(&other, 40 * 1024 + 1);
        build(&other, &other_hash).unwrap();

        let proof = slice(&other, &other_hash, 0, 16 * 1024).unwrap();

        assert!(verify(&proof, &root, 0, 16 * 1024).is_err());
    }

    /// A blob shorter than one chunk still has a root and a slice; the tree is
    /// a single chunk and the proof is the bytes.
    #[test]
    fn a_blob_smaller_than_a_chunk_still_verifies() {
        let store = MemoryBlobStore::default();
        let (hash, bytes) = blob(&store, 10);
        let root = build(&store, &hash).unwrap();
        let proof = slice(&store, &hash, 0, CHUNK_BYTES).unwrap();

        assert_eq!(verify(&proof, &root, 0, CHUNK_BYTES).unwrap(), bytes);
    }

    /// What the outboard costs to keep beside the content.
    #[test]
    fn the_outboard_is_a_fraction_of_the_content() {
        let store = MemoryBlobStore::default();
        let (hash, bytes) = blob(&store, 256 * 1024);

        build(&store, &hash).unwrap();

        let outboard = store.outboard_len(&hash).unwrap().unwrap();

        assert!(
            outboard < bytes.len() as u64 / 10,
            "an outboard of {outboard} bytes over {} is not worth keeping",
            bytes.len()
        );
    }

    fn blake3_root(bytes: &[u8]) -> Root {
        let (_, root) = bao::encode::outboard(bytes);

        root
    }
}
