//! Writes over `blob`.

use anyhow::{Context, Result};
use coracle_lib::events::EventId;
use rusqlite::params;

use crate::db::Tx;
use crate::model::{Blob, BlobHash};

use super::channel::{self, BlobChange};
use super::query;

/// Record that `event_id` references a blob. Returns whether the blob was new.
///
/// The metadata is the first referring event's and a later reference leaves it
/// alone: the columns are read off an `imeta` tag, and the second event's copy
/// of it says nothing about the bytes the first one's hash already addresses.
/// What every reference does add is a claim on the blob's life — the row and
/// the bytes outlive any one of them — and a say in who may fetch it.
pub fn record(tx: &Tx<'_>, blob: &Blob, event_id: &EventId) -> Result<bool> {
    let imeta = serde_json::to_string(&blob.imeta)
        .with_context(|| format!("serializing the imeta tag for blob {}", blob.sha256))?;

    tx.prepare_cached("INSERT OR IGNORE INTO blob_reference (sha256, event_id) VALUES (?1, ?2)")?
        .execute(params![blob.sha256, event_id.to_hex()])
        .with_context(|| format!("referencing blob {} from {event_id}", blob.sha256))?;

    let written = tx
        .prepare_cached(
            "INSERT OR IGNORE INTO blob (
                 sha256, role, url, mime_type, size, dim, blurhash, alt, blake3,
                 imeta, stored_bytes, complete, accessed_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
        )?
        .execute(params![
            blob.sha256,
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

/// Record how many verified bytes of a blob are held, which is where the next
/// session's transfer picks the blob up.
pub fn record_progress(tx: &Tx<'_>, sha256: &BlobHash, stored_bytes: u64) -> Result<bool> {
    let written = tx
        .prepare_cached("UPDATE blob SET stored_bytes = ?2 WHERE sha256 = ?1")?
        .execute(params![sha256, column_bytes(stored_bytes)])
        .with_context(|| format!("recording progress for blob {sha256}"))?;

    if written == 0 {
        return Ok(false);
    }

    channel::notify(tx, BlobChange::Progressed(sha256.clone(), stored_bytes));

    Ok(true)
}

/// Mark a blob whole: every byte is held and the file hashes to its address.
pub fn mark_complete(tx: &Tx<'_>, sha256: &BlobHash, stored_bytes: u64, at: i64) -> Result<bool> {
    let written = tx
        .prepare_cached(
            "UPDATE blob
             SET complete = 1, stored_bytes = ?2, size = COALESCE(size, ?2), accessed_at = ?3
             WHERE sha256 = ?1 AND complete = 0",
        )?
        .execute(params![sha256, column_bytes(stored_bytes), at])
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

/// A byte count as the column holds it: SQLite's integers are signed, and no
/// file this device wrote a group at a time reaches the saturation point.
fn column_bytes(bytes: u64) -> i64 {
    i64::try_from(bytes).unwrap_or(i64::MAX)
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

/// Drop an event's references, on the way to deleting the event, and with them
/// every blob no other event still references.
///
/// The reference rows would go by cascade anyway, and silently — nothing would
/// announce the blobs they were the last claim on, and nothing would reclaim the
/// bytes. Dropping them here is what makes the deletion say so.
pub fn remove_for_event(tx: &Tx<'_>, event_id: &EventId) -> Result<()> {
    let referenced = query::list_for_event(tx, event_id)?;

    tx.prepare_cached("DELETE FROM blob_reference WHERE event_id = ?1")?
        .execute(params![event_id.to_hex()])
        .with_context(|| format!("dropping the blob references of {event_id}"))?;

    for blob in referenced {
        if query::events_referencing(tx, &blob.sha256)?.is_empty() {
            remove(tx, &blob.sha256)?;
        }
    }

    Ok(())
}

/// Remove every blob record. The bytes go with the sweep the node runs at open.
/// Part of [`wipe`](crate::db::command::wipe).
pub fn clear(tx: &Tx<'_>) -> Result<()> {
    tx.execute_batch("DELETE FROM blob_reference; DELETE FROM blob;")
        .context("clearing the blob tables")
}

#[cfg(test)]
mod tests {
    use super::*;
    use coracle_lib::tags::{Tag, Tags};

    use crate::db::Db;
    use crate::db::event::command as event_command;
    use crate::fixtures::{author, blob_hash, id, note, peer};
    use crate::model::BlobRole;

    /// Store an event to reference blobs from, and return its id.
    fn store_event(tx: &Tx<'_>, content: &str) -> EventId {
        let event = note(author(1), 100, content, Tags::new());

        event_command::save(tx, &event, &[peer()], 10).unwrap();

        id(&event)
    }

    #[test]
    fn a_second_reference_is_recorded_and_the_metadata_is_not() {
        let mut db = Db::open_in_memory().unwrap();
        let tx = db.begin_write().unwrap();

        let first = store_event(&tx, "first");
        let second = store_event(&tx, "second");

        let hash = blob_hash(1);
        let mut later = Blob::new(hash.clone(), BlobRole::Original);
        later.alt = Some("read off the second event".into());

        assert!(record(&tx, &Blob::new(hash.clone(), BlobRole::Original), &first).unwrap());
        assert!(!record(&tx, &later, &second).unwrap());

        assert_eq!(query::get(&tx, &hash).unwrap().unwrap().alt, None);
        assert_eq!(
            query::events_referencing(&tx, &hash).unwrap().len(),
            2,
            "the second event did not claim the blob"
        );
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

        // Recorded out of hash order, to keep grouping from passing by accident.
        for (sha256, event_id) in [
            (blob_hash(3), &first),
            (blob_hash(1), &first),
            (blob_hash(2), &second),
            (blob_hash(4), &second),
        ] {
            record(&tx, &Blob::new(sha256, BlobRole::Original), event_id).unwrap();
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
        let blob = Blob::from_imeta(&tag).unwrap();

        record(&tx, &blob, &event_id).unwrap();

        let stored = query::get(&tx, &blob_hash(1)).unwrap().unwrap();

        assert_eq!(stored, blob);
        // A key with no column of its own is still readable off the kept tag.
        assert_eq!(stored.imeta_value("service"), Some("nostr.build"));
    }

    #[test]
    fn progress_short_of_the_whole_file_is_recorded() {
        let mut db = Db::open_in_memory().unwrap();
        let tx = db.begin_write().unwrap();

        let event_id = store_event(&tx, "with media");
        let hash = blob_hash(1);
        record(&tx, &Blob::new(hash.clone(), BlobRole::Original), &event_id).unwrap();

        assert!(record_progress(&tx, &hash, 4_096).unwrap());

        let blob = query::get(&tx, &hash).unwrap().unwrap();

        assert_eq!(blob.stored_bytes, 4_096);
        assert!(!blob.complete);
        assert!(!query::is_complete(&tx, &hash).unwrap());
    }

    #[test]
    fn completing_takes_a_blob_off_the_want_list() {
        let mut db = Db::open_in_memory().unwrap();
        let tx = db.begin_write().unwrap();

        let event_id = store_event(&tx, "with media");
        let hash = blob_hash(1);
        record(&tx, &Blob::new(hash.clone(), BlobRole::Original), &event_id).unwrap();

        assert_eq!(query::wanted(&tx, 10).unwrap().len(), 1);
        assert!(mark_complete(&tx, &hash, 8_192, 100).unwrap());
        assert!(query::wanted(&tx, 10).unwrap().is_empty());

        // Nothing to announce a second time, because it is already whole.
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
        record(&tx, &Blob::new(blob_hash(1), BlobRole::Original), &event_id).unwrap();
        record(&tx, &Blob::new(blob_hash(2), BlobRole::Preview), &event_id).unwrap();

        let wanted = query::wanted(&tx, 10).unwrap();

        // The preview leads whichever way the hashes sort.
        assert_eq!(wanted[0].sha256, blob_hash(2));
        assert_eq!(wanted[1].sha256, blob_hash(1));
    }

    #[test]
    fn a_partial_original_does_not_count_against_the_cache() {
        let mut db = Db::open_in_memory().unwrap();
        let tx = db.begin_write().unwrap();

        let event_id = store_event(&tx, "with media");
        let hash = blob_hash(1);
        record(&tx, &Blob::new(hash.clone(), BlobRole::Original), &event_id).unwrap();
        record_progress(&tx, &hash, 4_096).unwrap();

        // Counting it would hold the cache over its ceiling for good, as nothing could evict it.
        assert_eq!(query::stored_bytes(&tx, BlobRole::Original).unwrap(), 0);
    }

    #[test]
    fn eviction_order_is_least_recently_read() {
        let mut db = Db::open_in_memory().unwrap();
        let tx = db.begin_write().unwrap();

        let event_id = store_event(&tx, "with media");

        for hash in [blob_hash(1), blob_hash(2)] {
            record(&tx, &Blob::new(hash.clone(), BlobRole::Original), &event_id).unwrap();
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
    fn a_blob_dies_with_the_last_event_to_reference_it_and_not_the_first() {
        let mut db = Db::open_in_memory().unwrap();
        let tx = db.begin_write().unwrap();

        let first = store_event(&tx, "first");
        let second = store_event(&tx, "second");
        let hash = blob_hash(1);

        record(&tx, &Blob::new(hash.clone(), BlobRole::Original), &first).unwrap();
        record(&tx, &Blob::new(hash.clone(), BlobRole::Original), &second).unwrap();

        event_command::delete(&tx, &first).unwrap();

        assert!(
            query::get(&tx, &hash).unwrap().is_some(),
            "the second event still references it"
        );
        assert_eq!(query::events_referencing(&tx, &hash).unwrap(), [second]);
        assert_eq!(query::list_for_event(&tx, &second).unwrap().len(), 1);

        event_command::delete(&tx, &second).unwrap();

        assert!(query::get(&tx, &hash).unwrap().is_none());
        assert!(query::all_hashes(&tx).unwrap().is_empty());
        assert!(query::events_referencing(&tx, &hash).unwrap().is_empty());
    }

    /// The bytes are reclaimed off this notification. A deletion that goes
    /// by cascade and announces nothing leaves them on disk forever. A blob
    /// another event still wants must not be announced at all.
    #[test]
    fn deleting_an_event_announces_the_blobs_it_was_the_last_claim_on() {
        let mut db = Db::open_in_memory().unwrap();
        let mut changes = channel::subscribe(&db);

        let tx = db.begin_write().unwrap();
        let going = store_event(&tx, "with media");
        let staying = store_event(&tx, "shares one of them");

        for hash in [blob_hash(1), blob_hash(2)] {
            record(&tx, &Blob::new(hash, BlobRole::Original), &going).unwrap();
        }

        record(&tx, &Blob::new(blob_hash(2), BlobRole::Original), &staying).unwrap();

        event_command::delete(&tx, &going).unwrap();
        tx.commit().unwrap();

        let mut removed = Vec::new();

        while let Ok(change) = changes.try_recv() {
            if let BlobChange::Removed(sha256) = change {
                removed.push(sha256);
            }
        }

        removed.sort();

        assert_eq!(removed, [blob_hash(1)]);
    }
}
