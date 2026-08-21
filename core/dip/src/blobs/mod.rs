//! The blob bytes: read, appended, and deleted through a store.
//!
//! Metadata — hashes, permissions, progress — lives in SQLite; this is only
//! the file itself. The core ships a file-backed store over the directory the
//! shell provides, the same way it opens the SQLite store in one; the trait is
//! what lets tests run on memory and a shell slot in a custom backing (an
//! encrypted one, say) without touching the sync layer.
//! `docs/sync.md#blob-sync`.
//!
//! | Module | Holds |
//! | --- | --- |
//! | `trait` | [`BlobStore`], the interface the sync layer holds one through |
//! | [`mod@file`] | [`FileBlobStore`], the shipped store over a directory |
//! | [`mod@memory`] | [`MemoryBlobStore`], the one tests and tooling run on |

pub mod file;
pub mod memory;
pub mod r#trait;

pub use file::FileBlobStore;
pub use memory::MemoryBlobStore;
pub use r#trait::BlobStore;

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
}
