//! Reads over `event`, `event_tag`, `event_fts` and `event_seen`.

use std::collections::BTreeSet;

use anyhow::{Context, Result};
use coracle_lib::addresses::{Address, EventExtensionAddress};
use coracle_lib::events::HashedEvent;
use coracle_lib::filters::{Filter, TagMatch};
use coracle_lib::keys::PublicKey;
use coracle_lib::search::SearchQuery;
use coracle_lib::tags::Tags;
use rusqlite::types::Value;
use rusqlite::{Row, params, params_from_iter};

use crate::db::Tx;
use crate::db::condition::{Conditions, text};
use crate::db::sql::{bytes_from_sql, pubkey_from_sql};
use crate::model::{
    Authors, EventCategory, EventFilter, Order, PeerPolicy, Provenance, Register, Registers, Seen,
};

/// The event columns, in the order [`to_event`] reads them.
const COLUMNS: &str = "e.id, e.pubkey, e.created_at, e.kind, e.tags, e.content";

/// The subquery for an event's seen time: the earliest sighting of it.
const SEEN_AT: &str = "(SELECT MIN(s.seen_at) FROM event_seen s WHERE s.event_id = e.id)";

/// One event by id.
pub fn get(tx: &Tx<'_>, id: &str) -> Result<Option<HashedEvent>> {
    let event = tx
        .prepare_cached(&format!("SELECT {COLUMNS} FROM event e WHERE e.id = ?1"))?
        .query_row(params![id], to_event)
        .map(Some)
        .or_else(none_if_missing)
        .with_context(|| format!("loading event {id}"))?;

    Ok(event)
}

/// Whether an event is stored, without loading it. The hot path on ingest.
pub fn exists(tx: &Tx<'_>, id: &str) -> Result<bool> {
    let exists = tx
        .prepare_cached("SELECT EXISTS (SELECT 1 FROM event WHERE id = ?1)")?
        .query_row(params![id], |row| row.get::<_, bool>(0))
        .with_context(|| format!("checking for event {id}"))?;

    Ok(exists)
}

/// Events matching every constraint on `filter`.
pub fn list(tx: &Tx<'_>, filter: &EventFilter) -> Result<Vec<HashedEvent>> {
    if filter.matches_nothing() {
        return Ok(Vec::new());
    }

    let mut conditions = conditions(filter);
    let tail = match filter.order {
        Order::CreatedAt => "ORDER BY e.created_at DESC, e.id ASC".to_string(),
        Order::SeenAt => format!("ORDER BY {SEEN_AT} DESC, e.id ASC"),
    };
    let sql = statement(COLUMNS, &mut conditions, &tail, filter.filter.limit);

    let mut prepared = tx.prepare(&sql)?;
    let events = prepared
        .query_map(params_from_iter(conditions.params()), to_event)?
        .collect::<rusqlite::Result<Vec<_>>>()
        .context("listing events")?;

    Ok(events)
}

/// How many events match.
pub fn count(tx: &Tx<'_>, filter: &EventFilter) -> Result<usize> {
    if filter.matches_nothing() {
        return Ok(0);
    }

    let mut conditions = conditions(filter);
    let sql = statement("COUNT(*)", &mut conditions, "", None);

    let count: i64 = tx
        .prepare(&sql)?
        .query_row(params_from_iter(conditions.params()), |row| row.get(0))
        .context("counting events")?;

    Ok(usize::try_from(count).unwrap_or(0))
}

/// The current event at an address.
pub fn by_address(tx: &Tx<'_>, address: &Address) -> Result<Option<HashedEvent>> {
    let event = tx
        .prepare_cached(&format!(
            "SELECT {COLUMNS} FROM event e
             WHERE e.address = ?1
             ORDER BY e.created_at DESC, e.id ASC
             LIMIT 1"
        ))?
        .query_row(params![address.to_string()], to_event)
        .map(Some)
        .or_else(none_if_missing)
        .with_context(|| format!("loading event at {address}"))?;

    Ok(event)
}

/// When an event was first seen.
pub fn seen_at(tx: &Tx<'_>, id: &str) -> Result<Option<i64>> {
    let seen_at = tx
        .prepare_cached("SELECT MIN(seen_at) FROM event_seen WHERE event_id = ?1")?
        .query_row(params![id], |row| row.get::<_, Option<i64>>(0))
        .with_context(|| format!("reading seen time for {id}"))?;

    Ok(seen_at)
}

/// Every sighting of an event, earliest first.
pub fn provenance(tx: &Tx<'_>, id: &str) -> Result<Vec<Provenance>> {
    let mut prepared = tx.prepare_cached(
        "SELECT event_id, peer_pubkey, seen_at FROM event_seen
         WHERE event_id = ?1
         ORDER BY seen_at ASC, peer_pubkey ASC",
    )?;

    let provenance = prepared
        .query_map(params![id], |row| {
            Ok(Provenance {
                event_id: row.get("event_id")?,
                peer_pubkey: pubkey_from_sql(&row.get::<_, String>("peer_pubkey")?, 1)?,
                seen_at: row.get("seen_at")?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()
        .with_context(|| format!("reading provenance for {id}"))?;

    Ok(provenance)
}

/// Which peers an event has been seen from.
pub fn seen_from(tx: &Tx<'_>, id: &str) -> Result<Vec<PublicKey>> {
    Ok(provenance(tx, id)?
        .into_iter()
        .map(|sighting| sighting.peer_pubkey)
        .collect())
}

/// Whether the author has already asked for this event to be deleted.
pub fn is_deleted(tx: &Tx<'_>, event: &HashedEvent) -> Result<bool> {
    let id = hex::encode(event.id);
    let deleted = tx
        .prepare_cached(
            "SELECT EXISTS (
                 SELECT 1 FROM event d
                 JOIN event_tag t ON t.event_id = d.id
                 WHERE d.kind = 5
                   AND d.pubkey = ?1
                   AND (
                     (t.name = 'e' AND t.value = ?2)
                     OR (t.name = 'a' AND t.value = ?3 AND d.created_at >= ?4)
                   )
             )",
        )?
        .query_row(
            params![
                event.pubkey.to_hex(),
                id,
                event.address().map(|address| address.to_string()),
                event.created_at,
            ],
            |row| row.get::<_, bool>(0),
        )
        .with_context(|| format!("checking whether {id} is deleted"))?;

    Ok(deleted)
}

/// Assemble a statement from its parts. The limit binds after every other
/// parameter, which is why this takes the conditions by mutable reference.
fn statement(
    columns: &str,
    conditions: &mut Conditions,
    tail: &str,
    limit: Option<usize>,
) -> String {
    let limit = limit.map(|limit| {
        let index = conditions.bind(Value::Integer(i64::try_from(limit).unwrap_or(i64::MAX)));

        format!("LIMIT ?{index}")
    });

    [
        format!("SELECT {columns} FROM event e"),
        conditions.where_clause(),
        tail.to_string(),
        limit.unwrap_or_default(),
    ]
    .into_iter()
    .filter(|part| !part.is_empty())
    .collect::<Vec<_>>()
    .join(" ")
}

/// Compile every constraint on an [`EventFilter`] into SQL.
fn conditions(filter: &EventFilter) -> Conditions {
    let mut conditions = Conditions::new();

    push_filter(&mut conditions, &filter.filter);
    push_seen(&mut conditions, &filter.seen);

    if let Some(registers) = &filter.registers {
        push_registers(&mut conditions, registers);
    }

    if let Some(policy) = &filter.policy {
        push_policy(&mut conditions, policy);
    }

    conditions
}

/// Compile a NIP-01 filter.
fn push_filter(conditions: &mut Conditions, filter: &Filter) {
    if let Some(ids) = &filter.ids {
        conditions.push_set("e.id", ids.iter().map(|id| text(hex::encode(id))).collect());
    }

    if let Some(authors) = &filter.authors {
        conditions.push_set(
            "e.pubkey",
            authors.iter().map(|author| text(author.to_hex())).collect(),
        );
    }

    if let Some(kinds) = &filter.kinds {
        conditions.push_set(
            "e.kind",
            kinds
                .iter()
                .map(|kind| Value::Integer((*kind).into()))
                .collect(),
        );
    }

    if let Some(since) = filter.since {
        let index = conditions.bind(Value::Integer(since));

        conditions.push(format!("e.created_at >= ?{index}"));
    }

    if let Some(until) = filter.until {
        let index = conditions.bind(Value::Integer(until));

        conditions.push(format!("e.created_at <= ?{index}"));
    }

    for (key, values) in &filter.tags {
        push_tag(conditions, key, values);
    }

    if let Some(search) = &filter.search {
        match fts_query(search) {
            // A search of nothing but punctuation and spaces asked for
            // something no document has, rather than for everything.
            None => conditions.push_never(),
            Some(query) => {
                let index = conditions.bind(Value::Text(query));

                conditions.push(format!(
                    "e.id IN (SELECT event_id FROM event_fts WHERE event_fts MATCH ?{index})"
                ));
            }
        }
    }
}

/// Compile one tag constraint. `Any` is a single lookup against the tag index;
/// `All` is one per value, since a row carries a single value.
///
/// The map's keys carry the wire prefix `TagMatch` produced — `#` for NIP-01,
/// `&` for NIP-91 — so reading one back is splitting that character off.
fn push_tag(conditions: &mut Conditions, key: &str, values: &BTreeSet<String>) {
    let (mode, name) = match key.split_at_checked(1) {
        Some(("#", name)) => (TagMatch::Any, name),
        Some(("&", name)) => (TagMatch::All, name),
        // Not a key any builder produces, so it constrains nothing that can be
        // stored. Matching nothing is the honest reading.
        _ => {
            conditions.push_never();
            return;
        }
    };

    if values.is_empty() {
        match mode {
            // Membership of the empty set. `matches_nothing` catches this
            // before the query is built; the clause is here so compiling a
            // filter is correct on its own.
            TagMatch::Any => conditions.push_never(),
            // Every value of an empty list is present in any event, so this
            // requires nothing. The builder drops such a constraint; a filter
            // off the wire may still carry one.
            TagMatch::All => {}
        }

        return;
    }

    match mode {
        TagMatch::Any => {
            let name_index = conditions.bind(text(name));
            let placeholders = conditions.bind_all(values.iter().map(text));

            conditions.push(format!(
                "EXISTS (SELECT 1 FROM event_tag t
                         WHERE t.event_id = e.id AND t.name = ?{name_index} AND t.value IN ({placeholders}))"
            ));
        }
        TagMatch::All => {
            for value in values {
                let name_index = conditions.bind(text(name));
                let value_index = conditions.bind(text(value));

                conditions.push(format!(
                    "EXISTS (SELECT 1 FROM event_tag t
                             WHERE t.event_id = e.id AND t.name = ?{name_index} AND t.value = ?{value_index})"
                ));
            }
        }
    }
}

/// Compile the local seen criteria. Time bounds read the earliest sighting;
/// the peer constraint reads any of them, since seeing an event from someone
/// later is still having seen it from them.
fn push_seen(conditions: &mut Conditions, seen: &Seen) {
    if let Some(since) = seen.since {
        let index = conditions.bind(Value::Integer(since));

        conditions.push(format!("{SEEN_AT} >= ?{index}"));
    }

    if let Some(until) = seen.until {
        let index = conditions.bind(Value::Integer(until));

        conditions.push(format!("{SEEN_AT} <= ?{index}"));
    }

    if let Some(peers) = &seen.peers {
        if peers.is_empty() {
            conditions.push_never();
            return;
        }

        let placeholders = conditions.bind_all(peers.iter().map(|peer| text(peer.to_hex())));

        conditions.push(format!(
            "EXISTS (SELECT 1 FROM event_seen s
                     WHERE s.event_id = e.id AND s.peer_pubkey IN ({placeholders}))"
        ));
    }
}

/// Compile the register constraint.
fn push_registers(conditions: &mut Conditions, registers: &Registers) {
    if registers.matches_nothing() {
        conditions.push_never();
        return;
    }

    let index = conditions.bind(text(registers.identity.to_hex()));

    let signed = format!(
        "EXISTS (SELECT 1 FROM proof p WHERE p.event_id = e.id AND p.recipient_pubkey = ?{index})"
    );

    let disjuncts = registers
        .registers
        .iter()
        .map(|register| match register {
            Register::Own => format!("e.pubkey = ?{index}"),
            Register::Forwardable => format!("(e.pubkey <> ?{index} AND {signed})"),
            Register::Held => format!("(e.pubkey <> ?{index} AND NOT {signed})"),
        })
        .collect::<Vec<_>>()
        .join(" OR ");

    conditions.push(format!("({disjuncts})"));
}

/// Compile the policy governing the peer being answered.
fn push_policy(conditions: &mut Conditions, policy: &PeerPolicy) {
    if policy.is_blocked() {
        conditions.push_never();
        return;
    }

    let hex = |author: &PublicKey| text(author.to_hex());

    match policy.gossip_authors() {
        Authors::Any => {}
        Authors::Only(authors) => {
            conditions.push_set("e.pubkey", authors.iter().map(hex).collect())
        }
        Authors::Except(authors) => {
            conditions.push_excluded("e.pubkey", authors.iter().map(hex).collect());
        }
    }

    for category in EventCategory::ALL {
        if !policy.sees(category) {
            push_hidden(conditions, policy.identity(), category);
        }
    }
}

/// Exclude the user's own events of a category this peer may not see.
fn push_hidden(conditions: &mut Conditions, identity: &PublicKey, category: EventCategory) {
    let complement = category == EventCategory::Content;
    let kinds = if complement {
        EventCategory::named_kinds()
    } else {
        category.kinds()
    };

    if kinds.is_empty() && !complement {
        return;
    }

    let index = conditions.bind(text(identity.to_hex()));

    // Every kind is named by another category, so the complement is empty and
    // the whole of the user's own output is hidden.
    if kinds.is_empty() {
        conditions.push(format!("e.pubkey <> ?{index}"));
        return;
    }

    let placeholders =
        conditions.bind_all(kinds.into_iter().map(|kind| Value::Integer(kind.into())));
    let membership = if complement { "NOT IN" } else { "IN" };

    conditions.push(format!(
        "NOT (e.pubkey = ?{index} AND e.kind {membership} ({placeholders}))"
    ));
}

/// Turn a NIP-50 query into an FTS5 one.
fn fts_query(search: &str) -> Option<String> {
    let terms: Vec<String> = SearchQuery::parse(search)
        .terms
        .iter()
        .map(|term| format!("\"{}\"", term.replace('"', "\"\"")))
        .collect();

    (!terms.is_empty()).then(|| terms.join(" AND "))
}

/// Read a row into an event, parsing the tags column back into an array.
fn to_event(row: &Row<'_>) -> rusqlite::Result<HashedEvent> {
    let tags: String = row.get("tags")?;

    Ok(HashedEvent {
        id: bytes_from_sql(&row.get::<_, String>("id")?, 0)?,
        pubkey: pubkey_from_sql(&row.get::<_, String>("pubkey")?, 1)?,
        created_at: row.get("created_at")?,
        kind: row.get("kind")?,
        tags: serde_json::from_str::<Tags>(&tags).map_err(|error| {
            rusqlite::Error::FromSqlConversionFailure(
                4,
                rusqlite::types::Type::Text,
                Box::new(error),
            )
        })?,
        content: row.get("content")?,
    })
}

/// Turn "no rows" into `None`, leaving every other failure alone.
fn none_if_missing<T>(error: rusqlite::Error) -> rusqlite::Result<Option<T>> {
    match error {
        rusqlite::Error::QueryReturnedNoRows => Ok(None),
        other => Err(other),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use coracle_lib::events::HasId;

    use crate::db::event::command;
    use crate::db::open_in_memory;
    use crate::db::proof::command as proof;
    use crate::fixtures::{author, event, id, note, peer};
    use crate::model::{KIND_MUTE, KIND_PROFILE, Policy, Proof, Scope};

    /// A query narrowed by a NIP-01 filter and nothing else.
    fn matching(filter: Filter) -> EventFilter {
        EventFilter::new().with_filter(filter)
    }

    /// Everything the store holds.
    fn everything() -> EventFilter {
        EventFilter::new()
    }

    fn store(tx: &Tx<'_>, seed: u8, created_at: i64, tags: Tags) {
        let event = note(author(seed), created_at, "note", tags);

        command::save(tx, &event, &peer(), created_at).unwrap();
    }

    /// Store an event and the author's signature naming `recipient`, which is
    /// what puts it in the forwardable register.
    fn store_signed(tx: &Tx<'_>, event: &HashedEvent, recipient: &PublicKey) {
        command::save(tx, event, &peer(), event.created_at).unwrap();
        proof::save(
            tx,
            &Proof {
                event_id: id(event),
                recipient_pubkey: *recipient,
                sig: [7u8; 64],
            },
        )
        .unwrap();
    }

    #[test]
    fn constraints_combine_and_results_are_newest_first() {
        let mut connection = open_in_memory().unwrap();
        let tx = Tx::begin_write(&mut connection).unwrap();

        store(&tx, 1, 100, Tags::new());
        store(&tx, 1, 200, Tags::new());
        store(&tx, 2, 300, Tags::new());

        let all = list(&tx, &everything()).unwrap();
        assert_eq!(all.len(), 3);
        assert_eq!(all[0].created_at, 300);
        assert_eq!(all[2].created_at, 100);

        let mine = list(&tx, &matching(Filter::new().add_author(author(1)))).unwrap();
        assert_eq!(mine.len(), 2);

        let window = list(&tx, &matching(Filter::new().add_since(150).add_until(250))).unwrap();
        assert_eq!(window.len(), 1);
        assert_eq!(window[0].created_at, 200);

        // Fields are ANDed: an author and a window that do not overlap match
        // nothing rather than either one.
        let neither = list(
            &tx,
            &matching(
                Filter::new()
                    .add_author(author(2))
                    .add_since(0)
                    .add_until(150),
            ),
        )
        .unwrap();
        assert!(neither.is_empty());

        assert_eq!(
            list(&tx, &matching(Filter::new().add_limit(2)))
                .unwrap()
                .len(),
            2
        );
    }

    #[test]
    fn an_empty_set_matches_nothing() {
        let mut connection = open_in_memory().unwrap();
        let tx = Tx::begin_write(&mut connection).unwrap();

        store(&tx, 1, 100, Tags::new());

        // The distinction that matters for filter composition: no constraint
        // matches everything, a constraint on the empty set matches nothing.
        assert_eq!(list(&tx, &everything()).unwrap().len(), 1);
        assert!(
            list(&tx, &matching(Filter::new().add_authors([])))
                .unwrap()
                .is_empty()
        );
        assert!(
            list(&tx, &matching(Filter::new().add_kinds([])))
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn tag_constraints_honor_their_mode() {
        let mut connection = open_in_memory().unwrap();
        let tx = Tx::begin_write(&mut connection).unwrap();

        store(&tx, 1, 100, Tags::new().add("t", ["town"]));
        store(
            &tx,
            1,
            200,
            Tags::new().add("t", ["town"]).add("t", ["market"]),
        );

        // NIP-01: any of the listed values.
        let any = list(
            &tx,
            &matching(Filter::new().add_tags(TagMatch::Any, "t", ["town", "farm"])),
        )
        .unwrap();
        assert_eq!(any.len(), 2);

        // NIP-91: every one of them.
        let all = list(
            &tx,
            &matching(Filter::new().add_tags(TagMatch::All, "t", ["town", "market"])),
        )
        .unwrap();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].created_at, 200);
    }

    #[test]
    fn an_empty_tag_constraint_means_opposite_things_per_mode() {
        let mut connection = open_in_memory().unwrap();
        let tx = Tx::begin_write(&mut connection).unwrap();

        store(&tx, 1, 100, Tags::new().add("t", ["town"]));

        // The builder drops an empty `All`, so this is a filter that arrived
        // off the wire. Every value of an empty list is present in any event,
        // so it requires nothing and everything matches.
        let mut requires_nothing = Filter::new();
        requires_nothing
            .tags
            .insert("&t".to_string(), BTreeSet::new());
        assert!(!requires_nothing.matches_nothing());
        assert_eq!(list(&tx, &matching(requires_nothing)).unwrap().len(), 1);

        // An empty `Any` is membership of the empty set, which nothing
        // satisfies — and the library says so before the query is built.
        let mut matches_nothing = Filter::new();
        matches_nothing
            .tags
            .insert("#t".to_string(), BTreeSet::new());
        assert!(matches_nothing.matches_nothing());
        assert!(list(&tx, &matching(matches_nothing)).unwrap().is_empty());
    }

    #[test]
    fn an_impossible_window_is_not_asked_of_the_database() {
        let mut connection = open_in_memory().unwrap();
        let tx = Tx::begin_write(&mut connection).unwrap();

        store(&tx, 1, 100, Tags::new());

        let impossible = matching(Filter::new().add_since(200).add_until(100));

        assert!(list(&tx, &impossible).unwrap().is_empty());
        assert_eq!(count(&tx, &impossible).unwrap(), 0);

        assert!(
            list(
                &tx,
                &everything().with_seen(Seen::new().add_since(200).add_until(100))
            )
            .unwrap()
            .is_empty()
        );
    }

    #[test]
    fn a_search_query_is_taken_literally() {
        let mut connection = open_in_memory().unwrap();
        let tx = Tx::begin_write(&mut connection).unwrap();

        command::save(
            &tx,
            &note(author(1), 100, "hello neighbor", Tags::new()),
            &peer(),
            100,
        )
        .unwrap();

        assert_eq!(
            list(&tx, &matching(Filter::new().add_search("neighbor")))
                .unwrap()
                .len(),
            1
        );

        // FTS5 has operators of its own, and a search string can arrive in a
        // peer's REQ. Neither of these is a syntax error, and neither runs as
        // an operator: they are terms that match nothing.
        for hostile in ["hello AND (", "\"unbalanced", "  "] {
            assert!(
                list(&tx, &matching(Filter::new().add_search(hostile)))
                    .unwrap()
                    .is_empty(),
                "{hostile:?} was not taken literally"
            );
        }
    }

    #[test]
    fn a_register_is_own_signed_or_neither() {
        let mut connection = open_in_memory().unwrap();
        let tx = Tx::begin_write(&mut connection).unwrap();

        let us = author(1);
        let mine = note(us, 100, "mine", Tags::new());
        let signed = note(author(2), 200, "signed over to us", Tags::new());
        let held = note(author(3), 300, "no signature came with this", Tags::new());

        command::save(&tx, &mine, &us, 100).unwrap();
        store_signed(&tx, &signed, &us);
        command::save(&tx, &held, &peer(), 300).unwrap();

        let ids = |registers: Registers| {
            list(&tx, &everything().with_registers(registers))
                .unwrap()
                .iter()
                .map(id)
                .collect::<BTreeSet<_>>()
        };

        assert_eq!(ids(Registers::new(us, [Register::Own])), [id(&mine)].into());
        assert_eq!(
            ids(Registers::new(us, [Register::Forwardable])),
            [id(&signed)].into()
        );
        assert_eq!(
            ids(Registers::new(us, [Register::Held])),
            [id(&held)].into()
        );

        // What this device can put on the wire at all: the two registers that
        // travel, and nothing else.
        assert_eq!(
            ids(Registers::offerable(us)),
            [id(&mine), id(&signed)].into()
        );
        assert!(ids(Registers::new(us, [])).is_empty());

        // The signature names a recipient, and is worth nothing under another
        // one: the same event is unforwardable from a device it does not name.
        assert!(ids(Registers::new(author(9), [Register::Forwardable])).is_empty());
    }

    #[test]
    fn a_register_constraint_is_applied_before_the_limit() {
        let mut connection = open_in_memory().unwrap();
        let tx = Tx::begin_write(&mut connection).unwrap();

        let us = author(1);

        // Three events this device cannot forward, newer than the one it can.
        // A constraint applied to the page rather than to the query would
        // return nothing and call it the end of the set.
        command::save(&tx, &note(us, 100, "mine", Tags::new()), &us, 100).unwrap();

        for created_at in [200, 300, 400] {
            store(&tx, 2, created_at, Tags::new());
        }

        let page = list(
            &tx,
            &matching(Filter::new().add_limit(2)).with_registers(Registers::offerable(us)),
        )
        .unwrap();

        assert_eq!(page.len(), 1);
        assert_eq!(page[0].created_at, 100);
    }

    #[test]
    fn a_policy_narrows_by_author_and_by_visibility() {
        let mut connection = open_in_memory().unwrap();
        let tx = Tx::begin_write(&mut connection).unwrap();

        let us = author(1);
        let mut policy = Policy::new(us);

        policy.graph.trusted.insert(author(2));
        policy.graph.blocked.insert(author(4));

        let profile = event(us, KIND_PROFILE, 100, "", Tags::new());
        let mutes = event(us, KIND_MUTE, 200, "", Tags::new());
        let ours = note(us, 300, "ours", Tags::new());
        let trusted = note(author(2), 400, "trusted", Tags::new());
        let stranger = note(author(9), 500, "stranger", Tags::new());
        let blocked = note(author(4), 600, "blocked", Tags::new());

        for held in [&profile, &mutes, &ours, &trusted, &stranger, &blocked] {
            command::save(&tx, held, &peer(), held.created_at).unwrap();
        }

        let served = |policy: Policy, to: PublicKey| {
            list(&tx, &everything().with_policy(policy.for_peer(to)))
                .unwrap()
                .iter()
                .map(id)
                .collect::<BTreeSet<_>>()
        };

        // The defaults: gossip reaches the trusted tier and no further, and
        // metadata is for trusted peers only.
        assert_eq!(
            served(policy.clone(), author(9)),
            [id(&profile), id(&ours), id(&trusted)].into()
        );
        assert_eq!(
            served(policy.clone(), author(2)),
            [id(&profile), id(&mutes), id(&ours), id(&trusted)].into()
        );

        // A blocked peer is served nothing at all.
        assert!(served(policy.clone(), author(4)).is_empty());

        // Nothing gossips only the user's own, still by category.
        let mut silent = policy.clone();
        silent.gossip = Scope::Nothing;
        assert_eq!(served(silent, author(9)), [id(&profile), id(&ours)].into());

        // Lenient gossips everyone but the blocked.
        let mut lenient = policy.clone();
        lenient.gossip = Scope::Lenient;
        assert_eq!(
            served(lenient, author(9)),
            [id(&profile), id(&ours), id(&trusted), id(&stranger)].into()
        );

        // Hiding content leaves the user's other categories alone, and touches
        // nobody else's events.
        let mut private = policy;
        private.content_visibility = Scope::Nothing;
        private.gossip = Scope::Lenient;
        assert_eq!(
            served(private, author(9)),
            [id(&profile), id(&trusted), id(&stranger)].into()
        );
    }

    #[test]
    fn a_query_orders_by_arrival_when_asked_to() {
        let mut connection = open_in_memory().unwrap();
        let tx = Tx::begin_write(&mut connection).unwrap();

        // An author whose clock is days out, handed over after a note that
        // claims to be older.
        let stale = note(author(1), 100, "stale", Tags::new());
        let fresh = note(author(2), 900, "fresh", Tags::new());

        command::save(&tx, &fresh, &peer(), 10).unwrap();
        command::save(&tx, &stale, &peer(), 20).unwrap();

        let by_claim = list(&tx, &everything()).unwrap();
        assert_eq!(by_claim[0].id, *fresh.id());

        let by_arrival = list(&tx, &everything().with_order(Order::SeenAt)).unwrap();
        assert_eq!(by_arrival[0].id, *stale.id());

        let since = list(&tx, &everything().add_seen_since(15)).unwrap();
        assert_eq!(since.len(), 1);
        assert_eq!(since[0].id, *stale.id());
    }
}
