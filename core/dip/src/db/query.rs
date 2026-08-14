//! What the rest of the core asks the store, one function per question.
//!
//! Each function takes the [`Db`] it is asking, opens a read transaction on it
//! with [`Db::read`] and threads that through whatever domain queries the
//! answer takes, so a caller never holds a transaction, never names a table,
//! and never gets a half-consistent answer assembled from two of them.
//!
//! Every question about events is [`list_events`], because what separates the
//! feed from a peer's `REQ` is which constraints a [`Query`] carries rather
//! than which function is called. [`with_details`] is the follow-up read for a
//! caller that wants what the store knows about an event beyond the event.

use std::collections::BTreeSet;

use anyhow::Result;
use coracle_lib::events::{EventId, HashedEvent};
use coracle_lib::keys::PublicKey;

use super::Db;
use crate::db::blob::query as blob;
use crate::db::event::query as event;
use crate::db::pref::query as pref;
use crate::db::sql::hex_key;
use crate::model::{Blob, BlobRole, Policy, Pref, Provenance, Query};

// ============================================================================
// Policy and preferences
// ============================================================================

/// Every preference, for the settings screen.
pub fn preferences(db: &Db) -> Result<Vec<Pref>> {
    db.read(pref::all)
}

/// One preference's raw JSON value, or `None` if it has never been written.
pub fn preference(db: &Db, key: &str) -> Result<Option<String>> {
    db.read(|tx| pref::get(tx, key))
}

/// Everything the user has said about who gets what.
pub fn policy(db: &Db, identity: &PublicKey) -> Result<Policy> {
    db.read(|tx| pref::policy(tx, identity))
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

/// Events matching every constraint on `query`.
pub fn list_events(db: &Db, query: &Query) -> Result<Vec<HashedEvent>> {
    db.read(|tx| event::list(tx, query))
}

/// Attach each event's media and provenance to it.
///
/// Two reads for the page rather than two per event: the caller is the view
/// rendering a feed, and asking per event made the cost of showing a screen
/// scale with how much of it is on screen.
pub fn with_details(db: &Db, events: Vec<HashedEvent>) -> Result<Vec<EventDetail>> {
    let ids: Vec<EventId> = events
        .iter()
        .map(|event| event.id)
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();

    db.read(|tx| {
        let blobs = blob::list_for_events(tx, &ids)?;
        let sightings = event::provenance_for(tx, &ids)?;

        Ok(events
            .into_iter()
            .map(|event| {
                let id = event.id;

                EventDetail {
                    blobs: blobs.get(&id).cloned().unwrap_or_default(),
                    sightings: sightings.get(&id).cloned().unwrap_or_default(),
                    event,
                }
            })
            .collect())
    })
}

// ============================================================================
// Blobs
// ============================================================================

/// Blobs a stored event references and this device does not hold, previews first.
pub fn wanted_blobs(db: &Db, limit: usize) -> Result<Vec<Blob>> {
    db.read(|tx| blob::wanted(tx, limit))
}

/// One blob's metadata and transfer progress.
pub fn get_blob(db: &Db, sha256: &str) -> Result<Option<Blob>> {
    let sha256 = hex_key(sha256)?;

    db.read(|tx| blob::get(tx, &sha256))
}

/// How many bytes of held originals the cache is carrying, which is what the
/// ceiling in `docs/sync.md` is measured against.
pub fn cached_bytes(db: &Db) -> Result<i64> {
    db.read(|tx| blob::stored_bytes(tx, BlobRole::Original))
}
