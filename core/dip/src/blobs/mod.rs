//! The blob bytes: read, appended, and deleted through a store.
//!
//! Metadata — hashes, permissions, progress — lives in SQLite; this is only
//! the file itself. The core ships a file-backed store over the directory the
//! shell provides, the same way it opens the SQLite store in one; the trait is
//! what lets tests run on memory and a shell slot in a custom backing (an
//! encrypted one, say) without touching the sync layer.
//! `docs/sync.md#blob-sync`.
//!
//! A store holds bytes for exactly as long as the `blob` table has a record of
//! the hash. [`Node`](crate::node::Node) is what enforces that: it deletes on
//! [`BlobChange::Removed`](crate::db::blob::channel::BlobChange::Removed), and
//! sweeps the store against the table for the removals a lossy channel or a
//! closed app never delivered.
//!
//! | Module | Holds |
//! | --- | --- |
//! | `trait` | [`BlobStore`], the interface the sync layer holds one through |
//! | [`mod@file`] | [`FileBlobStore`], the shipped store over a directory |
//! | [`mod@memory`] | [`MemoryBlobStore`], the one tests and tooling run on |
//! | [`mod@tree`] | [`GroupTree`], the BLAKE3 values a group verifies against |

pub mod file;
pub mod memory;
pub mod r#trait;
pub mod tree;

pub use file::FileBlobStore;
pub use memory::MemoryBlobStore;
pub use r#trait::BlobStore;
pub use tree::{GROUP_BYTES, GroupTree};

#[cfg(test)]
mod tests {
    use super::*;

    use crate::fixtures::TempDir;
    use crate::model::BlobHash;

    /// Both implementations, so a test can assert they answer alike.
    fn stores(dir: &TempDir) -> (FileBlobStore, MemoryBlobStore) {
        (
            FileBlobStore::open(&dir.0).unwrap(),
            MemoryBlobStore::default(),
        )
    }

    #[test]
    fn the_two_stores_answer_the_same_reads() {
        let dir = TempDir::new("blobs");
        let (file, memory) = stores(&dir);
        let hash = BlobHash::digest(b"short");

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

    /// What a sweep deletes is whatever this reports, so the two stores
    /// disagreeing here is a file that leaks on a phone and not in a test.
    #[test]
    fn the_two_stores_list_the_same_hashes() {
        let dir = TempDir::new("blobs");
        let (file, memory) = stores(&dir);
        let mut held = [BlobHash::digest(b"one"), BlobHash::digest(b"two")];
        held.sort();

        for hash in &held {
            file.append(hash, b"bytes").unwrap();
            memory.append(hash, b"bytes").unwrap();
        }

        let mut listed = file.hashes().unwrap();
        listed.sort();

        assert_eq!(listed, held);

        let mut from_memory = memory.hashes().unwrap();
        from_memory.sort();

        assert_eq!(from_memory, held);

        file.delete(&held[0]).unwrap();
        memory.delete(&held[0]).unwrap();

        assert_eq!(file.hashes().unwrap(), [held[1].clone()]);
        assert_eq!(memory.hashes().unwrap(), [held[1].clone()]);
    }

    /// The store names its own files, so anything else under the directory is
    /// the shell's and a sweep must leave it alone.
    #[test]
    fn a_file_that_is_not_a_hash_is_not_the_stores() {
        let dir = TempDir::new("blobs");
        let store = FileBlobStore::open(&dir.0).unwrap();

        std::fs::write(dir.0.join("README"), b"not ours").unwrap();

        assert!(store.hashes().unwrap().is_empty());
    }
}
