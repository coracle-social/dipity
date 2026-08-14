//! Writes over `event`, `event_tag`, `event_fts` and `event_seen`.
//!
//! [`save`] is the whole of ingest below the policy layer: it enforces what the
//! tables mean to each other — that the tag and full-text indexes match the
//! event, that a replaceable event has one row per address, that a deletion the
//! author already published is not undone by the next peer to offer the event,
//! and that every sighting is recorded whether or not the event was new.
//!
//! Authorization is not here. An event reaches this point already accepted by
//! the authenticated session or by an authorship proof; a peer that cannot
//! establish authorship never gets as far as a write. See `docs/proofs.md`.

use anyhow::{Context, Result};
use coracle_lib::addresses::{Address, EventExtensionAddress};
use coracle_lib::events::HashedEvent;
use coracle_lib::keys::PublicKey;
use coracle_lib::kinds::is_ephemeral;
use rusqlite::params;

use crate::db::Tx;
use crate::model::{KIND_DELETE, Provenance, is_indexed_tag};

use super::events::{self, EventChange};
use super::query;

/// Store an event seen from `peer_pubkey` at `seen_at`, and record the
/// sighting. Returns whether the event is new to this device.
///
/// A locally authored event goes through here too, naming the author's own
/// pubkey as the peer, so every stored event has provenance and a seen time.
///
/// Nothing is stored when the event is ephemeral, when its author has already
/// deleted it, or when a newer event holds its address. A sighting is still
/// recorded for an event already stored, because who handed it over and when is
/// the point of provenance.
///
/// # Errors
///
/// If any of the writes fail, in which case the caller's transaction rolls back
/// and the tables stay consistent with each other.
pub fn save(
    tx: &Tx<'_>,
    event: &HashedEvent,
    peer_pubkey: &PublicKey,
    seen_at: i64,
) -> Result<bool> {
    // Ephemeral kinds are relayed, never stored. Kind 22242 auth events are the
    // ones this app produces, and they belong to a session, not to a store.
    if is_ephemeral(event.kind) {
        return Ok(false);
    }

    let id = hex::encode(event.id);

    if query::exists(tx, &id)? {
        record_seen(tx, &id, peer_pubkey, seen_at)?;
        return Ok(false);
    }

    if query::is_deleted(tx, event)? {
        return Ok(false);
    }

    if let Some(address) = event.address()
        && !replaces_current(tx, event, &address)?
    {
        return Ok(false);
    }

    insert(tx, event, &id)?;
    events::notify(tx, EventChange::Stored(Box::new(event.clone())));
    record_seen(tx, &id, peer_pubkey, seen_at)?;

    if event.kind == KIND_DELETE {
        apply_deletion(tx, event)?;
    }

    Ok(true)
}

/// Record that an event was seen from a peer, if that pair is not already
/// recorded. Returns whether this was a new sighting.
///
/// Provenance rows are written once and never updated: an event's seen time is
/// the earliest of them, so a later sighting from the same peer changes
/// nothing. This never leaves the device — see `docs/privacy.md`.
///
/// # Errors
///
/// If the write fails, including when no such event is stored.
pub fn record_seen(
    tx: &Tx<'_>,
    event_id: &str,
    peer_pubkey: &PublicKey,
    seen_at: i64,
) -> Result<bool> {
    let written = tx
        .prepare_cached(
            "INSERT OR IGNORE INTO event_seen (event_id, peer_pubkey, seen_at)
             VALUES (?1, ?2, ?3)",
        )?
        .execute(params![event_id, peer_pubkey.to_hex(), seen_at])
        .with_context(|| format!("recording {event_id} seen from {peer_pubkey}"))?;

    if written == 0 {
        return Ok(false);
    }

    events::notify(
        tx,
        EventChange::Seen(Provenance {
            event_id: event_id.to_string(),
            peer_pubkey: *peer_pubkey,
            seen_at,
        }),
    );

    Ok(true)
}

/// Remove an event and everything hanging off it. Returns whether it was there.
///
/// The tag, provenance, proof and blob rows go by cascade; the full-text row is
/// deleted by hand, since a virtual table has no foreign keys.
pub fn delete(tx: &Tx<'_>, id: &str) -> Result<bool> {
    tx.prepare_cached("DELETE FROM event_fts WHERE event_id = ?1")?
        .execute(params![id])
        .with_context(|| format!("removing {id} from the search index"))?;

    let removed = tx
        .prepare_cached("DELETE FROM event WHERE id = ?1")?
        .execute(params![id])
        .with_context(|| format!("deleting event {id}"))?;

    if removed == 0 {
        return Ok(false);
    }

    events::notify(tx, EventChange::Deleted(id.to_string()));

    Ok(true)
}

/// Forget events first seen before `cutoff`. Returns how many went.
pub fn forget_seen_before(tx: &Tx<'_>, cutoff: i64) -> Result<usize> {
    let mut prepared =
        tx.prepare("SELECT event_id FROM event_seen GROUP BY event_id HAVING MIN(seen_at) < ?1")?;

    let stale = prepared
        .query_map(params![cutoff], |row| row.get::<_, String>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()
        .context("finding events to forget")?;

    let mut forgotten = 0;

    for id in stale {
        if delete(tx, &id)? {
            forgotten += 1;
        }
    }

    Ok(forgotten)
}

/// Whether `event` supersedes whatever currently holds its address, removing
/// the current one if so.
///
/// NIP-01's rule: the later `created_at` wins, and the lower id breaks a tie.
fn replaces_current(tx: &Tx<'_>, event: &HashedEvent, address: &Address) -> Result<bool> {
    let Some(current) = query::by_address(tx, address)? else {
        return Ok(true);
    };

    let supersedes = event.created_at > current.created_at
        || (event.created_at == current.created_at && event.id < current.id);

    if supersedes {
        delete(tx, &hex::encode(current.id))?;
    }

    Ok(supersedes)
}

/// Write the event and the two indexes over it.
fn insert(tx: &Tx<'_>, event: &HashedEvent, id: &str) -> Result<()> {
    let tags = serde_json::to_string(&event.tags).context("serializing tags")?;

    tx.prepare_cached(
        "INSERT INTO event (id, pubkey, created_at, kind, tags, content, address)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
    )?
    .execute(params![
        id,
        event.pubkey.to_hex(),
        event.created_at,
        event.kind,
        tags,
        event.content,
        event.address().map(|address| address.to_string()),
    ])
    .with_context(|| format!("storing event {id}"))?;

    let mut tag_insert = tx.prepare_cached(
        "INSERT INTO event_tag (event_id, position, name, value) VALUES (?1, ?2, ?3, ?4)",
    )?;

    for (position, tag) in event.tags.iter().enumerate() {
        if !is_indexed_tag(tag.name()) || tag.len() < 2 {
            continue;
        }

        tag_insert
            .execute(params![
                id,
                i64::try_from(position).unwrap_or(i64::MAX),
                tag.name(),
                tag.value()
            ])
            .with_context(|| format!("indexing tag {} on {id}", tag.name()))?;
    }

    if !event.content.is_empty() {
        tx.prepare_cached("INSERT INTO event_fts (event_id, content) VALUES (?1, ?2)")?
            .execute(params![id, event.content])
            .with_context(|| format!("indexing content of {id}"))?;
    }

    Ok(())
}

/// Apply a kind 5, deleting the events it names that its author wrote.
///
/// An author can only delete their own work, so both branches test the deleting
/// event's pubkey. Addressable targets are only deleted up to the deletion's
/// own timestamp, so a republished address survives its own tombstone.
fn apply_deletion(tx: &Tx<'_>, deletion: &HashedEvent) -> Result<()> {
    for id in deletion.tags.values("e") {
        let Some(target) = query::get(tx, id)? else {
            continue;
        };

        if target.pubkey == deletion.pubkey {
            delete(tx, &hex::encode(target.id))?;
        }
    }

    for value in deletion.tags.values("a") {
        // A tag value this device cannot parse names nothing it can hold.
        let Ok(address) = value.parse::<Address>() else {
            continue;
        };

        let Some(target) = query::by_address(tx, &address)? else {
            continue;
        };

        if target.pubkey == deletion.pubkey && target.created_at <= deletion.created_at {
            delete(tx, &hex::encode(target.id))?;
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use coracle_lib::filters::Filter;
    use coracle_lib::tags::Tags;

    use crate::db::open_in_memory;
    use crate::fixtures::{author, event, id, note, peer};
    use crate::model::{EventFilter, ProvenanceFilter};

    /// A query narrowed by a NIP-01 filter and nothing else.
    fn matching(filter: Filter) -> EventFilter {
        EventFilter::new().with_filter(filter)
    }

    /// Everything the store holds.
    fn everything() -> EventFilter {
        EventFilter::new()
    }

    #[test]
    fn saving_indexes_tags_and_content() {
        let mut connection = open_in_memory().unwrap();
        let tx = Tx::begin_write(&mut connection).unwrap();

        let tags = Tags::new()
            .add("t", ["town"])
            .add("imeta", ["url https://example.com/x.jpg"]);
        let subject = note(author(1), 100, "hello neighbor", tags);

        assert!(save(&tx, &subject, &peer(), 10).unwrap());

        let by_tag = query::list(
            &tx,
            &matching(Filter::new().add_tag(coracle_lib::filters::TagMatch::Any, "t", "town")),
        )
        .unwrap();
        assert_eq!(by_tag, vec![subject.clone()]);

        let by_search = query::list(&tx, &matching(Filter::new().add_search("neighbor"))).unwrap();
        assert_eq!(by_search, vec![subject.clone()]);

        // Multi-character tags are not filterable under NIP-01, so they stay
        // out of the index and are read back from the event itself.
        let indexed: i64 = tx
            .query_row("SELECT COUNT(*) FROM event_tag", [], |row| row.get(0))
            .unwrap();
        assert_eq!(indexed, 1);
        assert_eq!(
            query::get(&tx, &id(&subject)).unwrap().unwrap().tags,
            subject.tags
        );
    }

    #[test]
    fn saving_twice_records_a_sighting_but_not_a_second_event() {
        let mut connection = open_in_memory().unwrap();
        let tx = Tx::begin_write(&mut connection).unwrap();

        let subject = note(author(1), 100, "note", Tags::new());
        let other = author(3);

        assert!(save(&tx, &subject, &peer(), 10).unwrap());
        assert!(!save(&tx, &subject, &other, 20).unwrap());
        assert!(!save(&tx, &subject, &other, 30).unwrap());

        assert_eq!(query::count(&tx, &everything()).unwrap(), 1);
        assert_eq!(
            query::seen_from(&tx, &id(&subject)).unwrap(),
            vec![peer(), other]
        );
        // The earliest sighting is the seen time, and a later one from the
        // same peer does not move it.
        assert_eq!(query::seen_at(&tx, &id(&subject)).unwrap(), Some(10));
    }

    #[test]
    fn ephemeral_events_are_never_stored() {
        let mut connection = open_in_memory().unwrap();
        let tx = Tx::begin_write(&mut connection).unwrap();

        let auth = event(author(1), 22_242, 100, "", Tags::new());

        assert!(!save(&tx, &auth, &peer(), 10).unwrap());
        assert_eq!(query::count(&tx, &everything()).unwrap(), 0);
    }

    #[test]
    fn a_replaceable_event_keeps_one_row_per_address() {
        let mut connection = open_in_memory().unwrap();
        let tx = Tx::begin_write(&mut connection).unwrap();

        let older = event(author(1), 0, 100, "{}", Tags::new());
        let newer = event(author(1), 0, 200, "{\"name\":\"me\"}", Tags::new());

        assert!(save(&tx, &older, &peer(), 10).unwrap());
        assert!(save(&tx, &newer, &peer(), 20).unwrap());

        assert_eq!(query::count(&tx, &everything()).unwrap(), 1);
        assert!(query::get(&tx, &id(&newer)).unwrap().is_some());

        // An older version arriving afterwards does not resurrect itself.
        let oldest = event(author(1), 0, 50, "{\"name\":\"old\"}", Tags::new());
        assert!(!save(&tx, &oldest, &peer(), 30).unwrap());
        assert_eq!(query::count(&tx, &everything()).unwrap(), 1);
    }

    #[test]
    fn addressable_events_are_keyed_on_their_identifier() {
        let mut connection = open_in_memory().unwrap();
        let tx = Tx::begin_write(&mut connection).unwrap();

        let first = event(author(1), 30_023, 100, "one", Tags::new().add("d", ["one"]));
        let second = event(author(1), 30_023, 200, "two", Tags::new().add("d", ["two"]));

        save(&tx, &first, &peer(), 10).unwrap();
        save(&tx, &second, &peer(), 20).unwrap();

        // Different identifiers, so neither replaces the other.
        assert_eq!(query::count(&tx, &everything()).unwrap(), 2);

        let replacement = event(
            author(1),
            30_023,
            300,
            "again",
            Tags::new().add("d", ["one"]),
        );
        save(&tx, &replacement, &peer(), 30).unwrap();

        assert_eq!(query::count(&tx, &everything()).unwrap(), 2);
        assert!(query::get(&tx, &id(&first)).unwrap().is_none());
    }

    #[test]
    fn a_deletion_removes_the_authors_own_events_only() {
        let mut connection = open_in_memory().unwrap();
        let tx = Tx::begin_write(&mut connection).unwrap();

        let mine = note(author(1), 100, "mine", Tags::new());
        let theirs = note(author(2), 100, "theirs", Tags::new());

        save(&tx, &mine, &peer(), 10).unwrap();
        save(&tx, &theirs, &peer(), 10).unwrap();

        let deletion = event(
            author(1),
            KIND_DELETE,
            200,
            "",
            Tags::new().add("e", [id(&mine)]).add("e", [id(&theirs)]),
        );
        save(&tx, &deletion, &peer(), 20).unwrap();

        assert!(query::get(&tx, &id(&mine)).unwrap().is_none());
        assert!(query::get(&tx, &id(&theirs)).unwrap().is_some());
    }

    #[test]
    fn a_deletion_by_address_removes_the_slot() {
        let mut connection = open_in_memory().unwrap();
        let tx = Tx::begin_write(&mut connection).unwrap();

        let article = event(
            author(1),
            30_023,
            100,
            "draft",
            Tags::new().add("d", ["slug"]),
        );
        save(&tx, &article, &peer(), 10).unwrap();

        let address = article.address().unwrap().to_string();
        let deletion = event(
            author(1),
            KIND_DELETE,
            200,
            "",
            Tags::new().add("a", [address]),
        );
        save(&tx, &deletion, &peer(), 20).unwrap();

        assert!(query::get(&tx, &id(&article)).unwrap().is_none());
    }

    #[test]
    fn a_deleted_event_is_not_taken_back() {
        let mut connection = open_in_memory().unwrap();
        let tx = Tx::begin_write(&mut connection).unwrap();

        let subject = note(author(1), 100, "deleted", Tags::new());
        let deletion = event(
            author(1),
            KIND_DELETE,
            200,
            "",
            Tags::new().add("e", [id(&subject)]),
        );

        save(&tx, &deletion, &peer(), 10).unwrap();

        // The deletion arrived first, which is ordinary here: two peers hand
        // over what they have in whatever order they meet.
        assert!(!save(&tx, &subject, &peer(), 20).unwrap());
        assert!(query::get(&tx, &id(&subject)).unwrap().is_none());
    }

    #[test]
    fn deleting_an_event_takes_its_indexes_with_it() {
        let mut connection = open_in_memory().unwrap();
        let tx = Tx::begin_write(&mut connection).unwrap();

        let subject = note(author(1), 100, "note", Tags::new().add("t", ["town"]));

        save(&tx, &subject, &peer(), 10).unwrap();
        assert!(delete(&tx, &id(&subject)).unwrap());

        for table in ["event_tag", "event_seen", "event_fts"] {
            let rows: i64 = tx
                .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
                    row.get(0)
                })
                .unwrap();
            assert_eq!(rows, 0, "{table} kept a row for a deleted event");
        }
    }

    #[test]
    fn forgetting_goes_by_seen_time_not_created_at() {
        let mut connection = open_in_memory().unwrap();
        let tx = Tx::begin_write(&mut connection).unwrap();

        let old_but_fresh = note(author(1), 1, "old news, just in", Tags::new());
        let new_but_stale = note(author(1), 10_000, "yesterday's future", Tags::new());

        save(&tx, &old_but_fresh, &peer(), 900).unwrap();
        save(&tx, &new_but_stale, &peer(), 100).unwrap();

        assert_eq!(forget_seen_before(&tx, 500).unwrap(), 1);
        assert!(query::get(&tx, &id(&old_but_fresh)).unwrap().is_some());
        assert!(query::get(&tx, &id(&new_but_stale)).unwrap().is_none());
    }

    #[test]
    fn local_queries_narrow_by_peer_and_seen_time() {
        let mut connection = open_in_memory().unwrap();
        let tx = Tx::begin_write(&mut connection).unwrap();

        let early = note(author(1), 100, "early", Tags::new());
        let late = note(author(1), 100, "late", Tags::new());
        let other = author(3);

        save(&tx, &early, &peer(), 100).unwrap();
        save(&tx, &late, &other, 200).unwrap();

        let from_peer = query::list(
            &tx,
            &everything().with_provenance(ProvenanceFilter::new().add_peers([peer()])),
        )
        .unwrap();
        assert_eq!(from_peer, vec![early.clone()]);

        let recent = query::list(&tx, &everything().add_seen_since(150)).unwrap();
        assert_eq!(recent, vec![late.clone()]);

        // Ordered by arrival, not by the timestamp a peer claimed.
        let all = query::list(&tx, &everything()).unwrap();
        assert_eq!(all, vec![late, early]);
    }

    #[test]
    fn a_notification_carries_the_stored_event() {
        let mut connection = open_in_memory().unwrap();
        let mut changes = events::subscribe();

        let tx = Tx::begin_write(&mut connection).unwrap();
        let subject = note(author(1), 100, "announced", Tags::new());
        save(&tx, &subject, &peer(), 10).unwrap();

        assert!(changes.try_recv().is_err(), "notified before the commit");

        tx.commit().unwrap();

        match changes.try_recv() {
            Ok(EventChange::Stored(stored)) => assert_eq!(*stored, subject),
            other => panic!("expected the stored event, got {other:?}"),
        }
    }
}
