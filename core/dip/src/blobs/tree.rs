//! The BLAKE3 group tree over a blob's bytes.
//!
//! A blob moves one [`GROUP_BYTES`] group at a time, and a group is a
//! left-aligned BLAKE3 subtree, so it has a chaining value of its own. Those
//! chaining values fold back into the root the `imeta` tag carries
//! (`docs/nips/imeta-blake3.md`), which is what a fetcher checks the list
//! against before trusting any of it — the event id commits to the root, so a
//! list that folds to it is the author's and a forwarder cannot substitute
//! one. From there a group verifies on arrival against its own value: a bad
//! group costs one group rather than the whole file, and a transfer that drops
//! resumes from the last group that verified. `docs/sync.md#blob-sync`.
//!
//! The whole list travels once per transfer rather than a proof per group. It
//! is a thirty-second of a percent of the blob either way, and a list the
//! receiver holds outright verifies out of order and needs no re-sending when
//! a transfer is picked up again.
//!
//! A blob of one group or less has no tree: its only subtree is the root, so
//! there is nothing to fold and nothing to resume within it.

use anyhow::{Result, ensure};
use blake3::Hasher;
use blake3::hazmat::{HasherExt, Mode, merge_subtrees_non_root, merge_subtrees_root};

/// The bytes one `GET` asks for, and the bytes one chaining value covers.
pub const GROUP_BYTES: u64 = 16 * 1024;

/// A chaining value, as it is held and as it travels.
const CV_BYTES: usize = 32;

/// One blob's groups, each as the BLAKE3 chaining value over its bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GroupTree {
    /// One per group, in order; the last covers a partial group.
    cvs: Vec<[u8; CV_BYTES]>,
    /// The whole blob's length, which is what gives the last group its size.
    total: u64,
}

impl GroupTree {
    /// The tree over a whole blob's bytes.
    pub fn build(bytes: &[u8]) -> Result<Self> {
        let total = bytes.len() as u64;

        ensure!(spans_groups(total), "a blob of {total} bytes has no tree");

        let cvs = bytes
            .chunks(GROUP_BYTES as usize)
            .enumerate()
            .map(|(index, group)| group_cv(index as u64, group))
            .collect();

        Ok(Self { cvs, total })
    }

    /// The tree a peer sent, over a blob whose length is already known.
    pub fn decode(encoded: &[u8], total: u64) -> Result<Self> {
        ensure!(spans_groups(total), "a blob of {total} bytes has no tree");

        let groups = usize::try_from(group_count(total)).unwrap_or(usize::MAX);

        ensure!(
            encoded.len() == groups * CV_BYTES,
            "a tree of {} bytes does not cover {groups} groups",
            encoded.len()
        );

        let cvs = encoded
            .chunks_exact(CV_BYTES)
            .map(|cv| cv.try_into().expect("a chaining value's width"))
            .collect();

        Ok(Self { cvs, total })
    }

    /// The tree as it travels: every chaining value end to end.
    #[must_use]
    pub fn encode(&self) -> Vec<u8> {
        self.cvs.concat()
    }

    /// The BLAKE3 root the whole tree folds to, lowercase hex.
    #[must_use]
    pub fn root(&self) -> String {
        let split = left_split(self.cvs.len());
        let left = fold(&self.cvs[..split]);
        let right = fold(&self.cvs[split..]);

        hex::encode(merge_subtrees_root(&left, &right, Mode::Hash).as_bytes())
    }

    /// Whether `bytes` are the group at `index`, whole and unaltered.
    #[must_use]
    pub fn accepts(&self, index: u64, bytes: &[u8]) -> bool {
        let Some(expected) = self.cvs.get(usize::try_from(index).unwrap_or(usize::MAX)) else {
            return false;
        };

        // A short group is refused here rather than appended and never finished.
        bytes.len() as u64 == self.group_len(index) && group_cv(index, bytes) == *expected
    }

    /// How long the group at `index` is, the last one being the remainder.
    #[must_use]
    pub fn group_len(&self, index: u64) -> u64 {
        self.total
            .saturating_sub(index.saturating_mul(GROUP_BYTES))
            .min(GROUP_BYTES)
    }
}

/// Whether a blob of `total` bytes spans enough groups to have a tree.
#[must_use]
pub fn spans_groups(total: u64) -> bool {
    total > GROUP_BYTES
}

/// How many groups a blob of `total` bytes is fetched in.
#[must_use]
pub fn group_count(total: u64) -> u64 {
    total.div_ceil(GROUP_BYTES)
}

/// The chaining value of the group at `index`, over exactly its bytes.
fn group_cv(index: u64, bytes: &[u8]) -> [u8; CV_BYTES] {
    let mut hasher = Hasher::new();

    hasher.set_input_offset(index * GROUP_BYTES);
    hasher.update(bytes);

    hasher.finalize_non_root()
}

/// The chaining value of the subtree `cvs` covers, which is never the root.
fn fold(cvs: &[[u8; CV_BYTES]]) -> [u8; CV_BYTES] {
    if cvs.len() == 1 {
        return cvs[0];
    }

    let split = left_split(cvs.len());

    merge_subtrees_non_root(&fold(&cvs[..split]), &fold(&cvs[split..]), Mode::Hash)
}

/// Where BLAKE3 splits `groups` groups: the largest power of two below them.
fn left_split(groups: usize) -> usize {
    1 << (usize::BITS - 1 - (groups - 1).leading_zeros())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `len` bytes of something that is not all one byte.
    fn bytes(len: usize) -> Vec<u8> {
        (0..len).map(|index| (index % 251) as u8).collect()
    }

    /// The fold has to land on BLAKE3's own root at every shape of tree.
    #[test]
    fn the_tree_folds_to_the_files_own_root() {
        let group = GROUP_BYTES as usize;

        for len in [
            group + 1,
            2 * group,
            2 * group + 7,
            3 * group,
            4 * group,
            5 * group,
            7 * group - 1,
            8 * group,
            9 * group + 3,
            17 * group,
        ] {
            let bytes = bytes(len);
            let tree = GroupTree::build(&bytes).unwrap();

            assert_eq!(
                tree.root(),
                blake3::hash(&bytes).to_hex().to_string(),
                "the tree over {len} bytes folds to the wrong root"
            );
        }
    }

    #[test]
    fn a_blob_of_one_group_or_less_has_no_tree() {
        assert!(GroupTree::build(&bytes(GROUP_BYTES as usize)).is_err());
        assert!(!spans_groups(GROUP_BYTES));
        assert!(spans_groups(GROUP_BYTES + 1));
    }

    #[test]
    fn a_tree_round_trips_through_the_wire() {
        let bytes = bytes(5 * GROUP_BYTES as usize + 11);
        let total = bytes.len() as u64;
        let tree = GroupTree::build(&bytes).unwrap();
        let encoded = tree.encode();

        assert_eq!(encoded.len() as u64, group_count(total) * CV_BYTES as u64);
        assert_eq!(GroupTree::decode(&encoded, total).unwrap(), tree);

        // A tree that does not cover the declared length is refused.
        assert!(GroupTree::decode(&encoded, total + GROUP_BYTES).is_err());
        assert!(GroupTree::decode(&encoded[..31], total).is_err());
    }

    /// One bad byte anywhere in a group is what the per-group check is for.
    #[test]
    fn a_group_verifies_against_its_own_chaining_value() {
        let group = GROUP_BYTES as usize;
        let bytes = bytes(3 * group + 5);
        let tree = GroupTree::build(&bytes).unwrap();

        for index in 0..4u64 {
            let start = index as usize * group;
            let end = (start + group).min(bytes.len());

            assert!(tree.accepts(index, &bytes[start..end]));
        }

        let mut altered = bytes[..group].to_vec();
        altered[7] ^= 1;

        assert!(!tree.accepts(0, &altered));
        // The offset is part of what a chaining value covers.
        assert!(!tree.accepts(1, &bytes[..group]));
        assert!(!tree.accepts(0, &bytes[..group - 1]));
        assert!(!tree.accepts(4, &bytes[..group]));
    }

    /// A substituted chaining value is what the root check has to catch.
    #[test]
    fn an_altered_tree_folds_to_a_different_root() {
        let bytes = bytes(4 * GROUP_BYTES as usize);
        let total = bytes.len() as u64;
        let tree = GroupTree::build(&bytes).unwrap();
        let mut encoded = tree.encode();

        encoded[70] ^= 1;

        assert_ne!(
            GroupTree::decode(&encoded, total).unwrap().root(),
            tree.root()
        );
    }
}
