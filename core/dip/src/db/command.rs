//! What the rest of the core asks the store to do, one function per thing that
//! happens.
//!
//! Each function opens a write transaction with [`db::write`] and threads it
//! through whatever domain commands the change takes, so a change spanning
//! models commits/notifies atomically.

use anyhow::Result;
use coracle_lib::events::HashedEvent;
use coracle_lib::keys::PublicKey;

use super::{Tx, write};
use crate::db::blob::command as blob;
use crate::db::blob::query as blob_query;
use crate::db::event::command as event;
use crate::db::event::query as event_query;
use crate::db::pref::command as pref;
use crate::db::proof::command as proof;
use crate::model::{Blob, BlobRole, Proof};

/// Take in an event from a peer, with whatever came alongside it.
///
/// Returns whether the event is new to this device.
///
/// One transaction over three models, because they only make sense together:
/// the event, the author's signature if this device is the recipient it names,
/// and the media the event references. `identity` is this device's own pubkey —
/// the party a signature has to name for a proof to be built from it later.
///
/// The signature is stored even when the event is not new, since the two arrive
/// independently.
///
/// # Errors
///
/// If any of the writes fail, in which case nothing is written and nothing is
/// announced.
pub fn receive_event(
    event: &HashedEvent,
    from_peer: &PublicKey,
    author_signature: Option<&[u8; 64]>,
    identity: &PublicKey,
    seen_at: i64,
) -> Result<bool> {
    write(|tx| {
        let id = hex::encode(event.id);
        let stored = event::save(tx, event, from_peer, seen_at)?;

        if stored {
            record_media(tx, event, &id)?;
        }

        if let Some(sig) = author_signature
            && event_query::exists(tx, &id)?
        {
            proof::save(
                tx,
                &Proof {
                    event_id: id,
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
pub fn publish_event(event: &HashedEvent, identity: &PublicKey, at: i64) -> Result<bool> {
    write(|tx| {
        let stored = event::save(tx, event, identity, at)?;

        if stored {
            record_media(tx, event, &hex::encode(event.id))?;
        }

        Ok(stored)
    })
}

/// Store the author's signature over an event this device already holds.
///
/// For the case where the two arrive separately. Returns whether it was stored.
pub fn receive_signature(event_id: &str, sig: &[u8; 64], identity: &PublicKey) -> Result<bool> {
    write(|tx| {
        if !event_query::exists(tx, event_id)? {
            return Ok(false);
        }

        proof::save(
            tx,
            &Proof {
                event_id: event_id.to_string(),
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
    sha256: &str,
    stored_bytes: i64,
    chunks: Option<&[u8]>,
) -> Result<bool> {
    write(|tx| blob::record_progress(tx, sha256, stored_bytes, chunks))
}

/// Mark a blob whole: every chunk arrived and verified. Returns whether this
/// completed it, and `false` if it was already complete.
pub fn complete_blob(sha256: &str, stored_bytes: i64, at: i64) -> Result<bool> {
    write(|tx| blob::mark_complete(tx, sha256, stored_bytes, at))
}

/// Note that a blob was read, which is what eviction orders on.
pub fn touch_blob(sha256: &str, at: i64) -> Result<bool> {
    write(|tx| blob::touch(tx, sha256, at))
}

/// Write a preference. `value` is a JSON document.
pub fn set_preference(key: &str, value: &str, at: i64) -> Result<()> {
    write(|tx| pref::set(tx, key, value, at))
}

/// Remove a preference. Returns whether it existed before.
pub fn clear_preference(key: &str) -> Result<bool> {
    write(|tx| pref::remove(tx, key))
}

/// Forget events first seen before `cutoff`. Returns how many were deleted.
pub fn forget_events_before(cutoff: i64) -> Result<usize> {
    write(|tx| event::forget_seen_before(tx, cutoff))
}

/// Evict held originals, until the cache is under `ceiling_bytes`.
/// Returns the hashes evicted.
pub fn evict_originals(ceiling_bytes: i64) -> Result<Vec<String>> {
    write(|tx| {
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
fn record_media(tx: &Tx<'_>, event: &HashedEvent, id: &str) -> Result<()> {
    for tag in event.tags.find_all("imeta") {
        if let Some(media) = Blob::from_imeta(tag, id, BlobRole::Original) {
            blob::record(tx, &media)?;
        }
    }

    Ok(())
}
