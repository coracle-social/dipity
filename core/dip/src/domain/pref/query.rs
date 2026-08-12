//! Reads over `pref`, and the [`Policy`] assembled from them.
//!
//! [`policy`] is the one the rest of the core calls. It reads every key
//! `docs/policy.md` defines, substitutes the document's defaults for the ones
//! never written, and resolves the trust graph the tiers are measured against —
//! which lives in events rather than in this table, since trust, block and mute
//! are lists the user publishes.

use std::collections::BTreeSet;

use anyhow::{Context, Result};
use coracle_lib::addresses::Address;
use coracle_lib::keys::PublicKey;
use rusqlite::{Row, params};
use serde::de::DeserializeOwned;

use crate::db::Tx;
use crate::domain::event::model::KIND_MUTE;
use crate::domain::event::query as event;

use super::model::{
    DEFAULT_COOL_OFF_MINUTES, DEFAULT_DISCLOSURE_BUDGET, Graph, Policy, Pref, Scope, keys,
};

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
///
/// # Errors
///
/// If the query fails or the stored value does not decode into `T`.
pub fn get_as<T: DeserializeOwned>(tx: &Tx<'_>, key: &str) -> Result<Option<T>> {
    let Some(value) = get(tx, key)? else {
        return Ok(None);
    };

    let decoded =
        serde_json::from_str(&value).with_context(|| format!("decoding preference {key}"))?;

    Ok(Some(decoded))
}

/// One preference, decoded, falling back to `default` when it has never been
/// written.
///
/// # Errors
///
/// If the query fails or the stored value does not decode into `T`.
pub fn get_or<T: DeserializeOwned>(tx: &Tx<'_>, key: &str, default: T) -> Result<T> {
    Ok(get_as(tx, key)?.unwrap_or(default))
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
/// Read once per session and bound to a peer with
/// [`Policy::for_peer`](super::model::Policy::for_peer), rather than re-read
/// per event: a session asks the same questions of the same peer many times,
/// and the answers cannot change under it while it runs.
///
/// # Errors
///
/// If a query fails, or a stored preference does not decode into the shape this
/// build expects — which means an older build wrote it, and substituting a
/// default would quietly loosen a policy the user set.
pub fn policy(tx: &Tx<'_>, identity: &PublicKey) -> Result<Policy> {
    Ok(Policy {
        identity: *identity,
        cool_off_minutes: get_or(tx, keys::COOL_OFF_MINUTES, DEFAULT_COOL_OFF_MINUTES)?,
        discoverable_times: get_or(tx, keys::DISCOVERABLE_TIMES, Vec::new())?,
        disclosure_budget: get_or(tx, keys::DISCLOSURE_BUDGET, DEFAULT_DISCLOSURE_BUDGET)?,
        profile_visibility: get_or(tx, keys::PROFILE_VISIBILITY, Scope::Public)?,
        content_visibility: get_or(tx, keys::CONTENT_VISIBILITY, Scope::Public)?,
        metadata_visibility: get_or(tx, keys::METADATA_VISIBILITY, Scope::Trusted)?,
        accept: get_or(tx, keys::ACCEPT, Scope::Lenient)?,
        gossip: get_or(tx, keys::GOSSIP, Scope::Network)?,
        graph: graph(tx, identity)?,
    })
}

/// The user's trust graph, from the lists they have published.
///
/// Only the mute list is readable today: `docs/policy.md` has yet to assign
/// kinds to trust and block, so those tiers stay empty and every scope narrower
/// than `lenient` admits nobody. That is the conservative direction to be wrong
/// in — a device that trusts no one relays nothing but its own — and it is
/// where the trust and block lists get read once the kinds exist.
///
/// A NIP-51 list carries private entries encrypted to the author in `content`.
/// Decrypting is the signing key's job and not this layer's, so what is read
/// here is the public half.
fn graph(tx: &Tx<'_>, identity: &PublicKey) -> Result<Graph> {
    Ok(Graph {
        trusted: BTreeSet::new(),
        network: BTreeSet::new(),
        blocked: BTreeSet::new(),
        muted: listed(tx, identity, KIND_MUTE)?,
    })
}

/// The pubkeys a list the user published names in its public `p` tags.
///
/// An entry that is not a pubkey is skipped rather than failing the read: the
/// list may have been written by another client, and one bad tag should not
/// cost the user the rest of it.
fn listed(tx: &Tx<'_>, identity: &PublicKey, kind: u16) -> Result<BTreeSet<PublicKey>> {
    let address = Address::new(kind, *identity, "");

    let Some(list) = event::by_address(tx, &address)? else {
        return Ok(BTreeSet::new());
    };

    Ok(list
        .tags
        .values("p")
        .filter_map(|value| PublicKey::from_hex(value).ok())
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

    use crate::db::open_in_memory;
    use crate::domain::event::command as event_command;
    use crate::domain::event::fixtures::{author, event, peer};
    use crate::domain::pref::command as pref_command;
    use crate::domain::pref::model::Standing;

    #[test]
    fn an_unwritten_policy_is_the_documents_defaults() {
        let mut connection = open_in_memory().unwrap();
        let tx = Tx::begin_write(&mut connection).unwrap();

        let policy = policy(&tx, &author(1)).unwrap();

        assert_eq!(policy.identity, author(1));
        assert_eq!(policy.cool_off_minutes, DEFAULT_COOL_OFF_MINUTES);
        assert_eq!(policy.disclosure_budget, DEFAULT_DISCLOSURE_BUDGET);
        assert!(policy.discoverable_times.is_empty());
        assert_eq!(policy.profile_visibility, Scope::Public);
        assert_eq!(policy.content_visibility, Scope::Public);
        assert_eq!(policy.metadata_visibility, Scope::Trusted);
        assert_eq!(policy.accept, Scope::Lenient);
        assert_eq!(policy.gossip, Scope::Network);
        assert_eq!(policy.graph, Graph::default());
    }

    #[test]
    fn a_written_preference_replaces_its_default() {
        let mut connection = open_in_memory().unwrap();
        let tx = Tx::begin_write(&mut connection).unwrap();

        pref_command::set_as(&tx, keys::GOSSIP, &Scope::Trusted, 10).unwrap();
        pref_command::set_as(&tx, keys::COOL_OFF_MINUTES, &0_i64, 10).unwrap();

        let policy = policy(&tx, &author(1)).unwrap();

        assert_eq!(policy.gossip, Scope::Trusted);
        assert_eq!(policy.cool_off_minutes, 0);
        assert_eq!(policy.accept, Scope::Lenient);
    }

    #[test]
    fn a_preference_this_build_cannot_read_is_an_error() {
        let mut connection = open_in_memory().unwrap();
        let tx = Tx::begin_write(&mut connection).unwrap();

        // Written by a build that knew a tier this one does not. Falling back
        // to the default here would quietly widen the scope the user chose.
        pref_command::set(&tx, keys::GOSSIP, r#""neighbors""#, 10).unwrap();

        assert!(policy(&tx, &author(1)).is_err());
    }

    #[test]
    fn the_graph_reads_the_lists_the_user_published() {
        let mut connection = open_in_memory().unwrap();
        let tx = Tx::begin_write(&mut connection).unwrap();

        let us = author(1);
        let mutes = event(
            us,
            KIND_MUTE,
            100,
            "",
            Tags::new()
                .add("p", [author(2).to_hex()])
                // Not a pubkey, and not a reason to lose the rest of the list.
                .add("p", ["nonsense"]),
        );

        event_command::save(&tx, &mutes, &us, 100).unwrap();

        let ours = policy(&tx, &us).unwrap();

        assert_eq!(ours.graph.muted, [author(2)].into());
        assert!(ours.is_muted(&author(2)));

        // Muting is a display filter, so it moves nobody in the tiers.
        assert_eq!(ours.graph.standing(&author(2)), Standing::Stranger);

        // Someone else's mute list is not the user's.
        let theirs = event(
            peer(),
            KIND_MUTE,
            200,
            "",
            Tags::new().add("p", [author(3).to_hex()]),
        );

        event_command::save(&tx, &theirs, &peer(), 200).unwrap();

        assert_eq!(policy(&tx, &us).unwrap().graph.muted, [author(2)].into());
    }
}
