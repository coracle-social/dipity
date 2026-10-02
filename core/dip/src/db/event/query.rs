//! Reads over `event`, `event_tag`, `event_fts`, `event_seen` and
//! `event_shared`.

use std::collections::{BTreeSet, HashMap};

use anyhow::{Context, Result};
use coracle_kinds::delete::{self, DeleteReader};
use coracle_lib::addresses::{Address, EventExtensionAddress};
use coracle_lib::events::{EventId, HashedEvent};
use coracle_lib::filters::{Filter, TagMatch};
use coracle_lib::keys::PublicKey;
use coracle_lib::readers::Reader;
use coracle_lib::search::SearchQuery;
use coracle_lib::tags::Tags;
use rusqlite::types::Value;
use rusqlite::{Row, params, params_from_iter};

use crate::db::Tx;
use crate::db::condition::{Conditions, text};
use crate::db::sql::{event_id_from_sql, placeholders, pubkey_from_sql};
use crate::model::{
    Authors, Order, PeerPolicy, Provenance, ProvenanceFilter, Query, Register, Registers, Scope,
    Share,
};

/// The event columns, in the order [`to_event`] reads them.
const COLUMNS: &str = "e.id, e.pubkey, e.created_at, e.kind, e.tags, e.content";

/// One event by id.
pub fn get(tx: &Tx<'_>, id: &EventId) -> Result<Option<HashedEvent>> {
    let event = tx
        .prepare_cached(&format!("SELECT {COLUMNS} FROM event e WHERE e.id = ?1"))?
        .query_row(params![id.to_hex()], to_event)
        .map(Some)
        .or_else(none_if_missing)
        .with_context(|| format!("loading event {id}"))?;

    Ok(event)
}

/// Every id this device refused, as the items reconciliation diffs.
pub fn refused(tx: &Tx<'_>) -> Result<Vec<coracle_lib::sync::Item>> {
    tx.prepare_cached("SELECT id, created_at FROM event_refused")?
        .query_map([], |row| {
            Ok(coracle_lib::sync::Item {
                id: event_id_from_sql(&row.get::<_, String>(0)?, 0)?,
                timestamp: row.get(1)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()
        .context("reading refused ids")
}

/// Whether an event is stored, without loading it. The hot path on ingest.
pub fn exists(tx: &Tx<'_>, id: &EventId) -> Result<bool> {
    let exists = tx
        .prepare_cached("SELECT EXISTS (SELECT 1 FROM event WHERE id = ?1)")?
        .query_row(params![id.to_hex()], |row| row.get::<_, bool>(0))
        .with_context(|| format!("checking for event {id}"))?;

    Ok(exists)
}

/// Events matching every constraint on `query`.
pub fn list(tx: &Tx<'_>, query: &Query) -> Result<Vec<HashedEvent>> {
    if query.matches_nothing() {
        return Ok(Vec::new());
    }

    let mut conditions = conditions(query);
    let tail = match query.order {
        Order::CreatedAt => "ORDER BY e.created_at DESC, e.id ASC",
        Order::SeenAt => "ORDER BY e.seen_at DESC, e.id ASC",
    };
    let sql = statement(COLUMNS, &mut conditions, tail, query.filter.limit);

    let mut prepared = tx.prepare(&sql)?;
    let events = prepared
        .query_map(params_from_iter(conditions.params), to_event)?
        .collect::<rusqlite::Result<Vec<_>>>()
        .context("listing events")?;

    Ok(events)
}

/// How many events match.
pub fn count(tx: &Tx<'_>, query: &Query) -> Result<usize> {
    if query.matches_nothing() {
        return Ok(0);
    }

    let mut conditions = conditions(query);
    let sql = statement("COUNT(*)", &mut conditions, "", None);

    let count: i64 = tx
        .prepare(&sql)?
        .query_row(params_from_iter(conditions.params), |row| row.get(0))
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

/// When an event was first seen. `None` for an event this device does not hold.
pub fn seen_at(tx: &Tx<'_>, id: &EventId) -> Result<Option<i64>> {
    let seen_at = tx
        .prepare_cached("SELECT seen_at FROM event WHERE id = ?1")?
        .query_row(params![id.to_hex()], |row| row.get::<_, i64>(0))
        .map(Some)
        .or_else(none_if_missing)
        .with_context(|| format!("reading seen time for {id}"))?;

    Ok(seen_at)
}

/// Every sighting of an event, earliest first.
pub fn provenance(tx: &Tx<'_>, id: &EventId) -> Result<Vec<Provenance>> {
    let mut prepared = tx.prepare_cached(
        "SELECT event_id, pubkey, seen_at FROM event_seen
         WHERE event_id = ?1
         ORDER BY seen_at ASC, pubkey ASC",
    )?;

    let provenance = prepared
        .query_map(params![id.to_hex()], to_provenance)?
        .collect::<rusqlite::Result<Vec<_>>>()
        .with_context(|| format!("reading provenance for {id}"))?;

    Ok(provenance)
}

/// Every sighting of any of `ids`, grouped by the event.
///
/// One query rather than one per event, because the view asks this of a whole
/// page at a time. An event this device does not hold is absent from the map.
pub fn provenance_for(tx: &Tx<'_>, ids: &[EventId]) -> Result<HashMap<EventId, Vec<Provenance>>> {
    if ids.is_empty() {
        return Ok(HashMap::new());
    }

    let placeholders = placeholders(1, ids.len());
    let mut prepared = tx.prepare(&format!(
        "SELECT event_id, pubkey, seen_at FROM event_seen
         WHERE event_id IN ({placeholders})
         ORDER BY seen_at ASC, pubkey ASC"
    ))?;

    // Ordered across the whole set, so each event's sightings keep `provenance` order.
    let mut sightings: HashMap<EventId, Vec<Provenance>> = HashMap::new();

    for sighting in prepared
        .query_map(
            params_from_iter(ids.iter().map(EventId::to_hex)),
            to_provenance,
        )?
        .collect::<rusqlite::Result<Vec<_>>>()
        .context("reading provenance for a page of events")?
    {
        sightings
            .entry(sighting.event_id)
            .or_default()
            .push(sighting);
    }

    Ok(sightings)
}

/// Every handoff of any of `ids`, grouped by the event.
///
/// The outbound half of [`provenance_for`], and one query for the same reason:
/// the view asks it of a whole page. An event this device has never handed on is
/// absent from the map.
pub fn shares_for(tx: &Tx<'_>, ids: &[EventId]) -> Result<HashMap<EventId, Vec<Share>>> {
    if ids.is_empty() {
        return Ok(HashMap::new());
    }

    let placeholders = placeholders(1, ids.len());
    let mut prepared = tx.prepare(&format!(
        "SELECT event_id, pubkey, shared_at FROM event_shared
         WHERE event_id IN ({placeholders})
         ORDER BY shared_at ASC, pubkey ASC"
    ))?;

    // Ordered across the whole set, so each event's handoffs keep that order.
    let mut shares: HashMap<EventId, Vec<Share>> = HashMap::new();

    for share in prepared
        .query_map(params_from_iter(ids.iter().map(EventId::to_hex)), to_share)?
        .collect::<rusqlite::Result<Vec<_>>>()
        .context("reading handoffs for a page of events")?
    {
        shares.entry(share.event_id).or_default().push(share);
    }

    Ok(shares)
}

/// The user's own events handed to any of `to` without the recipient signature.
pub fn unsigned_shares(
    tx: &Tx<'_>,
    identity: &PublicKey,
    to: &[PublicKey],
) -> Result<Vec<EventId>> {
    if to.is_empty() {
        return Ok(Vec::new());
    }

    let placeholders = placeholders(2, to.len());
    let mut prepared = tx.prepare(&format!(
        "SELECT DISTINCT event_shared.event_id FROM event_shared
         JOIN event ON event.id = event_shared.event_id
         WHERE event.pubkey = ?1 AND event_shared.signed = 0
           AND event_shared.pubkey IN ({placeholders})"
    ))?;

    let mut values = vec![identity.to_hex()];
    values.extend(to.iter().map(PublicKey::to_hex));

    prepared
        .query_map(params_from_iter(values), |row| {
            event_id_from_sql(&row.get::<_, String>(0)?, 0)
        })?
        .collect::<rusqlite::Result<Vec<_>>>()
        .context("finding own events handed over unsigned")
}

/// Every author the store holds an event from, other than the user.
pub fn authors(tx: &Tx<'_>, identity: &PublicKey) -> Result<Vec<PublicKey>> {
    tx.prepare_cached("SELECT DISTINCT pubkey FROM event WHERE pubkey <> ?1")?
        .query_map(params![identity.to_hex()], |row| {
            pubkey_from_sql(&row.get::<_, String>(0)?, 0)
        })?
        .collect::<rusqlite::Result<Vec<_>>>()
        .context("listing the authors held")
}

/// Which peers an event has been seen from.
pub fn seen_from(tx: &Tx<'_>, id: &EventId) -> Result<Vec<PublicKey>> {
    Ok(provenance(tx, id)?
        .into_iter()
        .map(|sighting| sighting.pubkey)
        .collect())
}

/// Whether a stored request has already asked for this event to be deleted.
///
/// The mirror of the sweep in [`command`](super::command), which runs when the
/// request is the event arriving. Both ask [`DeleteReader::matches`], so who
/// may speak for an event is decided in one place — `coracle-kinds`, where it
/// is authorship for most kinds and the recipient for a gift wrap, whose
/// signing key was thrown away. SQL narrows to the requests naming this event
/// and no further; a second copy of the rule here would be one the two
/// directions could disagree about.
pub fn is_deleted(tx: &Tx<'_>, event: &HashedEvent) -> Result<bool> {
    let id = event.id.to_hex();
    let requests = tx
        .prepare_cached(&format!(
            "SELECT DISTINCT {COLUMNS} FROM event e
             JOIN event_tag t ON t.event_id = e.id
             WHERE e.kind = ?3
               AND ((t.name = 'e' AND t.value = ?1) OR (t.name = 'a' AND t.value = ?2))"
        ))?
        .query_map(
            params![
                id,
                event.address().map(|address| address.to_string()),
                delete::KIND,
            ],
            to_event,
        )?
        .collect::<rusqlite::Result<Vec<_>>>()
        .with_context(|| format!("checking whether {id} is deleted"))?;

    Ok(requests
        .iter()
        .filter_map(|request| DeleteReader::read(request).ok())
        .any(|request| request.matches(event)))
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

/// Compile every constraint on a [`Query`] into SQL.
fn conditions(query: &Query) -> Conditions {
    let mut conditions = Conditions::new();

    push_filter(&mut conditions, &query.filter);
    push_provenance(&mut conditions, &query.provenance);

    if let Some(registers) = &query.registers {
        push_registers(&mut conditions, registers);
    }

    if let Some(policy) = &query.policy {
        push_policy(&mut conditions, policy);
    }

    conditions
}

/// Compile a NIP-01 filter.
fn push_filter(conditions: &mut Conditions, filter: &Filter) {
    if let Some(ids) = &filter.ids {
        conditions.push_set("e.id", ids.iter().map(|id| text(id.to_hex())).collect());
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
            // Nothing but punctuation asked for something no document has, not for everything.
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
        // Not a key any builder produces, so matching nothing is the honest reading.
        _ => {
            conditions.push_never();
            return;
        }
    };

    if values.is_empty() {
        match mode {
            // Membership of the empty set; `matches_nothing` catches it before the query.
            TagMatch::Any => conditions.push_never(),
            // Every value of an empty list is present in any event, so this requires nothing.
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

/// Compile the provenance criteria. Time bounds read the earliest sighting;
/// the peer constraint reads any of them, since seeing an event from someone
/// later is still having seen it from them.
fn push_provenance(conditions: &mut Conditions, provenance: &ProvenanceFilter) {
    if let Some(since) = provenance.since {
        let index = conditions.bind(Value::Integer(since));

        conditions.push(format!("e.seen_at >= ?{index}"));
    }

    if let Some(until) = provenance.until {
        let index = conditions.bind(Value::Integer(until));

        conditions.push(format!("e.seen_at <= ?{index}"));
    }

    if let Some(pubkeys) = &provenance.pubkeys {
        if pubkeys.is_empty() {
            conditions.push_never();
            return;
        }

        let placeholders = conditions.bind_all(pubkeys.iter().map(|pubkey| text(pubkey.to_hex())));

        conditions.push(format!(
            "EXISTS (SELECT 1 FROM event_seen s
                     WHERE s.event_id = e.id AND s.pubkey IN ({placeholders}))"
        ));
    }
}

/// Compile the register constraint.
fn push_registers(conditions: &mut Conditions, registers: &Registers) {
    if registers.matches_nothing() {
        conditions.push_never();
        return;
    }

    // Both registers are relative to who is asking, and a device may act as several.
    let bound = registers
        .identities
        .iter()
        .map(|identity| format!("?{}", conditions.bind(text(identity.to_hex()))))
        .collect::<Vec<_>>()
        .join(", ");

    let ours = format!("e.pubkey IN ({bound})");
    let signed = format!(
        "EXISTS (SELECT 1 FROM recipient_signature p WHERE p.event_id = e.id AND p.recipient_pubkey IN ({bound}))"
    );

    let disjuncts = registers
        .registers
        .iter()
        .map(|register| match register {
            Register::Own => ours.clone(),
            Register::Forwardable => format!("(NOT {ours} AND {signed})"),
            Register::Held => format!("(NOT {ours} AND NOT {signed})"),
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

    push_visibility(conditions, policy);
}

/// Exclude the user's own events this peer may not see.
fn push_visibility(conditions: &mut Conditions, policy: &PeerPolicy) {
    let visibility = &policy.policy.visibility;
    let standing = policy.standing;
    let hides = |scope: Scope| !scope.admits(standing);

    let depth = if hides(visibility.default) {
        visibility.rules.len()
    } else {
        visibility
            .rules
            .iter()
            .rposition(|rule| hides(rule.scope))
            .map_or(0, |last| last + 1)
    };

    let mut unmatched: Vec<String> = Vec::new();

    for rule in &visibility.rules[..depth] {
        let matched = conditions.group(|conditions| push_filter(conditions, &rule.filter));

        if hides(rule.scope) {
            let mut reached = unmatched.clone();

            reached.push(matched.clone());
            push_hidden(conditions, &policy.policy.identity, &reached);
        }

        unmatched.push(format!("NOT {matched}"));
    }

    if hides(visibility.default) {
        push_hidden(conditions, &policy.policy.identity, &unmatched);
    }
}

/// Exclude the user's own events that reach a rule hiding them from this peer,
/// which is those matching every one of `reached`.
fn push_hidden(conditions: &mut Conditions, identity: &PublicKey, reached: &[String]) {
    let index = conditions.bind(text(identity.to_hex()));

    if reached.is_empty() {
        conditions.push(format!("e.pubkey <> ?{index}"));
        return;
    }

    conditions.push(format!(
        "NOT (e.pubkey = ?{index} AND {})",
        reached.join(" AND ")
    ));
}

/// Turn a NIP-50 query into an FTS5 one.
fn fts_query(search: &str) -> Option<String> {
    let terms: Vec<String> = SearchQuery::parse(search)
        .terms
        .iter()
        // Each term matches as a prefix, so a search typed a letter at a time finds as it goes.
        .map(|term| format!("\"{}\"*", term.replace('"', "\"\"")))
        .collect();

    (!terms.is_empty()).then(|| terms.join(" AND "))
}

/// Read a row into one sighting.
fn to_provenance(row: &Row<'_>) -> rusqlite::Result<Provenance> {
    Ok(Provenance {
        event_id: event_id_from_sql(&row.get::<_, String>("event_id")?, 0)?,
        pubkey: pubkey_from_sql(&row.get::<_, String>("pubkey")?, 1)?,
        seen_at: row.get("seen_at")?,
    })
}

/// Read a row of `event_shared` into one handoff.
fn to_share(row: &Row<'_>) -> rusqlite::Result<Share> {
    Ok(Share {
        event_id: event_id_from_sql(&row.get::<_, String>("event_id")?, 0)?,
        pubkey: pubkey_from_sql(&row.get::<_, String>("pubkey")?, 1)?,
        shared_at: row.get("shared_at")?,
    })
}

/// Read a row into an event, parsing the tags column back into an array.
fn to_event(row: &Row<'_>) -> rusqlite::Result<HashedEvent> {
    let tags: String = row.get("tags")?;

    Ok(HashedEvent {
        id: event_id_from_sql(&row.get::<_, String>("id")?, 0)?,
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

    use coracle_kinds::profile;

    use crate::db::Db;
    use crate::db::event::command;
    use crate::db::recipient_signature::command as signature;
    use crate::fixtures::{author, event, id, note, peer};
    use crate::model::{MUTE, Policy, RecipientSignature, Scope, Visibility, VisibilityRule};

    /// A query narrowed by a NIP-01 filter and nothing else.
    fn matching(filter: Filter) -> Query {
        Query::new().with_filter(filter)
    }

    /// Everything the store holds.
    fn everything() -> Query {
        Query::new()
    }

    fn store(tx: &Tx<'_>, seed: u8, created_at: i64, tags: Tags) {
        let event = note(author(seed), created_at, "note", tags);

        command::save(tx, &event, &[peer()], created_at).unwrap();
    }

    /// Store an event and the author's signature naming `recipient`, which is
    /// what puts it in the forwardable register.
    fn store_signed(tx: &Tx<'_>, event: &HashedEvent, recipient: &PublicKey) {
        command::save(tx, event, &[peer()], event.created_at).unwrap();
        signature::save(
            tx,
            &RecipientSignature {
                event_id: id(event),
                author_pubkey: event.pubkey,
                recipient_pubkey: *recipient,
                sig: [7u8; 64],
            },
        )
        .unwrap();
    }

    #[test]
    fn constraints_combine_and_results_are_newest_first() {
        let mut db = Db::open_in_memory().unwrap();
        let tx = db.begin_write().unwrap();

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

        // Fields are ANDed: an author and a window that do not overlap match nothing.
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
        let mut db = Db::open_in_memory().unwrap();
        let tx = db.begin_write().unwrap();

        store(&tx, 1, 100, Tags::new());

        // No constraint matches everything; a constraint on the empty set matches nothing.
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
        let mut db = Db::open_in_memory().unwrap();
        let tx = db.begin_write().unwrap();

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
        let mut db = Db::open_in_memory().unwrap();
        let tx = db.begin_write().unwrap();

        store(&tx, 1, 100, Tags::new().add("t", ["town"]));

        // The builder drops an empty `All`, so this is a filter that arrived off the wire.
        let mut requires_nothing = Filter::new();
        requires_nothing
            .tags
            .insert("&t".to_string(), BTreeSet::new());
        assert!(!requires_nothing.matches_nothing());
        assert_eq!(list(&tx, &matching(requires_nothing)).unwrap().len(), 1);

        // An empty `Any` is membership of the empty set, and the library says so first.
        let matches_nothing = Filter::new().set_tag(TagMatch::Any, "t", Vec::<String>::new());
        assert!(matches_nothing.matches_nothing());
        assert!(list(&tx, &matching(matches_nothing)).unwrap().is_empty());
    }

    #[test]
    fn an_impossible_window_is_not_asked_of_the_database() {
        let mut db = Db::open_in_memory().unwrap();
        let tx = db.begin_write().unwrap();

        store(&tx, 1, 100, Tags::new());

        let impossible = matching(Filter::new().add_since(200).add_until(100));

        assert!(list(&tx, &impossible).unwrap().is_empty());
        assert_eq!(count(&tx, &impossible).unwrap(), 0);

        assert!(
            list(
                &tx,
                &everything()
                    .with_provenance(ProvenanceFilter::new().add_since(200).add_until(100))
            )
            .unwrap()
            .is_empty()
        );
    }

    #[test]
    fn a_search_query_is_taken_literally() {
        let mut db = Db::open_in_memory().unwrap();
        let tx = db.begin_write().unwrap();

        command::save(
            &tx,
            &note(author(1), 100, "hello neighbor", Tags::new()),
            &[peer()],
            100,
        )
        .unwrap();

        assert_eq!(
            list(&tx, &matching(Filter::new().add_search("neighbor")))
                .unwrap()
                .len(),
            1
        );

        // A word half typed already finds what it is the start of.
        assert_eq!(
            list(&tx, &matching(Filter::new().add_search("neigh")))
                .unwrap()
                .len(),
            1
        );

        // FTS5 operators can arrive in a peer's REQ; these are terms that match nothing.
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
        let mut db = Db::open_in_memory().unwrap();
        let tx = db.begin_write().unwrap();

        let us = author(1);
        let mine = note(us, 100, "mine", Tags::new());
        let signed = note(author(2), 200, "signed over to us", Tags::new());
        let held = note(author(3), 300, "no signature came with this", Tags::new());

        command::save(&tx, &mine, &[us], 100).unwrap();
        store_signed(&tx, &signed, &us);
        command::save(&tx, &held, &[peer()], 300).unwrap();

        let ids = |registers: Registers| {
            list(&tx, &everything().with_registers(registers))
                .unwrap()
                .iter()
                .map(id)
                .collect::<BTreeSet<_>>()
        };

        assert_eq!(
            ids(Registers::new(&[us], [Register::Own])),
            [id(&mine)].into()
        );
        assert_eq!(
            ids(Registers::new(&[us], [Register::Forwardable])),
            [id(&signed)].into()
        );
        assert_eq!(
            ids(Registers::new(&[us], [Register::Held])),
            [id(&held)].into()
        );

        // What this device can put on the wire at all: the two registers that travel.
        assert_eq!(
            ids(Registers::offerable(&[us])),
            [id(&mine), id(&signed)].into()
        );
        assert!(ids(Registers::new(&[us], [])).is_empty());

        // A signature is worth nothing under any recipient but the one it names.
        assert!(ids(Registers::new(&[author(9)], [Register::Forwardable])).is_empty());
    }

    #[test]
    fn a_register_constraint_is_applied_before_the_limit() {
        let mut db = Db::open_in_memory().unwrap();
        let tx = db.begin_write().unwrap();

        let us = author(1);

        // A constraint applied to the page rather than the query would call this the end.
        command::save(&tx, &note(us, 100, "mine", Tags::new()), &[us], 100).unwrap();

        for created_at in [200, 300, 400] {
            store(&tx, 2, created_at, Tags::new());
        }

        let page = list(
            &tx,
            &matching(Filter::new().add_limit(2)).with_registers(Registers::offerable(&[us])),
        )
        .unwrap();

        assert_eq!(page.len(), 1);
        assert_eq!(page[0].created_at, 100);
    }

    #[test]
    fn a_policy_narrows_by_author_and_by_visibility() {
        let mut db = Db::open_in_memory().unwrap();
        let tx = db.begin_write().unwrap();

        let us = author(1);
        let mut policy = Policy::new(us);

        policy.graph.trusted.insert(author(2));
        policy.graph.blocked.insert(author(4));

        let profile = event(us, profile::KIND, 100, "", Tags::new());
        let mutes = event(us, MUTE, 200, "", Tags::new());
        let ours = note(us, 300, "ours", Tags::new());
        let trusted = note(author(2), 400, "trusted", Tags::new());
        let stranger = note(author(9), 500, "stranger", Tags::new());
        let blocked = note(author(4), 600, "blocked", Tags::new());

        for held in [&profile, &mutes, &ours, &trusted, &stranger, &blocked] {
            command::save(&tx, held, &[peer()], held.created_at).unwrap();
        }

        let served = |policy: Policy, to: PublicKey| {
            list(&tx, &everything().with_policy(policy.for_pubkey(to)))
                .unwrap()
                .iter()
                .map(id)
                .collect::<BTreeSet<_>>()
        };

        // The defaults: gossip reaches the trusted tier, and metadata is trusted-only.
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

        // Nothing gossips only the user's own, still governed by visibility.
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

        // A rule hiding the user's notes leaves what earlier rules matched alone.
        let mut private = policy;
        private.visibility = Visibility {
            rules: vec![
                VisibilityRule {
                    filter: Filter::new().add_kinds([profile::KIND]),
                    scope: Scope::Public,
                },
                VisibilityRule {
                    filter: Filter::new(),
                    scope: Scope::Nothing,
                },
            ],
            default: Scope::Public,
        };
        private.gossip = Scope::Lenient;
        assert_eq!(
            served(private, author(9)),
            [id(&profile), id(&trusted), id(&stranger)].into()
        );
    }

    /// The SQL walk in [`push_visibility`] and the walk in
    /// [`Visibility::scope_for`] are the same rules read by two engines, and
    /// only one of them is reachable from a test that asks about an event. So
    /// this asks both about every event under every ordering of a rule set
    /// built to make order matter.
    #[test]
    fn the_compiled_rules_serve_exactly_what_the_rules_admit() {
        let mut db = Db::open_in_memory().unwrap();
        let tx = db.begin_write().unwrap();

        let us = author(1);
        let them = author(9);

        let events = [
            event(us, profile::KIND, 100, "", Tags::new()),
            event(us, MUTE, 200, "", Tags::new()),
            event(us, 1, 300, "plain", Tags::new()),
            event(us, 1, 400, "tagged", Tags::new().add("t", ["work"])),
            event(us, 30_023, 500, "long", Tags::new().add("d", ["post"])),
            // Someone else's, which visibility never governs.
            note(author(2), 600, "theirs", Tags::new()),
        ];

        for held in &events {
            command::save(&tx, held, &[peer()], held.created_at).unwrap();
        }

        let rules = [
            VisibilityRule {
                filter: Filter::new().add_kinds([profile::KIND]),
                scope: Scope::Public,
            },
            VisibilityRule {
                filter: Filter::new().add_tag(TagMatch::Any, "t", "work"),
                scope: Scope::Nothing,
            },
            VisibilityRule {
                filter: Filter::new().add_kinds([1, 30_023]),
                scope: Scope::Trusted,
            },
            VisibilityRule {
                filter: Filter::new(),
                scope: Scope::Lenient,
            },
        ];

        // Every ordering of the four, so a rule shadowing another is covered too.
        for order in permutations(rules.len()) {
            for default in [Scope::Nothing, Scope::Public] {
                let mut policy = Policy::new(us);

                policy.gossip = Scope::Lenient;
                policy.visibility = Visibility {
                    rules: order.iter().map(|index| rules[*index].clone()).collect(),
                    default,
                };

                let bound = policy.for_pubkey(them);
                let admitted: BTreeSet<EventId> = events
                    .iter()
                    .filter(|held| held.pubkey != us || bound.is_visible(*held))
                    .map(id)
                    .collect();

                let served: BTreeSet<EventId> = list(&tx, &everything().with_policy(bound.clone()))
                    .unwrap()
                    .iter()
                    .map(id)
                    .collect();

                assert_eq!(
                    served, admitted,
                    "rule order {order:?} with default {default:?} was compiled to a \
                     query disagreeing with the rules it came from"
                );
            }
        }
    }

    /// Every ordering of `n` indexes.
    fn permutations(n: usize) -> Vec<Vec<usize>> {
        if n == 0 {
            return vec![Vec::new()];
        }

        let mut orders = Vec::new();

        for shorter in permutations(n - 1) {
            for position in 0..n {
                let mut order = shorter.clone();

                order.insert(position, n - 1);
                orders.push(order);
            }
        }

        orders
    }

    /// The batched read has to answer exactly what the per-event one does,
    /// including the order within an event and the events that have nothing.
    #[test]
    fn provenance_batches_without_changing_the_answer() {
        let mut db = Db::open_in_memory().unwrap();
        let tx = db.begin_write().unwrap();

        let carried = note(author(1), 100, "carried around", Tags::new());
        let once = note(author(1), 200, "seen once", Tags::new());

        // Sightings recorded out of time order, so grouping cannot be passing rows through.
        command::save(&tx, &carried, &[author(5)], 900).unwrap();
        command::save(&tx, &once, &[peer()], 400).unwrap();
        command::save(&tx, &carried, &[author(3)], 300).unwrap();
        command::save(&tx, &carried, &[author(4)], 600).unwrap();

        let missing = id(&note(author(1), 300, "never stored", Tags::new()));
        let ids = [id(&carried), id(&once), missing];
        let batched = provenance_for(&tx, &ids).unwrap();

        for id in &ids {
            assert_eq!(
                batched.get(id).cloned().unwrap_or_default(),
                provenance(&tx, id).unwrap(),
                "batched provenance for {id} differs from the per-event read"
            );
        }

        // Earliest first within the event, and an event we hold nothing for is absent.
        assert_eq!(
            batched[&id(&carried)]
                .iter()
                .map(|sighting| sighting.seen_at)
                .collect::<Vec<_>>(),
            [300, 600, 900]
        );
        assert!(!batched.contains_key(&missing));

        assert!(provenance_for(&tx, &[]).unwrap().is_empty());
    }

    #[test]
    fn a_query_orders_by_arrival_when_asked_to() {
        let mut db = Db::open_in_memory().unwrap();
        let tx = db.begin_write().unwrap();

        // An author whose clock is days out, handed over after a note claiming to be older.
        let stale = note(author(1), 100, "stale", Tags::new());
        let fresh = note(author(2), 900, "fresh", Tags::new());

        command::save(&tx, &fresh, &[peer()], 10).unwrap();
        command::save(&tx, &stale, &[peer()], 20).unwrap();

        let by_claim = list(&tx, &everything()).unwrap();
        assert_eq!(by_claim[0].id, *fresh.id());

        let by_arrival = list(&tx, &everything().with_order(Order::SeenAt)).unwrap();
        assert_eq!(by_arrival[0].id, *stale.id());

        let since = list(&tx, &everything().add_seen_since(15)).unwrap();
        assert_eq!(since.len(), 1);
        assert_eq!(since[0].id, *stale.id());
    }
}
