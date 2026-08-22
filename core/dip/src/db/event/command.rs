//! Writes over `event`, `event_tag`, `event_fts` and `event_seen`.
//!
//! [`save`] is the whole of ingest below the policy layer: it enforces what the
//! tables mean to each other

use anyhow::{Context, Result};
use coracle_kinds::delete::{self, DeleteReader};
use coracle_lib::addresses::{Address, EventExtensionAddress};
use coracle_lib::events::{EventId, HashedEvent};
use coracle_lib::keys::PublicKey;
use coracle_lib::kinds::is_ephemeral;
use coracle_lib::readers::Reader;
use rusqlite::params;

use crate::db::Tx;
use crate::db::sql::event_id_from_sql;
use crate::model::Provenance;

use super::channel::{self, EventChange};
use super::query;

/// Store an event, and record a sighting of it from each pubkey in `from`.
/// Returns whether the event is new to this device.
///
/// Both come from the same session: a peer may prove more than one identity,
/// and each of them is a true sighting. A locally authored event goes through
/// here too, naming the author's own pubkey in `from`, so every stored event
/// has provenance and a seen time.
///
/// Nothing is stored when the event is ephemeral, when its author has already
/// deleted it, or when a newer event holds its address. A sighting is still
/// recorded for an event already stored.
pub fn save(tx: &Tx<'_>, event: &HashedEvent, from: &[PublicKey], seen_at: i64) -> Result<bool> {
    if is_ephemeral(event.kind) {
        return Ok(false);
    }

    let id = event.id.to_hex();

    if query::exists(tx, &event.id)? {
        record_seen_all(tx, &event.id, from, seen_at)?;
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

    insert(tx, event, &id, seen_at)?;
    channel::notify(tx, EventChange::Stored(Box::new(event.clone())));
    record_seen_all(tx, &event.id, from, seen_at)?;

    if event.kind == delete::KIND {
        apply_deletion(tx, event)?;
    }

    Ok(true)
}

/// Record a sighting of an event per pubkey a peer proved on one session.
fn record_seen_all(
    tx: &Tx<'_>,
    event_id: &EventId,
    from: &[PublicKey],
    seen_at: i64,
) -> Result<()> {
    for pubkey in from {
        record_seen(tx, event_id, pubkey, seen_at)?;
    }

    Ok(())
}

/// Record that an event was seen from a peer, if that pair is not already
/// recorded. Returns whether this was a new sighting.
///
/// The one place a sighting is written, so it is the one place `event.seen_at`
/// is kept at the earliest of them.
pub fn record_seen(
    tx: &Tx<'_>,
    event_id: &EventId,
    pubkey: &PublicKey,
    seen_at: i64,
) -> Result<bool> {
    let written = tx
        .prepare_cached(
            "INSERT OR IGNORE INTO event_seen (event_id, pubkey, seen_at)
             VALUES (?1, ?2, ?3)",
        )?
        .execute(params![event_id.to_hex(), pubkey.to_hex(), seen_at])
        .with_context(|| format!("recording {event_id} seen from {pubkey}"))?;

    if written == 0 {
        return Ok(false);
    }

    // Lowers, never sets. The row is born with the sighting it arrived on, so
    // this only fires for one recorded out of order — which would otherwise
    // move the arrival time forward, and leave the column disagreeing with the
    // rows it stands for.
    tx.prepare_cached("UPDATE event SET seen_at = ?2 WHERE id = ?1 AND seen_at > ?2")?
        .execute(params![event_id.to_hex(), seen_at])
        .with_context(|| format!("recording the arrival of {event_id}"))?;

    channel::notify(
        tx,
        EventChange::Seen(Provenance {
            event_id: *event_id,
            pubkey: *pubkey,
            seen_at,
        }),
    );

    Ok(true)
}

/// Remove an event and everything hanging off it. Returns whether it was there.
///
/// The tag, provenance, signature and blob rows go by cascade; the full-text row is
/// deleted by hand, since a virtual table has no foreign keys.
pub fn delete(tx: &Tx<'_>, id: &EventId) -> Result<bool> {
    tx.prepare_cached("DELETE FROM event_fts WHERE event_id = ?1")?
        .execute(params![id.to_hex()])
        .with_context(|| format!("removing {id} from the search index"))?;

    let removed = tx
        .prepare_cached("DELETE FROM event WHERE id = ?1")?
        .execute(params![id.to_hex()])
        .with_context(|| format!("deleting event {id}"))?;

    if removed == 0 {
        return Ok(false);
    }

    channel::notify(tx, EventChange::Deleted(id.to_string()));

    Ok(true)
}

/// Forget events first seen before `cutoff`. Returns how many went.
pub fn forget_seen_before(tx: &Tx<'_>, cutoff: i64) -> Result<usize> {
    let mut prepared = tx.prepare("SELECT id FROM event WHERE seen_at < ?1")?;

    let stale = prepared
        .query_map(params![cutoff], |row| {
            event_id_from_sql(&row.get::<_, String>(0)?, 0)
        })?
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
fn replaces_current(tx: &Tx<'_>, event: &HashedEvent, address: &Address) -> Result<bool> {
    let Some(current) = query::by_address(tx, address)? else {
        return Ok(true);
    };

    let supersedes = event.created_at > current.created_at
        || (event.created_at == current.created_at && event.id < current.id);

    if supersedes {
        delete(tx, &current.id)?;
    }

    Ok(supersedes)
}

/// Write the event and the two indexes over it.
///
/// `seen_at` is the sighting this event arrived on, which is its earliest by
/// construction — [`record_seen`] lowers it if one ever turns up before it.
fn insert(tx: &Tx<'_>, event: &HashedEvent, id: &str, seen_at: i64) -> Result<()> {
    let tags = serde_json::to_string(&event.tags).context("serializing tags")?;

    tx.prepare_cached(
        "INSERT INTO event (id, pubkey, created_at, kind, tags, content, address, seen_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
    )?
    .execute(params![
        id,
        event.pubkey.to_hex(),
        event.created_at,
        event.kind,
        tags,
        event.content,
        event.address().map(|address| address.to_string()),
        seen_at,
    ])
    .with_context(|| format!("storing event {id}"))?;

    let mut tag_insert = tx.prepare_cached(
        "INSERT INTO event_tag (event_id, position, name, value) VALUES (?1, ?2, ?3, ?4)",
    )?;

    for (position, tag) in event.tags.iter().enumerate() {
        // Single-letter alphanumeric names are the ones a NIP-01 filter can
        // name, so they are the set worth indexing. Everything else is read
        // back from the event's own tags.
        let name = tag.name();
        let indexed = name.len() == 1 && name.chars().all(|c| c.is_ascii_alphanumeric());

        if !indexed || tag.len() < 2 {
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
fn apply_deletion(tx: &Tx<'_>, event: &HashedEvent) -> Result<()> {
    let Ok(deletion) = DeleteReader::read(event) else {
        return Ok(());
    };

    for id in deletion.ids() {
        let Some(target) = query::get(tx, id)? else {
            continue;
        };

        if deletion.matches(&target) {
            delete(tx, &target.id)?;
        }
    }

    for address in deletion.addresses() {
        let Some(target) = query::by_address(tx, address)? else {
            continue;
        };

        if deletion.matches(&target) {
            delete(tx, &target.id)?;
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use coracle_lib::filters::Filter;
    use coracle_lib::tags::Tags;

    use crate::db::Db;
    use crate::fixtures::{author, event, id, note, peer};
    use crate::model::{ProvenanceFilter, Query};

    /// A query narrowed by a NIP-01 filter and nothing else.
    fn matching(filter: Filter) -> Query {
        Query::new().with_filter(filter)
    }

    /// Everything the store holds.
    fn everything() -> Query {
        Query::new()
    }

    #[test]
    fn saving_indexes_tags_and_content() {
        let mut db = Db::open_in_memory().unwrap();
        let tx = db.begin_write().unwrap();

        let tags = Tags::new()
            .add("t", ["town"])
            .add("A", ["upper"])
            // Single character, but not alphanumeric, so not filterable.
            .add("-", ["protected"])
            .add("imeta", ["url https://example.com/x.jpg"]);
        let subject = note(author(1), 100, "hello neighbor", tags);

        assert!(save(&tx, &subject, &[peer()], 10).unwrap());

        let by_tag = query::list(
            &tx,
            &matching(Filter::new().add_tag(coracle_lib::filters::TagMatch::Any, "t", "town")),
        )
        .unwrap();
        assert_eq!(by_tag, vec![subject.clone()]);

        let by_search = query::list(&tx, &matching(Filter::new().add_search("neighbor"))).unwrap();
        assert_eq!(by_search, vec![subject.clone()]);

        // Only `t` and `A` are filterable under NIP-01. The rest stay out of
        // the index and are read back from the event itself.
        let indexed: Vec<String> = tx
            .prepare("SELECT name FROM event_tag ORDER BY name")
            .unwrap()
            .query_map([], |row| row.get(0))
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap();
        assert_eq!(indexed, ["A", "t"]);
        assert_eq!(
            query::get(&tx, &id(&subject)).unwrap().unwrap().tags,
            subject.tags
        );
    }

    #[test]
    fn saving_twice_records_a_sighting_but_not_a_second_event() {
        let mut db = Db::open_in_memory().unwrap();
        let tx = db.begin_write().unwrap();

        let subject = note(author(1), 100, "note", Tags::new());
        let other = author(3);

        assert!(save(&tx, &subject, &[peer()], 10).unwrap());
        assert!(!save(&tx, &subject, &[other], 20).unwrap());
        assert!(!save(&tx, &subject, &[other], 30).unwrap());

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
        let mut db = Db::open_in_memory().unwrap();
        let tx = db.begin_write().unwrap();

        let auth = event(author(1), 22_242, 100, "", Tags::new());

        assert!(!save(&tx, &auth, &[peer()], 10).unwrap());
        assert_eq!(query::count(&tx, &everything()).unwrap(), 0);
    }

    #[test]
    fn a_replaceable_event_keeps_one_row_per_address() {
        let mut db = Db::open_in_memory().unwrap();
        let tx = db.begin_write().unwrap();

        let older = event(author(1), 0, 100, "{}", Tags::new());
        let newer = event(author(1), 0, 200, "{\"name\":\"me\"}", Tags::new());

        assert!(save(&tx, &older, &[peer()], 10).unwrap());
        assert!(save(&tx, &newer, &[peer()], 20).unwrap());

        assert_eq!(query::count(&tx, &everything()).unwrap(), 1);
        assert!(query::get(&tx, &id(&newer)).unwrap().is_some());

        // An older version arriving afterwards does not resurrect itself.
        let oldest = event(author(1), 0, 50, "{\"name\":\"old\"}", Tags::new());
        assert!(!save(&tx, &oldest, &[peer()], 30).unwrap());
        assert_eq!(query::count(&tx, &everything()).unwrap(), 1);
    }

    #[test]
    fn addressable_events_are_keyed_on_their_identifier() {
        let mut db = Db::open_in_memory().unwrap();
        let tx = db.begin_write().unwrap();

        let first = event(author(1), 30_023, 100, "one", Tags::new().add("d", ["one"]));
        let second = event(author(1), 30_023, 200, "two", Tags::new().add("d", ["two"]));

        save(&tx, &first, &[peer()], 10).unwrap();
        save(&tx, &second, &[peer()], 20).unwrap();

        // Different identifiers, so neither replaces the other.
        assert_eq!(query::count(&tx, &everything()).unwrap(), 2);

        let replacement = event(
            author(1),
            30_023,
            300,
            "again",
            Tags::new().add("d", ["one"]),
        );
        save(&tx, &replacement, &[peer()], 30).unwrap();

        assert_eq!(query::count(&tx, &everything()).unwrap(), 2);
        assert!(query::get(&tx, &id(&first)).unwrap().is_none());
    }

    #[test]
    fn a_deletion_removes_the_authors_own_events_only() {
        let mut db = Db::open_in_memory().unwrap();
        let tx = db.begin_write().unwrap();

        let mine = note(author(1), 100, "mine", Tags::new());
        let theirs = note(author(2), 100, "theirs", Tags::new());

        save(&tx, &mine, &[peer()], 10).unwrap();
        save(&tx, &theirs, &[peer()], 10).unwrap();

        let deletion = event(
            author(1),
            delete::KIND,
            200,
            "",
            Tags::new()
                .add("e", [id(&mine).to_hex()])
                .add("e", [id(&theirs).to_hex()]),
        );
        save(&tx, &deletion, &[peer()], 20).unwrap();

        assert!(query::get(&tx, &id(&mine)).unwrap().is_none());
        assert!(query::get(&tx, &id(&theirs)).unwrap().is_some());
    }

    #[test]
    fn a_deletion_by_address_removes_the_slot() {
        let mut db = Db::open_in_memory().unwrap();
        let tx = db.begin_write().unwrap();

        let article = event(
            author(1),
            30_023,
            100,
            "draft",
            Tags::new().add("d", ["slug"]),
        );
        save(&tx, &article, &[peer()], 10).unwrap();

        let address = article.address().unwrap().to_string();
        let deletion = event(
            author(1),
            delete::KIND,
            200,
            "",
            Tags::new().add("a", [address]),
        );
        save(&tx, &deletion, &[peer()], 20).unwrap();

        assert!(query::get(&tx, &id(&article)).unwrap().is_none());
    }

    #[test]
    fn a_deleted_event_is_not_taken_back() {
        let mut db = Db::open_in_memory().unwrap();
        let tx = db.begin_write().unwrap();

        let subject = note(author(1), 100, "deleted", Tags::new());
        let deletion = event(
            author(1),
            delete::KIND,
            200,
            "",
            Tags::new().add("e", [id(&subject).to_hex()]),
        );

        save(&tx, &deletion, &[peer()], 10).unwrap();

        // The deletion arrived first, which is ordinary here: two peers hand
        // over what they have in whatever order they meet.
        assert!(!save(&tx, &subject, &[peer()], 20).unwrap());
        assert!(query::get(&tx, &id(&subject)).unwrap().is_none());
    }

    #[test]
    fn deleting_an_event_takes_its_indexes_with_it() {
        let mut db = Db::open_in_memory().unwrap();
        let tx = db.begin_write().unwrap();

        let subject = note(author(1), 100, "note", Tags::new().add("t", ["town"]));

        save(&tx, &subject, &[peer()], 10).unwrap();
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
        let mut db = Db::open_in_memory().unwrap();
        let tx = db.begin_write().unwrap();

        let old_but_fresh = note(author(1), 1, "old news, just in", Tags::new());
        let new_but_stale = note(author(1), 10_000, "yesterday's future", Tags::new());

        save(&tx, &old_but_fresh, &[peer()], 900).unwrap();
        save(&tx, &new_but_stale, &[peer()], 100).unwrap();

        assert_eq!(forget_seen_before(&tx, 500).unwrap(), 1);
        assert!(query::get(&tx, &id(&old_but_fresh)).unwrap().is_some());
        assert!(query::get(&tx, &id(&new_but_stale)).unwrap().is_none());
    }

    /// `event.seen_at` is a cache of the earliest row in `event_seen`, and the
    /// two are read by different queries — so nothing catches them disagreeing
    /// except asking both.
    #[test]
    fn an_events_arrival_time_is_the_earliest_of_its_sightings() {
        let mut db = Db::open_in_memory().unwrap();
        let tx = db.begin_write().unwrap();

        let subject = note(author(1), 100, "carried around", Tags::new());
        let stored = id(&subject);

        let cached = || {
            tx.query_row(
                "SELECT seen_at FROM event WHERE id = ?1",
                [stored.to_hex()],
                |row| row.get::<_, Option<i64>>(0),
            )
            .unwrap()
        };
        let earliest = || {
            tx.query_row(
                "SELECT MIN(seen_at) FROM event_seen WHERE event_id = ?1",
                [stored.to_hex()],
                |row| row.get::<_, Option<i64>>(0),
            )
            .unwrap()
        };

        save(&tx, &subject, &[author(2)], 500).unwrap();
        assert_eq!(cached(), Some(500));

        // A later sighting leaves the arrival time alone.
        save(&tx, &subject, &[author(3)], 900).unwrap();
        assert_eq!(cached(), Some(500));

        // One that arrives out of order lowers it, so the cache and the rows it
        // came from cannot part company.
        save(&tx, &subject, &[author(4)], 200).unwrap();
        assert_eq!(cached(), Some(200));

        // Recording the same pair twice changes neither.
        record_seen(&tx, &stored, &author(4), 50).unwrap();
        assert_eq!(cached(), Some(200));

        assert_eq!(cached(), earliest());
        assert_eq!(query::seen_at(&tx, &stored).unwrap(), Some(200));
        assert_eq!(query::provenance(&tx, &stored).unwrap().len(), 3);
    }

    #[test]
    fn local_queries_narrow_by_peer_and_seen_time() {
        let mut db = Db::open_in_memory().unwrap();
        let tx = db.begin_write().unwrap();

        let early = note(author(1), 100, "early", Tags::new());
        let late = note(author(1), 100, "late", Tags::new());
        let other = author(3);

        save(&tx, &early, &[peer()], 100).unwrap();
        save(&tx, &late, &[other], 200).unwrap();

        let seen_from = query::list(
            &tx,
            &everything().with_provenance(ProvenanceFilter::new().add_pubkeys([peer()])),
        )
        .unwrap();
        assert_eq!(seen_from, vec![early.clone()]);

        let recent = query::list(&tx, &everything().add_seen_since(150)).unwrap();
        assert_eq!(recent, vec![late.clone()]);

        // Ordered by arrival, not by the timestamp a peer claimed.
        let all = query::list(&tx, &everything()).unwrap();
        assert_eq!(all, vec![late, early]);
    }

    #[test]
    fn a_notification_carries_the_stored_event() {
        let mut db = Db::open_in_memory().unwrap();
        let mut changes = channel::subscribe(&db);

        let tx = db.begin_write().unwrap();
        let subject = note(author(1), 100, "announced", Tags::new());
        save(&tx, &subject, &[peer()], 10).unwrap();

        assert!(changes.try_recv().is_err(), "notified before the commit");

        tx.commit().unwrap();

        match changes.try_recv() {
            Ok(EventChange::Stored(stored)) => assert_eq!(*stored, subject),
            other => panic!("expected the stored event, got {other:?}"),
        }
    }
}
