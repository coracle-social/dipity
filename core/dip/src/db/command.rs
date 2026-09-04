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
use crate::db::pairing::command as pairing;
use crate::db::pref::command as pref;
use crate::db::recipient_signature::command as signature;
use crate::model::{Blob, BlobHash, BlobRole, RecipientSignature};

/// Take in an event from a peer, with the media it references.
///
/// Returns whether the event is new to this device.
///
/// One transaction over two models, because they only make sense together: the
/// event and the media it names. The author's signature arrives on its own path
/// and is taken by [`receive_signature`].
///
/// `seen_from` is every pubkey the peer proved on the session, so a peer
/// holding more than one identity is recorded from all of them rather than
/// arbitrarily from one.
pub fn receive_event(
    db: &Db,
    event: &HashedEvent,
    seen_from: &[PublicKey],
    seen_at: i64,
) -> Result<bool> {
    db.write(|tx| {
        let stored = event::save(tx, event, seen_from, seen_at)?;

        if stored {
            record_media(tx, event, event.id)?;
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

/// Store the pair secret derived from a completed session against every pubkey
/// the peer proved.
pub fn pair_with(db: &Db, pubkeys: &[PublicKey], secret: &[u8; 32], at: i64) -> Result<()> {
    db.write(|tx| {
        for pubkey in pubkeys {
            pairing::save_secret(tx, pubkey, secret, at)?;
        }

        Ok(())
    })
}

/// Spend one unit of the disclosure budget, for an identity this device has
/// just handed to an unrecognized peer.
pub fn record_disclosure(db: &Db, at: i64) -> Result<()> {
    db.write(|tx| pairing::record_disclosure(tx, at))
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

        let signature = RecipientSignature {
            event_id: *event_id,
            author_pubkey: event.pubkey,
            recipient_pubkey: *identity,
            sig: *sig,
        };

        // A row here is the forwarding capability itself — it is what puts the
        // event in the `Forwardable` register and what an authorship proof is
        // later built from. Verifying at the write means the capability cannot
        // be minted by a caller that forgot to check, whatever path it came in
        // on.
        if !signature.verifies() {
            return Ok(false);
        }

        signature::save(tx, &signature)
    })
}

/// Record how many bytes of a blob are held mid-transfer.
///
/// `chunks` is the bitmap per-chunk verification will fill in once it exists
/// (`docs/nips/imeta-blake3.md`); until then a transfer that drops restarts.
/// Returns whether the blob is known.
pub fn record_blob_progress(
    db: &Db,
    sha256: &BlobHash,
    stored_bytes: i64,
    chunks: Option<&[u8]>,
) -> Result<bool> {
    db.write(|tx| blob::record_progress(tx, sha256, stored_bytes, chunks))
}

/// Mark a blob whole: every byte is held and the file hashes to its address.
/// Returns whether this completed it, and `false` if it was already complete.
pub fn complete_blob(db: &Db, sha256: &BlobHash, stored_bytes: i64, at: i64) -> Result<bool> {
    db.write(|tx| blob::mark_complete(tx, sha256, stored_bytes, at))
}

/// Note that a blob was read, which is what eviction orders on.
pub fn touch_blob(db: &Db, sha256: &BlobHash, at: i64) -> Result<bool> {
    db.write(|tx| blob::touch(tx, sha256, at))
}

/// Write a preference. `value` is a JSON document.
pub fn set_preference(db: &Db, key: &str, value: &str, at: i64) -> Result<()> {
    db.write(|tx| pref::set(tx, key, value, at))
}

/// Remove a preference. Returns whether it existed before.
pub fn clear_preference(db: &Db, key: &str) -> Result<bool> {
    db.write(|tx| pref::remove(tx, key))
}

/// Forget events last handed over before `cutoff`. Returns how many went.
pub fn forget_events_unseen_since(db: &Db, identity: &PublicKey, cutoff: i64) -> Result<usize> {
    db.write(|tx| event::forget_unseen_since(tx, identity, cutoff))
}

/// Evict held originals, until the cache is under `ceiling_bytes`.
/// Returns the hashes evicted.
pub fn evict_originals(db: &Db, ceiling_bytes: i64) -> Result<Vec<BlobHash>> {
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

/// Record the media an event references using imeta, each tag in the role it
/// claims for itself.
fn record_media(tx: &Tx<'_>, event: &HashedEvent, id: EventId) -> Result<()> {
    for tag in event.tags.find_all("imeta") {
        if let Some(media) = Blob::from_imeta(tag, id) {
            blob::record(tx, &media)?;
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use coracle_lib::tags::Tags;

    use crate::fixtures::{author, blob_hash, note, peer};

    #[test]
    fn an_events_media_is_recorded_in_the_role_each_tag_claims() {
        let db = Db::open_in_memory().unwrap();
        let original = blob_hash(1);
        let preview = blob_hash(2);
        let event = note(
            author(1),
            100,
            "hello neighbor",
            Tags::new()
                .add("imeta", [format!("x {original}"), "m image/jpeg".into()])
                .add(
                    "imeta",
                    [
                        format!("x {preview}"),
                        format!("preview-of {original}"),
                        "m image/jpeg".into(),
                    ],
                ),
        );

        assert!(receive_event(&db, &event, &[peer()], 10).unwrap());

        // The preview leads even though it sorts second by hash, which is the
        // precedence `docs/sync.md` gives it.
        let wanted = db.read(|tx| blob_query::wanted(tx, 10)).unwrap();
        let list: Vec<_> = wanted
            .iter()
            .map(|blob| (&blob.sha256, blob.role))
            .collect();

        assert_eq!(
            list,
            [
                (&preview, BlobRole::Preview),
                (&original, BlobRole::Original)
            ]
        );
        assert_eq!(wanted[0].imeta_value("preview-of"), Some(original.as_str()));
    }
}
