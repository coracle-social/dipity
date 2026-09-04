//! Writes over `blob`.

use anyhow::{Context, Result};
use coracle_lib::events::EventId;
use rusqlite::params;

use crate::db::Tx;
use crate::model::{Blob, BlobHash};

use super::channel::{self, BlobChange};
use super::query;

/// Record a blob a stored event references. Returns whether it was new.
///
/// The first event to reference a hash — by seen time, since that is the order
/// events arrive in — anchors it, and a later reference changes nothing. The
/// anchor is what the blob's permissions come from, so moving it on every
/// mention would let a later event widen who can fetch an earlier one's media.
pub fn record(tx: &Tx<'_>, blob: &Blob) -> Result<bool> {
    let imeta = serde_json::to_string(&blob.imeta)
        .with_context(|| format!("serializing the imeta tag for blob {}", blob.sha256))?;

    let written = tx
        .prepare_cached(
            "INSERT OR IGNORE INTO blob (
                 sha256, event_id, role, url, mime_type, size, dim, blurhash, alt, blake3,
                 imeta, stored_bytes, chunks, complete, accessed_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)",
        )?
        .execute(params![
            blob.sha256,
            blob.event_id.to_hex(),
            blob.role.as_str(),
            blob.url,
            blob.mime_type,
            blob.size,
            blob.dim,
            blob.blurhash,
            blob.alt,
            blob.blake3,
            imeta,
            blob.stored_bytes,
            blob.chunks,
            blob.complete,
            blob.accessed_at,
        ])
        .with_context(|| format!("recording blob {}", blob.sha256))?;

    if written == 0 {
        return Ok(false);
    }

    channel::notify(tx, BlobChange::Recorded(Box::new(blob.clone())));

    Ok(true)
}

/// Record how much of a blob is held.
///
/// `chunks` is the bitmap per-chunk verification will record itself in
/// (`docs/nips/imeta-blake3.md`); nothing writes it yet, so a transfer that
/// drops mid-file starts over rather than resuming. Called per group of bytes
/// rather than per chunk.
pub fn record_progress(
    tx: &Tx<'_>,
    sha256: &BlobHash,
    stored_bytes: i64,
    chunks: Option<&[u8]>,
) -> Result<bool> {
    let written = tx
        .prepare_cached("UPDATE blob SET stored_bytes = ?2, chunks = ?3 WHERE sha256 = ?1")?
        .execute(params![sha256, stored_bytes, chunks])
        .with_context(|| format!("recording progress for blob {sha256}"))?;

    if written == 0 {
        return Ok(false);
    }

    channel::notify(tx, BlobChange::Progressed(sha256.clone(), stored_bytes));

    Ok(true)
}

/// Mark a blob whole: every byte is held and the file hashes to its address.
pub fn mark_complete(tx: &Tx<'_>, sha256: &BlobHash, stored_bytes: i64, at: i64) -> Result<bool> {
    let written = tx
        .prepare_cached(
            "UPDATE blob
             SET complete = 1, stored_bytes = ?2, size = COALESCE(size, ?2), accessed_at = ?3
             WHERE sha256 = ?1 AND complete = 0",
        )?
        .execute(params![sha256, stored_bytes, at])
        .with_context(|| format!("completing blob {sha256}"))?;

    if written == 0 {
        return Ok(false);
    }

    channel::notify(tx, BlobChange::Completed(sha256.clone()));

    Ok(true)
}

/// Note that a blob was read, which is what LRU eviction orders on.
pub fn touch(tx: &Tx<'_>, sha256: &BlobHash, at: i64) -> Result<bool> {
    let written = tx
        .prepare_cached("UPDATE blob SET accessed_at = ?2 WHERE sha256 = ?1")?
        .execute(params![sha256, at])
        .with_context(|| format!("touching blob {sha256}"))?;

    Ok(written > 0)
}

/// Forget a blob. Returns whether it was there.
///
/// Only the record goes; deleting the bytes is the blob store's job, and it
/// takes this notification as its cue.
pub fn remove(tx: &Tx<'_>, sha256: &BlobHash) -> Result<bool> {
    let removed = tx
        .prepare_cached("DELETE FROM blob WHERE sha256 = ?1")?
        .execute(params![sha256])
        .with_context(|| format!("removing blob {sha256}"))?;

    if removed == 0 {
        return Ok(false);
    }

    channel::notify(tx, BlobChange::Removed(sha256.clone()));

    Ok(true)
}

/// Forget every blob an event anchors, on the way to deleting the event.
///
/// The rows would go by cascade anyway, and silently — nothing would announce
/// them, so nothing would reclaim the bytes. Removing them here is what makes
/// the deletion say so.
pub fn remove_for_event(tx: &Tx<'_>, event_id: &EventId) -> Result<()> {
    for blob in query::list_for_event(tx, event_id)? {
        remove(tx, &blob.sha256)?;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use coracle_lib::tags::{Tag, Tags};

    use crate::db::Db;
    use crate::db::event::command as event_command;
    use crate::fixtures::{author, blob_hash, id, note, peer};
    use crate::model::BlobRole;

    /// Store an event to anchor blobs against, and return its id.
    fn store_event(tx: &Tx<'_>, content: &str) -> EventId {
        let event = note(author(1), 100, content, Tags::new());

        event_command::save(tx, &event, &[peer()], 10).unwrap();

        id(&event)
    }

    #[test]
    fn the_first_event_to_reference_a_hash_anchors_it() {
        let mut db = Db::open_in_memory().unwrap();
        let tx = db.begin_write().unwrap();

        let first = store_event(&tx, "first");
        let second = store_event(&tx, "second");

        let hash = blob_hash(1);

        assert!(record(&tx, &Blob::new(hash.clone(), first, BlobRole::Original)).unwrap());
        assert!(!record(&tx, &Blob::new(hash.clone(), second, BlobRole::Original)).unwrap());

        assert_eq!(query::get(&tx, &hash).unwrap().unwrap().event_id, first);
    }

    /// The batched read has to answer exactly what the per-event one does,
    /// including the order within an event and the events that have nothing.
    #[test]
    fn a_batched_read_answers_what_the_per_event_one_does() {
        let mut db = Db::open_in_memory().unwrap();
        let tx = db.begin_write().unwrap();

        let first = store_event(&tx, "two blobs");
        let second = store_event(&tx, "one blob");
        let bare = store_event(&tx, "no blobs at all");

        // Recorded out of hash order, so grouping cannot be passing by accident.
        for (sha256, event_id) in [
            (blob_hash(3), &first),
            (blob_hash(1), &first),
            (blob_hash(2), &second),
            (blob_hash(4), &second),
        ] {
            record(&tx, &Blob::new(sha256, *event_id, BlobRole::Original)).unwrap();
        }

        let ids = [first, second, bare];
        let batched = query::list_for_events(&tx, &ids).unwrap();

        for id in &ids {
            let one = query::list_for_event(&tx, id).unwrap();

            assert_eq!(
                batched.get(id).cloned().unwrap_or_default(),
                one,
                "batched blobs for {id} differ from the per-event read"
            );
        }

        // Sorted within the event, and an event with nothing is simply absent.
        assert_eq!(
            batched[&first]
                .iter()
                .map(|blob| blob.sha256.clone())
                .collect::<Vec<_>>(),
            [blob_hash(1), blob_hash(3)]
        );
        assert!(!batched.contains_key(&bare));

        assert!(query::list_for_events(&tx, &[]).unwrap().is_empty());
    }

    #[test]
    fn the_whole_imeta_tag_survives_the_round_trip() {
        let mut db = Db::open_in_memory().unwrap();
        let tx = db.begin_write().unwrap();

        let event_id = store_event(&tx, "with media");
        let tag = Tag::new(
            "imeta",
            [
                format!("x {}", blob_hash(1)),
                "m image/jpeg".into(),
                "service nostr.build".into(),
            ],
        );
        let blob = Blob::from_imeta(&tag, event_id).unwrap();

        record(&tx, &blob).unwrap();

        let stored = query::get(&tx, &blob_hash(1)).unwrap().unwrap();

        assert_eq!(stored, blob);
        // A key with no column of its own is still readable, which is the point
        // of keeping the tag rather than only what was parsed out of it.
        assert_eq!(stored.imeta_value("service"), Some("nostr.build"));
    }

    /// The bitmap column is not written by anything yet — per-chunk
    /// verification is what will fill it — but it round trips, so the read
    /// side is ready for it.
    #[test]
    fn progress_short_of_the_whole_file_is_recorded() {
        let mut db = Db::open_in_memory().unwrap();
        let tx = db.begin_write().unwrap();

        let event_id = store_event(&tx, "with media");
        let hash = blob_hash(1);
        record(&tx, &Blob::new(hash.clone(), event_id, BlobRole::Original)).unwrap();

        assert!(record_progress(&tx, &hash, 4_096, Some(&[0b0000_0111])).unwrap());

        let blob = query::get(&tx, &hash).unwrap().unwrap();

        assert_eq!(blob.stored_bytes, 4_096);
        assert_eq!(blob.chunks.as_deref(), Some(&[0b0000_0111][..]));
        assert!(!blob.complete);
        assert!(!query::is_complete(&tx, &hash).unwrap());
    }

    #[test]
    fn completing_takes_a_blob_off_the_want_list() {
        let mut db = Db::open_in_memory().unwrap();
        let tx = db.begin_write().unwrap();

        let event_id = store_event(&tx, "with media");
        let hash = blob_hash(1);
        record(&tx, &Blob::new(hash.clone(), event_id, BlobRole::Original)).unwrap();

        assert_eq!(query::wanted(&tx, 10).unwrap().len(), 1);
        assert!(mark_complete(&tx, &hash, 8_192, 100).unwrap());
        assert!(query::wanted(&tx, 10).unwrap().is_empty());

        // Already whole, so there is nothing to announce a second time.
        assert!(!mark_complete(&tx, &hash, 8_192, 200).unwrap());

        let blob = query::get(&tx, &hash).unwrap().unwrap();
        assert_eq!(blob.size, Some(8_192));
        assert_eq!(query::stored_bytes(&tx, BlobRole::Original).unwrap(), 8_192);
    }

    #[test]
    fn previews_are_wanted_before_originals() {
        let mut db = Db::open_in_memory().unwrap();
        let tx = db.begin_write().unwrap();

        let event_id = store_event(&tx, "with media");
        record(&tx, &Blob::new(blob_hash(1), event_id, BlobRole::Original)).unwrap();
        record(&tx, &Blob::new(blob_hash(2), event_id, BlobRole::Preview)).unwrap();

        let wanted = query::wanted(&tx, 10).unwrap();

        // The preview leads whichever way the hashes sort.
        assert_eq!(wanted[0].sha256, blob_hash(2));
        assert_eq!(wanted[1].sha256, blob_hash(1));
    }

    #[test]
    fn eviction_order_is_least_recently_read() {
        let mut db = Db::open_in_memory().unwrap();
        let tx = db.begin_write().unwrap();

        let event_id = store_event(&tx, "with media");

        for hash in [blob_hash(1), blob_hash(2)] {
            record(&tx, &Blob::new(hash.clone(), event_id, BlobRole::Original)).unwrap();
            mark_complete(&tx, &hash, 1_024, 100).unwrap();
        }

        touch(&tx, &blob_hash(1), 300).unwrap();

        let candidates = query::least_recently_used(&tx, BlobRole::Original, 10).unwrap();

        assert_eq!(
            candidates
                .iter()
                .map(|blob| blob.sha256.clone())
                .collect::<Vec<_>>(),
            [blob_hash(2), blob_hash(1)]
        );
    }

    #[test]
    fn a_blob_record_dies_with_the_event_that_anchors_it() {
        let mut db = Db::open_in_memory().unwrap();
        let tx = db.begin_write().unwrap();

        let anchor = store_event(&tx, "first");
        let other = store_event(&tx, "second");
        let hash = blob_hash(1);

        record(&tx, &Blob::new(hash.clone(), anchor, BlobRole::Original)).unwrap();

        // A second mention changes nothing, the anchor included, so deleting
        // the anchoring event takes the record with it either way.
        assert!(!record(&tx, &Blob::new(hash.clone(), other, BlobRole::Original)).unwrap());

        event_command::delete(&tx, &anchor).unwrap();

        assert!(query::get(&tx, &hash).unwrap().is_none());
        assert!(query::all_hashes(&tx).unwrap().is_empty());
    }

    /// The bytes are reclaimed off this notification, so a deletion that goes
    /// by cascade and announces nothing leaves them on disk forever.
    #[test]
    fn deleting_an_event_announces_every_blob_it_anchored() {
        let mut db = Db::open_in_memory().unwrap();
        let mut changes = channel::subscribe(&db);

        let tx = db.begin_write().unwrap();
        let anchor = store_event(&tx, "with media");

        for hash in [blob_hash(1), blob_hash(2)] {
            record(&tx, &Blob::new(hash, anchor, BlobRole::Original)).unwrap();
        }

        event_command::delete(&tx, &anchor).unwrap();
        tx.commit().unwrap();

        let mut removed = Vec::new();

        while let Ok(change) = changes.try_recv() {
            if let BlobChange::Removed(sha256) = change {
                removed.push(sha256);
            }
        }

        removed.sort();

        assert_eq!(removed, [blob_hash(1), blob_hash(2)]);
    }
}
