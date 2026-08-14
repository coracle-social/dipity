//! What the rest of the core asks the store to do, one function per thing that
//! happens.
//!
//! Each function takes the [`Db`] it is acting on, opens a write transaction on
//! it with [`Db::write`] and threads that through whatever domain commands the
//! change takes, so a change spanning models commits/notifies atomically.

use anyhow::Result;
use coracle_lib::events::{EventId, HashedEvent};
use coracle_lib::keys::PublicKey;

use super::{Db, Tx};
use crate::db::blob::command as blob;
use crate::db::blob::query as blob_query;
use crate::db::event::command as event;
use crate::db::event::query as event_query;
use crate::db::pref::command as pref;
use crate::db::recipient_signature::command as signature;
use crate::db::sql::hex_key;
use crate::model::{Blob, BlobRole, RecipientSignature};

/// Take in an event from a peer, with whatever came alongside it.
///
/// Returns whether the event is new to this device.
///
/// One transaction over three models, because they only make sense together:
/// the event, the author's signature if this device is the recipient it names,
/// and the media the event references. `identity` is this device's own pubkey —
/// the party a signature has to name for a proof to be built from it later.
///
/// `seen_from` is every pubkey the peer proved on the session, so a peer
/// holding more than one identity is recorded from all of them rather than
/// arbitrarily from one. The signature is stored even when the event is not
/// new, since the two arrive independently.
pub fn receive_event(
    db: &Db,
    event: &HashedEvent,
    seen_from: &[PublicKey],
    author_signature: Option<&[u8; 64]>,
    identity: &PublicKey,
    seen_at: i64,
) -> Result<bool> {
    db.write(|tx| {
        let stored = event::save(tx, event, seen_from, seen_at)?;

        if stored {
            record_media(tx, event, event.id)?;
        }

        if let Some(sig) = author_signature
            && event_query::exists(tx, &event.id)?
        {
            signature::save(
                tx,
                &RecipientSignature {
                    event_id: event.id,
                    author_pubkey: event.pubkey,
                    recipient_pubkey: *identity,
                    sig: *sig,
                },
            )?;
        }

        Ok(stored)
    })
}

/// Store an event this device wrote. Returns whether it was new.
///
/// The author's own pubkey stands in for the peer it was seen from, so a local
/// event has provenance and a seen time.
pub fn publish_event(db: &Db, event: &HashedEvent, identity: &PublicKey, at: i64) -> Result<bool> {
    db.write(|tx| {
        let stored = event::save(tx, event, &[*identity], at)?;

        if stored {
            record_media(tx, event, event.id)?;
        }

        Ok(stored)
    })
}

/// Store the author's signature over an event this device already holds.
///
/// For the case where the two arrive separately. Returns whether it was stored.
///
/// The author comes off the stored event rather than from the caller: it is the
/// only party whose signature over that event means anything, so there is
/// nothing for a caller to get wrong.
pub fn receive_signature(
    db: &Db,
    event_id: &EventId,
    sig: &[u8; 64],
    identity: &PublicKey,
) -> Result<bool> {
    db.write(|tx| {
        let Some(event) = event_query::get(tx, event_id)? else {
            return Ok(false);
        };

        signature::save(
            tx,
            &RecipientSignature {
                event_id: *event_id,
                author_pubkey: event.pubkey,
                recipient_pubkey: *identity,
                sig: *sig,
            },
        )
    })
}

/// Record verified bytes against a blob mid-transfer.
///
/// `chunks` is the bitmap of chunks that have verified against the BLAKE3 root,
/// which is what lets a transfer interrupted on BLE resume instead of starting
/// over. Returns whether the blob is known.
pub fn record_blob_progress(
    db: &Db,
    sha256: &str,
    stored_bytes: i64,
    chunks: Option<&[u8]>,
) -> Result<bool> {
    let sha256 = hex_key(sha256)?;

    db.write(|tx| blob::record_progress(tx, &sha256, stored_bytes, chunks))
}

/// Mark a blob whole: every chunk arrived and verified. Returns whether this
/// completed it, and `false` if it was already complete.
pub fn complete_blob(db: &Db, sha256: &str, stored_bytes: i64, at: i64) -> Result<bool> {
    let sha256 = hex_key(sha256)?;

    db.write(|tx| blob::mark_complete(tx, &sha256, stored_bytes, at))
}

/// Note that a blob was read, which is what eviction orders on.
pub fn touch_blob(db: &Db, sha256: &str, at: i64) -> Result<bool> {
    let sha256 = hex_key(sha256)?;

    db.write(|tx| blob::touch(tx, &sha256, at))
}

/// Write a preference. `value` is a JSON document.
pub fn set_preference(db: &Db, key: &str, value: &str, at: i64) -> Result<()> {
    db.write(|tx| pref::set(tx, key, value, at))
}

/// Remove a preference. Returns whether it existed before.
pub fn clear_preference(db: &Db, key: &str) -> Result<bool> {
    db.write(|tx| pref::remove(tx, key))
}

/// Forget events first seen before `cutoff`. Returns how many were deleted.
pub fn forget_events_before(db: &Db, cutoff: i64) -> Result<usize> {
    db.write(|tx| event::forget_seen_before(tx, cutoff))
}

/// Evict held originals, until the cache is under `ceiling_bytes`.
/// Returns the hashes evicted.
pub fn evict_originals(db: &Db, ceiling_bytes: i64) -> Result<Vec<String>> {
    db.write(|tx| {
        let mut held = blob_query::stored_bytes(tx, BlobRole::Original)?;

        if held <= ceiling_bytes {
            return Ok(Vec::new());
        }

        let mut evicted = Vec::new();

        // Limit how many blobs we evict in one shot to avoid locking for too long
        for candidate in blob_query::least_recently_used(tx, BlobRole::Original, 256)? {
            if held <= ceiling_bytes {
                break;
            }

            if blob::remove(tx, &candidate.sha256)? {
                held -= candidate.stored_bytes;
                evicted.push(candidate.sha256);
            }
        }

        Ok(evicted)
    })
}

// ============================================================================
// Private helper functions
// ============================================================================

/// Record the media an event references using imeta.
fn record_media(tx: &Tx<'_>, event: &HashedEvent, id: EventId) -> Result<()> {
    for tag in event.tags.find_all("imeta") {
        if let Some(media) = Blob::from_imeta(tag, id, BlobRole::Original) {
            blob::record(tx, &media)?;
        }
    }

    Ok(())
}
