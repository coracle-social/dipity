//! What the rest of the core asks the store to do, one function per thing that
//! happens.
//!
//! Each function takes the [`Db`] it is acting on, opens a write transaction on
//! it with [`Db::write`] and threads that through whatever domain commands the
//! change takes. A change spanning models commits/notifies atomically.

use anyhow::Result;
use coracle_lib::events::{EventId, HashedEvent};
use coracle_lib::keys::PublicKey;

use super::{Db, Tx};
use crate::db::blob::command as blob;
use crate::db::event::command as event;
use crate::db::event::query as event_query;
use crate::db::pairing::command as pairing;
use crate::db::pairing::query as pairing_query;
use crate::db::pref::command as pref;
use crate::db::recipient_signature::command as signature;
use crate::db::spending::command as spending;
use crate::model::{Blob, BlobHash, Charge, Policy, RecipientSignature};

/// Take in an event from a peer, with the media it references.
///
/// Returns whether the event is new to this device.
///
/// One transaction over two models, because they only make sense together: the
/// event and the media it names. The author's signature arrives on its own path
/// and is taken by [`receive_signature`].
///
/// `seen_from` is every pubkey the peer proved on the session. A peer
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
/// A local event has provenance and a seen time, with the author's own pubkey
/// standing in for the peer it was seen from.
pub fn publish_event(db: &Db, event: &HashedEvent, identity: &PublicKey, at: i64) -> Result<bool> {
    db.write(|tx| {
        let stored = event::save(tx, event, &[*identity], at)?;

        if stored {
            record_media(tx, event, event.id)?;
        }

        Ok(stored)
    })
}

/// Record that a page of events went to a peer.
///
/// One transaction for the page: a subscription is answered a few hundred events
/// at a time, and a write each would put the cost of serving a peer into
/// SQLite's commit rather than into the radio.
pub fn record_shares(db: &Db, shares: &[(EventId, bool)], to: &[PublicKey], at: i64) -> Result<()> {
    if shares.is_empty() || to.is_empty() {
        return Ok(());
    }

    db.write(|tx| {
        for (id, signed) in shares {
            for pubkey in to {
                event::record_shared(tx, id, pubkey, at, *signed)?;
            }
        }

        Ok(())
    })
}

/// Forget the pairing with a pubkey. Its device is met as a stranger until
/// the two next sync. Returns whether one was held. `docs/discovery.md#recognition`.
pub fn forget_pairing(db: &Db, pubkey: &PublicKey) -> Result<bool> {
    db.write(|tx| pairing::forget_secret(tx, pubkey))
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

/// Spend one disclosure from the bucket, refilled at `per_day`, at `at`.
/// `docs/policy.md#discoverability`.
pub fn spend_disclosure(db: &Db, per_day: u32, at: i64) -> Result<()> {
    db.write(|tx| {
        let bucket = pairing_query::bucket(tx, at)?;

        pairing::save_bucket(tx, bucket.spend(per_day, at))
    })
}

/// Store the author's signature over an event this device already holds.
///
/// For the case where the two arrive separately. Returns whether it was stored.
///
/// The author comes off the stored event rather than from the caller, because
/// it is the only party whose signature over that event means anything.
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

        // Verified at the write, because a row here is the forwarding capability itself.
        if !signature.verifies() {
            return Ok(false);
        }

        signature::save(tx, &signature)
    })
}

/// Record how many verified bytes of a blob are held, which is where the next
/// session's transfer picks it up. Returns whether the blob is known.
pub fn record_blob_progress(db: &Db, sha256: &BlobHash, stored_bytes: u64) -> Result<bool> {
    db.write(|tx| blob::record_progress(tx, sha256, stored_bytes))
}

/// Mark a blob whole: every byte is held and the file hashes to its address.
/// Returns whether this completed it, and `false` if it was already complete.
pub fn complete_blob(db: &Db, sha256: &BlobHash, stored_bytes: u64) -> Result<bool> {
    db.write(|tx| blob::mark_complete(tx, sha256, stored_bytes))
}

/// Write a preference. `value` is a JSON document.
pub fn set_preference(db: &Db, key: &str, value: &str, at: i64) -> Result<()> {
    db.write(|tx| pref::set(tx, key, value, at))
}

/// Remove a preference. Returns whether it existed before.
pub fn clear_preference(db: &Db, key: &str) -> Result<bool> {
    db.write(|tx| pref::remove(tx, key))
}

/// Charge a peer for what it wrote, forgetting charges older than `cutoff`.
pub fn record_charge(db: &Db, charge: &Charge, cutoff: i64) -> Result<()> {
    db.write(|tx| spending::record(tx, charge, cutoff))
}

/// Remember that this device refused `event` by the user's Accept scope, so
/// reconciliation stops offering it until the policy changes.
pub fn refuse_by_policy(db: &Db, event: &HashedEvent, at: i64) -> Result<()> {
    db.write(|tx| event::refuse(tx, event, true, at))
}

/// Forget the refusals the user's settings made, once those settings change.
pub fn forget_policy_refusals(db: &Db) -> Result<usize> {
    db.write(event::forget_policy_refusals)
}

/// Forget what every author outside the user's Accept scope wrote, after the
/// policy has narrowed. Returns how many events went.
pub fn evict_out_of_scope(db: &Db, policy: &Policy, at: i64) -> Result<usize> {
    db.write(|tx| {
        let outside: Vec<PublicKey> = event_query::authors(tx, &policy.identity)?
            .into_iter()
            .filter(|author| !policy.accept.admits(policy.graph.standing(author)))
            .collect();

        event::evict_authors(tx, &policy.identity, &outside, at)
    })
}

/// Forget events that first reached this device before `cutoff`, remembering
/// each as refused at `at`. Returns how many events went. `docs/storage.md#retention`.
pub fn forget_events_seen_before(
    db: &Db,
    identity: &PublicKey,
    cutoff: i64,
    at: i64,
) -> Result<usize> {
    db.write(|tx| event::forget_seen_before(tx, identity, cutoff, at))
}

/// Forget everything the store holds.
///
/// What a device does with what it gathered under the identity it made at
/// first run, once it has adopted one transferred from the user's other phone:
/// the events, preferences, pairings and spending were all that identity's.
/// `docs/keys.md#login-with-device`.
pub fn wipe(db: &Db) -> Result<()> {
    db.write(|tx| {
        event::clear(tx)?;
        blob::clear(tx)?;
        pref::clear(tx)?;
        pairing::clear(tx)?;
        spending::clear(tx)
    })
}

/// Forget one event outright. Returns whether it was there.
///
/// The local half of removing something: the row goes with its sightings, its
/// signatures and its media, and nothing is published. Asking the network to
/// forget an event is a kind 5 the author publishes instead.
/// `docs/storage.md#dropping-one-thing`.
pub fn forget_event(db: &Db, id: &EventId) -> Result<bool> {
    db.write(|tx| event::delete(tx, id))
}

/// Put an event in the trash, or take it back out. `docs/storage.md#the-trash`.
pub fn set_trashed(db: &Db, id: &EventId, trashed: bool, at: i64) -> Result<bool> {
    db.write(|tx| event::set_trashed(tx, id, trashed, at))
}

// --------------------------------------------------- Private helper functions

/// Record the media an event references using imeta.
fn record_media(tx: &Tx<'_>, event: &HashedEvent, id: EventId) -> Result<()> {
    for tag in event.tags.find_all("imeta") {
        if let Some(media) = Blob::from_imeta(tag) {
            blob::record(tx, &media, &id)?;
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
    fn forgetting_a_pairing_drops_that_secret_only() {
        let db = Db::open_in_memory().unwrap();

        pair_with(&db, &[author(2), author(3)], &[1u8; 32], 10).unwrap();

        assert!(forget_pairing(&db, &author(2)).unwrap());
        assert!(!forget_pairing(&db, &author(2)).unwrap());

        let held: Vec<_> = crate::db::query::pair_secrets(&db)
            .unwrap()
            .into_iter()
            .map(|(pubkey, _)| pubkey)
            .collect();
        assert_eq!(held, vec![author(3)]);
    }

    #[test]
    fn a_wipe_leaves_every_table_empty() {
        let db = Db::open_in_memory().unwrap();
        let event = note(
            author(1),
            100,
            "gathered under the first-run identity",
            Tags::new().add("imeta", [format!("x {}", blob_hash(1))]),
        );

        receive_event(&db, &event, &[peer()], 10).unwrap();
        set_preference(&db, "policy.retention_days", "7", 10).unwrap();
        pair_with(&db, &[author(2)], &[1u8; 32], 10).unwrap();
        spend_disclosure(&db, 12, 10).unwrap();

        wipe(&db).unwrap();

        let rows: i64 = db
            .read(|tx| {
                Ok(tx.query_row(
                    "SELECT (SELECT COUNT(*) FROM event) + (SELECT COUNT(*) FROM event_fts)
                          + (SELECT COUNT(*) FROM event_seen) + (SELECT COUNT(*) FROM blob)
                          + (SELECT COUNT(*) FROM blob_reference) + (SELECT COUNT(*) FROM pref)
                          + (SELECT COUNT(*) FROM pair_secret) + (SELECT COUNT(*) FROM disclosure_bucket)",
                    [],
                    |row| row.get(0),
                )?)
            })
            .unwrap();

        assert_eq!(rows, 0);
    }

    #[test]
    fn every_imeta_tag_on_an_event_is_recorded_as_a_blob_it_keeps() {
        let db = Db::open_in_memory().unwrap();
        let first = blob_hash(1);
        let second = blob_hash(2);
        let event = note(
            author(1),
            100,
            "hello neighbor",
            Tags::new()
                .add("imeta", [format!("x {first}"), "m image/jpeg".into()])
                .add(
                    "imeta",
                    [format!("x {second}"), format!("preview-of {first}")],
                ),
        );

        assert!(receive_event(&db, &event, &[peer()], 10).unwrap());

        // A key this build no longer reads still survives in the tag as it arrived.
        let wanted = crate::db::query::wanted_blobs(&db, 10).unwrap();
        let second_blob = wanted.iter().find(|blob| blob.sha256 == second).unwrap();

        assert_eq!(wanted.len(), 2);
        assert_eq!(second_blob.imeta_value("preview-of"), Some(first.as_str()));
    }
}
