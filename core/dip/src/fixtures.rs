//! Events, keys and a two-node harness for tests, shared by every model and
//! every table that hangs off `event`.
//!
//! Keys are real points on the curve, because `PublicKey` will not hold
//! anything else, and ids are real hashes, because building an event through
//! the pipeline computes one. Both are derived from a seed, which makes a
//! fixture reproducible and keeps two seeds from colliding.

use std::collections::VecDeque;
use std::sync::Arc;

use coracle_lib::events::{EventContent, EventId, HashedEvent};
use coracle_lib::keys::{PublicKey, SecretKey};
use coracle_lib::tags::Tags;

use crate::keys::KeyCustody;
use crate::node::{Action, Node};

/// The key for a seed. Every seed below 255 is a valid scalar.
pub(crate) fn secret(seed: u8) -> SecretKey {
    SecretKey::from_hex(&hex::encode([seed; 32])).expect("seed is a valid secret key")
}

/// A key in hand, as the custody a node or a session takes.
pub(crate) fn custody(key: SecretKey) -> Arc<dyn KeyCustody> {
    Arc::new(key)
}

/// The pubkey for a seed.
pub(crate) fn author(seed: u8) -> PublicKey {
    secret(seed).public_key()
}

/// The peer a fixture is seen from, when the test does not care which.
pub(crate) fn peer() -> PublicKey {
    author(200)
}

/// A blob hash for a seed. Real hex, because [`BlobHash`] holds nothing else,
/// and ordered by the seed, because the want list and eviction sort on it.
pub(crate) fn blob_hash(seed: u8) -> crate::model::BlobHash {
    crate::model::BlobHash::parse(&hex::encode([seed; 32])).expect("a seed is 32 bytes of hex")
}

/// A kind 1 note.
pub(crate) fn note(author: PublicKey, created_at: i64, content: &str, tags: Tags) -> HashedEvent {
    event(author, 1, created_at, content, tags)
}

/// An event of any kind, hashed through the pipeline so its id is real.
pub(crate) fn event(
    author: PublicKey,
    kind: u16,
    created_at: i64,
    content: &str,
    tags: Tags,
) -> HashedEvent {
    EventContent::new()
        .with_content(content)
        .with_tags(tags)
        .with_kind(kind)
        .with_created_at(created_at)
        .with_pubkey(author)
        .with_id()
}

/// An event's id. Trivial now that the id is a type — kept so a fixture reads
/// the same as it did when the store keyed on hex.
pub(crate) fn id(event: &HashedEvent) -> EventId {
    event.id
}

/// Deliver the writes in `actions` and everything the two nodes then say to
/// each other, answering with what is left over.
///
/// This is the shell the `node` module's header describes: two nodes handing
/// each other bytes, with nothing between them.
pub(crate) fn settle(from: &mut Node, into: &mut Node, actions: Vec<Action>) -> Vec<Action> {
    let mut rest = Vec::new();
    let mut queued: VecDeque<(bool, Action)> =
        actions.into_iter().map(|action| (true, action)).collect();

    // One queue keeps each node's own fragments in order, which the cipher requires.
    while let Some((from_side, action)) = queued.pop_front() {
        let Action::Send(link, write) = action else {
            rest.push(action);
            continue;
        };

        // The shell does both, because acknowledging is what releases the sender's next fragment.
        let (answered, released) = if from_side {
            (into.bytes_received(link, &write), from.write_complete(link))
        } else {
            (from.bytes_received(link, &write), into.write_complete(link))
        };

        queued.extend(answered.into_iter().map(|action| (!from_side, action)));
        queued.extend(released.into_iter().map(|action| (from_side, action)));
    }

    rest
}

/// A directory under the system temp dir, removed when it goes out of scope.
///
/// The name carries a process-wide counter as well as the clock, because tests
/// run in parallel and two of them reading the clock in the same tick would
/// otherwise share a directory — and a shared directory makes whichever test
/// asserts on the directory's contents fail, seemingly at random.
pub(crate) struct TempDir(pub std::path::PathBuf);

impl TempDir {
    pub(crate) fn new(label: &str) -> Self {
        use std::sync::atomic::{AtomicU64, Ordering};

        static NEXT: AtomicU64 = AtomicU64::new(0);

        let dir = std::env::temp_dir().join(format!(
            "dip-{label}-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));

        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("a temp directory");

        Self(dir)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
