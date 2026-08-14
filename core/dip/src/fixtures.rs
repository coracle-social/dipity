//! Events and keys for tests, shared by every model and every table that hangs
//! off `event`.
//!
//! Keys are real points on the curve, because `PublicKey` will not hold
//! anything else, and ids are real hashes, because building an event through
//! the pipeline computes one. Both are derived from a seed, so a fixture is
//! reproducible and two seeds never collide.

use coracle_lib::events::{EventContent, EventId, HashedEvent};
use coracle_lib::keys::{PublicKey, SecretKey};
use coracle_lib::tags::Tags;

/// The key for a seed. Every seed below 255 is a valid scalar.
pub(crate) fn secret(seed: u8) -> SecretKey {
    SecretKey::from_hex(&hex::encode([seed; 32])).expect("seed is a valid secret key")
}

/// The pubkey for a seed.
pub(crate) fn author(seed: u8) -> PublicKey {
    secret(seed).public_key()
}

/// The peer a fixture is seen from, when the test does not care which.
pub(crate) fn peer() -> PublicKey {
    author(200)
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
