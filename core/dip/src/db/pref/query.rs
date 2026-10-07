//! Reads over `pref`, and the [`Policy`] assembled from them.
//!
//! [`policy`] is the one the rest of the core calls. It reads every key
//! `docs/policy.md` defines, substitutes the document's defaults for the ones
//! never written, and resolves the contact graph the tiers are measured
//! against — which lives in events rather than in this table, since contact
//! cards, block and mute are things the user publishes.

use std::collections::BTreeSet;

use anyhow::{Context, Result};
use coracle_lib::addresses::Address;
use coracle_lib::keys::PublicKey;
use coracle_lib::readers::Reader;
use rusqlite::{Row, params};
use serde::de::DeserializeOwned;

use crate::db::Tx;
use crate::db::event::query as event;
use crate::model::{BLOCK, CONTACT, Graph, MUTE, PeopleListReader, Policy, Pref, Query, keys};
use coracle_lib::filters::Filter;

/// One preference's raw JSON value, or `None` if it has never been written —
/// which is how a default is expressed.
pub fn get(tx: &Tx<'_>, key: &str) -> Result<Option<String>> {
    let value = tx
        .prepare_cached("SELECT value FROM pref WHERE key = ?1")?
        .query_row(params![key], |row| row.get::<_, String>(0))
        .map(Some)
        .or_else(|error| match error {
            rusqlite::Error::QueryReturnedNoRows => Ok(None),
            other => Err(other),
        })
        .with_context(|| format!("reading preference {key}"))?;

    Ok(value)
}

/// One preference, decoded.
///
/// A value that does not decode is an error rather than a default: it means an
/// older build wrote a shape this one does not understand, and silently
/// substituting a default would quietly loosen a policy the user set.
pub fn get_as<T: DeserializeOwned>(tx: &Tx<'_>, key: &str) -> Result<Option<T>> {
    let Some(value) = get(tx, key)? else {
        return Ok(None);
    };

    let decoded =
        serde_json::from_str(&value).with_context(|| format!("decoding preference {key}"))?;

    Ok(Some(decoded))
}

/// Replace `setting` with the written preference, if there is one.
///
/// The unwritten case leaves the value alone rather than substituting a
/// default here, so the defaults live only in [`Policy::new`] and cannot drift
/// between the two.
fn override_with<T: DeserializeOwned>(tx: &Tx<'_>, key: &str, setting: &mut T) -> Result<()> {
    if let Some(value) = get_as(tx, key)? {
        *setting = value;
    }

    Ok(())
}

/// Every preference, by key.
pub fn all(tx: &Tx<'_>) -> Result<Vec<Pref>> {
    let mut prepared =
        tx.prepare_cached("SELECT key, value, updated_at FROM pref ORDER BY key ASC")?;

    let prefs = prepared
        .query_map([], to_pref)?
        .collect::<rusqlite::Result<Vec<_>>>()
        .context("listing preferences")?;

    Ok(prefs)
}

/// Everything the user has said about who gets what, ready to apply.
///
/// Read once per session and bound to each pubkey a peer proved with
/// [`Policy::for_pubkey`](crate::model::Policy::for_pubkey), rather than re-read
/// per event: a session asks the same questions of the same peer many times,
/// and the answers cannot change under it while it runs.
pub fn policy(tx: &Tx<'_>, identity: &PublicKey) -> Result<Policy> {
    let mut policy = Policy::new(*identity);

    override_with(tx, keys::QUIET_TIMES, &mut policy.quiet_times)?;
    override_with(
        tx,
        keys::DISCOVER_IN_BACKGROUND,
        &mut policy.discover_in_background,
    )?;
    override_with(tx, keys::ACCEPT, &mut policy.accept)?;
    override_with(tx, keys::SHARING, &mut policy.sharing)?;
    override_with(tx, keys::RETENTION_DAYS, &mut policy.retention_days)?;

    policy.graph = graph(tx, identity)?;

    Ok(policy)
}

/// The user's contact graph, from the cards and lists they have published.
///
/// `contacts` is everybody the user's own contact cards name, and `network`
/// is derived: everybody the contacts' cards name, over whichever of those
/// cards this device happens to hold — so a contact whose cards have not
/// arrived contributes nobody, and the tier grows as the graph does.
///
/// It stops there. Reading the cards of people in `network` would grow the
/// tier until it meant nothing, and a person two hops out is reachable rather
/// than known. `docs/policy.md#social-graph`.
///
/// Block is a veto over both tiers: a blocked person is neither a contact nor
/// in the network, and their cards add nobody, whatever else names them.
///
/// One query per contact, plus three. This runs when a session opens and when
/// a preference changes, not per event.
fn graph(tx: &Tx<'_>, identity: &PublicKey) -> Result<Graph> {
    let blocked = listed::<BLOCK>(tx, identity)?;
    let contacts: BTreeSet<PublicKey> =
        named(tx, identity)?.difference(&blocked).copied().collect();
    let mut network = BTreeSet::new();

    for pubkey in &contacts {
        network.extend(named(tx, pubkey)?.difference(&blocked));
    }

    network.remove(identity);

    Ok(Graph {
        contacts,
        network,
        blocked,
        muted: listed::<MUTE>(tx, identity)?,
        muted_topics: topics::<MUTE>(tx, identity)?,
    })
}

/// Everybody `pubkey` has named, which for the user is everybody they paired
/// with. A card emptied by forgetting somebody names nobody.
fn named(tx: &Tx<'_>, pubkey: &PublicKey) -> Result<BTreeSet<PublicKey>> {
    let cards = event::list(
        tx,
        &Query::new().with_filter(Filter::new().add_kinds([CONTACT]).add_authors([*pubkey])),
    )?;

    Ok(cards
        .iter()
        .filter(|card| !card.content.trim().is_empty())
        .filter_map(|card| card.tags.iter().find(|tag| tag.name() == "d"))
        .filter_map(|tag| PublicKey::from_hex(tag.value()).ok())
        .collect())
}

/// The pubkeys the list `pubkey` published at `kind` names.
///
/// Both kinds are replaceable, so this is one indexed lookup and the current
/// list is whatever last superseded the address.
fn listed<const KIND: u16>(tx: &Tx<'_>, pubkey: &PublicKey) -> Result<BTreeSet<PublicKey>> {
    let address = Address::new(KIND, *pubkey, "");

    let Some(list) = event::by_address(tx, &address)? else {
        return Ok(BTreeSet::new());
    };

    // The event came back from the address, so its kind is KIND and cannot fail here.
    Ok(PeopleListReader::<_, KIND>::read(&list)
        .map(|list| list.pubkeys().iter().copied().collect())
        .unwrap_or_default())
}

/// The topics the list `pubkey` published at `kind` names, each without the
/// `#` a writer may have left on it.
fn topics<const KIND: u16>(tx: &Tx<'_>, pubkey: &PublicKey) -> Result<BTreeSet<String>> {
    let Some(list) = event::by_address(tx, &Address::new(KIND, *pubkey, ""))? else {
        return Ok(BTreeSet::new());
    };

    Ok(list
        .tags
        .iter()
        .filter(|tag| tag.name() == "t")
        .map(|tag| crate::model::topic(tag.value()).to_owned())
        .filter(|topic| !topic.is_empty())
        .collect())
}

fn to_pref(row: &Row<'_>) -> rusqlite::Result<Pref> {
    Ok(Pref {
        key: row.get("key")?,
        value: row.get("value")?,
        updated_at: row.get("updated_at")?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use coracle_lib::tags::Tags;

    use crate::db::Db;
    use crate::db::event::command as event_command;
    use crate::db::pref::command as pref_command;
    use crate::fixtures::{author, event, peer};
    use crate::model::{Scope, Sharing, Standing};

    #[test]
    fn an_unwritten_policy_is_the_documents_defaults() {
        let mut db = Db::open_in_memory().unwrap();
        let tx = db.begin_write().unwrap();

        // Nothing written, so nothing overridden: exactly what `Policy::new` says.
        assert_eq!(policy(&tx, &author(1)).unwrap(), Policy::new(author(1)));
    }

    #[test]
    fn a_written_preference_replaces_its_default() {
        let mut db = Db::open_in_memory().unwrap();
        let tx = db.begin_write().unwrap();

        pref_command::set_as(&tx, keys::SHARING, &Sharing::Network, 10).unwrap();
        pref_command::set_as(&tx, keys::DISCOVER_IN_BACKGROUND, &false, 10).unwrap();

        let policy = policy(&tx, &author(1)).unwrap();

        assert_eq!(policy.sharing, Sharing::Network);
        assert!(!policy.discover_in_background);
        assert_eq!(policy.accept, Scope::Lenient);
    }

    #[test]
    fn a_preference_this_build_cannot_read_is_an_error() {
        let mut db = Db::open_in_memory().unwrap();
        let tx = db.begin_write().unwrap();

        // Written by a build that knew a tier this one does not; defaulting would widen scope.
        pref_command::set(&tx, keys::ACCEPT, r#""neighbors""#, 10).unwrap();

        assert!(policy(&tx, &author(1)).is_err());
    }

    /// Publish `by`'s card calling `about` by `petname`.
    fn name(tx: &Tx<'_>, by: PublicKey, about: PublicKey, petname: &str, at: i64) {
        let card = event(
            by,
            CONTACT,
            at,
            petname,
            Tags::new().add("d", [about.to_hex()]),
        );

        event_command::save(tx, &card, &[by], at).unwrap();
    }

    /// Publish `pubkey`'s list of `kind` naming `names`.
    fn publish_list(tx: &Tx<'_>, pubkey: PublicKey, kind: u16, names: &[PublicKey], at: i64) {
        let tags = names
            .iter()
            .fold(Tags::new(), |tags, named| tags.add("p", [named.to_hex()]));

        event_command::save(tx, &event(pubkey, kind, at, "", tags), &[peer()], at).unwrap();
    }

    #[test]
    fn the_graph_reads_the_users_cards_and_block_list() {
        let mut db = Db::open_in_memory().unwrap();
        let tx = db.begin_write().unwrap();
        let us = author(1);

        name(&tx, us, author(2), "Ben", 100);
        name(&tx, us, author(3), "Ana", 100);
        publish_list(&tx, us, BLOCK, &[author(4)], 100);

        let graph = policy(&tx, &us).unwrap().graph;

        assert_eq!(graph.contacts, [author(2), author(3)].into());
        assert_eq!(graph.blocked, [author(4)].into());
        assert_eq!(graph.standing(&author(2)), Standing::Contact);
        assert_eq!(graph.standing(&author(4)), Standing::Blocked);
        assert_eq!(graph.standing(&author(9)), Standing::Stranger);
    }

    #[test]
    fn a_forgotten_person_is_no_contact() {
        let mut db = Db::open_in_memory().unwrap();
        let tx = db.begin_write().unwrap();
        let us = author(1);

        name(&tx, us, author(2), "Ben", 100);
        name(&tx, us, author(2), "", 200);

        assert!(policy(&tx, &us).unwrap().graph.contacts.is_empty());
    }

    #[test]
    fn a_blocked_person_is_in_no_tier_and_their_cards_add_nobody() {
        let mut db = Db::open_in_memory().unwrap();
        let tx = db.begin_write().unwrap();
        let us = author(1);

        name(&tx, us, author(2), "Ben", 100);
        name(&tx, us, author(3), "Ana", 100);
        publish_list(&tx, us, BLOCK, &[author(2), author(6)], 100);
        name(&tx, author(2), author(5), "Sam", 100);
        name(&tx, author(3), author(6), "Kit", 100);

        let graph = policy(&tx, &us).unwrap().graph;

        assert_eq!(graph.contacts, [author(3)].into());
        assert!(graph.network.is_empty());
        assert_eq!(graph.standing(&author(5)), Standing::Stranger);
    }

    #[test]
    fn network_is_everybody_the_contacts_named() {
        let mut db = Db::open_in_memory().unwrap();
        let tx = db.begin_write().unwrap();
        let us = author(1);

        name(&tx, us, author(2), "Ben", 100);
        name(&tx, us, author(3), "Ana", 100);
        name(&tx, author(2), author(5), "Sam", 100);
        name(&tx, author(3), author(6), "Kit", 100);
        // A stranger's cards reach nobody, however many people they name.
        name(&tx, author(9), author(7), "Lou", 100);

        let graph = policy(&tx, &us).unwrap().graph;

        assert_eq!(graph.standing(&author(5)), Standing::Network);
        assert_eq!(graph.standing(&author(6)), Standing::Network);
        assert_eq!(graph.standing(&author(7)), Standing::Stranger);
    }

    #[test]
    fn the_graph_stops_at_the_second_hop() {
        // Reading the cards of people in Network would grow the tier until it meant nothing.
        let mut db = Db::open_in_memory().unwrap();
        let tx = db.begin_write().unwrap();
        let us = author(1);

        name(&tx, us, author(2), "Ben", 100);
        name(&tx, author(2), author(3), "Ana", 100);
        name(&tx, author(3), author(4), "Sam", 100);

        let graph = policy(&tx, &us).unwrap().graph;

        assert_eq!(graph.standing(&author(3)), Standing::Network);
        assert_eq!(graph.standing(&author(4)), Standing::Stranger);
    }

    #[test]
    fn a_contact_whose_cards_have_not_arrived_contributes_nobody() {
        // The tier is derived from what this device holds, so a missing part shrinks it.
        let mut db = Db::open_in_memory().unwrap();
        let tx = db.begin_write().unwrap();
        let us = author(1);

        name(&tx, us, author(2), "Ben", 100);

        assert!(policy(&tx, &us).unwrap().graph.network.is_empty());

        name(&tx, author(2), author(5), "Sam", 200);

        assert_eq!(policy(&tx, &us).unwrap().graph.network, [author(5)].into());
    }

    #[test]
    fn someone_elses_cards_and_lists_are_not_the_users_own() {
        let mut db = Db::open_in_memory().unwrap();
        let tx = db.begin_write().unwrap();
        let us = author(1);

        name(&tx, peer(), author(8), "Ben", 100);
        publish_list(&tx, peer(), BLOCK, &[author(2)], 100);

        let graph = policy(&tx, &us).unwrap().graph;

        assert!(graph.contacts.is_empty());
        assert!(graph.blocked.is_empty());
    }

    #[test]
    fn the_graph_reads_the_lists_the_user_published() {
        let mut db = Db::open_in_memory().unwrap();
        let tx = db.begin_write().unwrap();

        let us = author(1);
        let mutes = event(
            us,
            MUTE,
            100,
            "",
            Tags::new()
                .add("p", [author(2).to_hex()])
                // Not a pubkey, and not a reason to lose the rest of the list.
                .add("p", ["nonsense"])
                .add("t", ["#politics"])
                .add("t", ["weather"]),
        );

        event_command::save(&tx, &mutes, &[us], 100).unwrap();

        let ours = policy(&tx, &us).unwrap();

        assert_eq!(ours.graph.muted, [author(2)].into());
        assert_eq!(
            ours.graph.muted_topics,
            ["politics".to_owned(), "weather".to_owned()].into()
        );
        assert!(ours.graph.muted.contains(&author(2)));

        // Muting is a display filter, so it moves nobody in the tiers.
        assert_eq!(ours.graph.standing(&author(2)), Standing::Stranger);

        // Someone else's mute list is not the user's.
        let theirs = event(
            peer(),
            MUTE,
            200,
            "",
            Tags::new().add("p", [author(3).to_hex()]),
        );

        event_command::save(&tx, &theirs, &[peer()], 200).unwrap();

        assert_eq!(policy(&tx, &us).unwrap().graph.muted, [author(2)].into());
    }
}
