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

use std::collections::{BTreeSet, HashSet};

use anyhow::Result;
use coracle_lib::events::{EventId, HashedEvent};
use coracle_lib::keys::PublicKey;
use coracle_lib::sync::{Item, SyncSet};

use super::Db;
use crate::db::blob::query as blob;
use crate::db::event::query as event;
use crate::db::pairing::query as pairing;
use crate::db::pref::query as pref;
use crate::db::recipient_signature::query as signature;
use crate::db::spending::query as spending;
use crate::model::{
    Blob, BlobHash, BlobRole, Charge, DisclosureBucket, NotificationPrefs, Policy, Pref,
    Provenance, Query, RecipientSignature, Share, keys,
};

// ----------------------------------------------------- Policy and preferences

/// Every preference, for the settings screen.
pub fn preferences(db: &Db) -> Result<Vec<Pref>> {
    db.read(pref::all)
}

/// One preference's raw JSON value, or `None` if it has never been written.
pub fn preference(db: &Db, key: &str) -> Result<Option<String>> {
    db.read(|tx| pref::get(tx, key))
}

/// Which notifications the user switched on.
pub fn notification_prefs(db: &Db) -> Result<NotificationPrefs> {
    db.read(|tx| {
        Ok(NotificationPrefs {
            pairing: pref::get_as(tx, keys::NOTIFY_PAIRING)?.unwrap_or(false),
            content: pref::get_as(tx, keys::NOTIFY_CONTENT)?.unwrap_or(false),
        })
    })
}

/// Whether the user has named `pubkey`, which is a contact card of theirs addressed to it.
pub fn has_named(db: &Db, identity: &PublicKey, pubkey: &PublicKey) -> Result<bool> {
    let address =
        coracle_lib::addresses::Address::new(crate::model::CONTACT, *identity, pubkey.to_hex());

    db.read(|tx| Ok(event::by_address(tx, &address)?.is_some()))
}

/// Everything the user has said about who gets what.
pub fn policy(db: &Db, identity: &PublicKey) -> Result<Policy> {
    db.read(|tx| pref::policy(tx, identity))
}

// -------------------------------------------------------------------- Pairing

/// Every pair secret this device holds, for trial-MACing a peer's recognition
/// tags.
pub fn pair_secrets(db: &Db) -> Result<Vec<(PublicKey, [u8; 32])>> {
    db.read(pairing::secrets)
}

/// The disclosure bucket, full at `now` if nothing has been spent from it.
pub fn disclosure_bucket(db: &Db, now: i64) -> Result<DisclosureBucket> {
    db.read(|tx| pairing::bucket(tx, now))
}

// --------------------------------------------------------------------- Events

/// An event, and what the store knows about it that the event does not carry.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct EventDetail {
    /// The event itself.
    pub event: HashedEvent,
    /// The media it references, held or not.
    pub blobs: Vec<Blob>,
    /// Which peers it arrived from, and when. Never served to a peer.
    pub sightings: Vec<Provenance>,
    /// Which peers it has been handed to, and when. Never served to a peer.
    pub shares: Vec<Share>,
}

/// Events matching every constraint on `query`.
pub fn list_events(db: &Db, query: &Query) -> Result<Vec<HashedEvent>> {
    db.read(|tx| event::list(tx, query))
}

/// Events matching `query`, reduced to the `(id, timestamp)` set negentropy diffs.
pub fn reconciliation_set(db: &Db, query: &Query) -> Result<SyncSet> {
    Ok(SyncSet::from_items(
        list_events(db, query)?.into_iter().map(|event| Item {
            timestamp: event.created_at,
            id: event.id,
        }),
    ))
}

/// What this device opens a reconciliation with: everything it holds under
/// `query`, and every id it has refused, so neither is reported missing.
///
/// A refused id carries no kind or author, so it joins the set whatever the
/// filter says. That costs nothing: an id the peer does not hold under the
/// filter lands in the unused `have` side of the diff.
pub fn initiator_set(db: &Db, query: &Query) -> Result<SyncSet> {
    let held = list_events(db, query)?.into_iter().map(|event| Item {
        timestamp: event.created_at,
        id: event.id,
    });
    let refused = db.read(event::refused)?;

    Ok(SyncSet::from_items(held.chain(refused)))
}

/// Every charge in the quota ledger at or after `cutoff`, oldest first.
pub fn charges_since(db: &Db, cutoff: i64) -> Result<Vec<Charge>> {
    db.read(|tx| spending::since(tx, cutoff))
}

/// Every pubkey an event has been seen from, which is local provenance and
/// never served.
pub fn seen_from(db: &Db, id: &EventId) -> Result<Vec<PublicKey>> {
    db.read(|tx| event::seen_from(tx, id))
}

/// One event, by id, or `None` if it is not stored.
pub fn get_event(db: &Db, id: &EventId) -> Result<Option<HashedEvent>> {
    db.read(|tx| event::get(tx, id))
}

/// What is in the trash, newest first, with when each went in.
pub fn trashed(db: &Db) -> Result<Vec<(EventId, i64)>> {
    db.read(event::trashed)
}

/// The user's own events handed to any of `to` without the recipient signature.
pub fn unsigned_shares(db: &Db, identity: &PublicKey, to: &[PublicKey]) -> Result<Vec<EventId>> {
    db.read(|tx| event::unsigned_shares(tx, identity, to))
}

/// The author's signature over an event naming `recipient`, if this device
/// holds it.
pub fn get_signature(
    db: &Db,
    event_id: &EventId,
    recipient: &PublicKey,
) -> Result<Option<RecipientSignature>> {
    db.read(|tx| signature::get(tx, event_id, recipient))
}

/// Attach each event's media and provenance to it.
///
/// Three reads for the page rather than three per event: the caller is the view
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
        let shares = event::shares_for(tx, &ids)?;

        Ok(events
            .into_iter()
            .map(|event| {
                let id = event.id;

                EventDetail {
                    blobs: blobs.get(&id).cloned().unwrap_or_default(),
                    sightings: sightings.get(&id).cloned().unwrap_or_default(),
                    shares: shares.get(&id).cloned().unwrap_or_default(),
                    event,
                }
            })
            .collect())
    })
}

// ---------------------------------------------------------------------- Blobs

/// Blobs a stored event references and this device does not hold, previews first.
pub fn wanted_blobs(db: &Db, limit: usize) -> Result<Vec<Blob>> {
    db.read(|tx| blob::wanted(tx, limit))
}

/// One blob's metadata and transfer progress.
pub fn get_blob(db: &Db, sha256: &BlobHash) -> Result<Option<Blob>> {
    db.read(|tx| blob::get(tx, sha256))
}

/// Every stored event that references a hash, whose permissions are the blob's.
pub fn events_referencing_blob(db: &Db, sha256: &BlobHash) -> Result<Vec<EventId>> {
    db.read(|tx| blob::events_referencing(tx, sha256))
}

/// How many bytes of held originals the cache is carrying, which is what the
/// ceiling in `docs/sync.md` is measured against.
pub fn cached_bytes(db: &Db) -> Result<i64> {
    db.read(|tx| blob::stored_bytes(tx, BlobRole::Original))
}

/// Every blob hash the store has a record for, which is every hash a blob
/// store is entitled to be holding bytes for.
pub fn recorded_blob_hashes(db: &Db) -> Result<HashSet<BlobHash>> {
    db.read(blob::all_hashes)
}
