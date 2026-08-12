//! What the rest of the core asks the store, one function per question.
//!
//! Each function opens a read transaction with [`db::read`](super::read) and
//! threads it through whatever domain queries the answer takes, so a caller
//! never holds a transaction, never names a table, and never gets a
//! half-consistent answer assembled from two of them.
//!
//! Every question about events is [`list_events`], because what separates the
//! feed from a peer's `REQ` is which constraints an
//! [`EventFilter`](crate::domain::event::model::EventFilter) carries rather
//! than which function is called. [`with_details`] is the follow-up read for a
//! caller that wants what the store knows about an event beyond the event.

use anyhow::Result;
use coracle_lib::events::HashedEvent;
use coracle_lib::keys::PublicKey;

use super::read;
use crate::domain::blob::model::{Blob, BlobRole};
use crate::domain::blob::query as blob;
use crate::domain::event::model::{EventFilter, Provenance};
use crate::domain::event::query as event;
use crate::domain::pref::model::{Policy, Pref};
use crate::domain::pref::query as pref;

// ============================================================================
// Policy and preferences
// ============================================================================

/// Every preference, for the settings screen.
pub fn preferences() -> Result<Vec<Pref>> {
    read(pref::all)
}

/// One preference's raw JSON value, or `None` if it has never been written.
pub fn preference(key: &str) -> Result<Option<String>> {
    read(|tx| pref::get(tx, key))
}

/// Everything the user has said about who gets what.
pub fn policy(identity: &PublicKey) -> Result<Policy> {
    read(|tx| pref::policy(tx, identity))
}

// ============================================================================
// Events
// ============================================================================

/// An event, and what the store knows about it that the event does not carry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EventDetail {
    /// The event itself.
    pub event: HashedEvent,
    /// The media it references, held or not.
    pub blobs: Vec<Blob>,
    /// Which peers it arrived from, and when. Never served to a peer.
    pub sightings: Vec<Provenance>,
}

/// Events matching every constraint on `filter`.
pub fn list_events(filter: &EventFilter) -> Result<Vec<HashedEvent>> {
    read(|tx| event::list(tx, filter))
}

/// Attach each event's media and provenance to it.
pub fn with_details(events: Vec<HashedEvent>) -> Result<Vec<EventDetail>> {
    read(|tx| {
        events
            .into_iter()
            .map(|event| {
                let id = hex::encode(event.id);

                Ok(EventDetail {
                    blobs: blob::list_for_event(tx, &id)?,
                    sightings: event::provenance(tx, &id)?,
                    event,
                })
            })
            .collect()
    })
}

// ============================================================================
// Blobs
// ============================================================================

/// Blobs a stored event references and this device does not hold, previews first.
pub fn wanted_blobs(limit: usize) -> Result<Vec<Blob>> {
    read(|tx| blob::wanted(tx, limit))
}

/// One blob's metadata and transfer progress.
pub fn get_blob(sha256: &str) -> Result<Option<Blob>> {
    read(|tx| blob::get(tx, sha256))
}

/// How many bytes of held originals the cache is carrying, which is what the
/// ceiling in `docs/sync.md` is measured against.
pub fn cached_bytes() -> Result<i64> {
    read(|tx| blob::stored_bytes(tx, BlobRole::Original))
}
