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
use crate::db::event::query as event;
use crate::model::{Graph, KIND_MUTE, Policy, Pref, keys};

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

    override_with(tx, keys::COOL_OFF_MINUTES, &mut policy.cool_off_minutes)?;
    override_with(tx, keys::DISCOVERABLE_TIMES, &mut policy.discoverable_times)?;
    override_with(tx, keys::DISCLOSURE_BUDGET, &mut policy.disclosure_budget)?;
    override_with(tx, keys::VISIBILITY, &mut policy.visibility)?;
    override_with(tx, keys::ACCEPT, &mut policy.accept)?;
    override_with(tx, keys::GOSSIP, &mut policy.gossip)?;
    override_with(tx, keys::FORWARD, &mut policy.forward)?;

    policy.graph = graph(tx, identity)?;

    Ok(policy)
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

    use crate::db::Db;
    use crate::db::event::command as event_command;
    use crate::db::pref::command as pref_command;
    use crate::fixtures::{author, event, peer};
    use crate::model::{Scope, Standing};

    #[test]
    fn an_unwritten_policy_is_the_documents_defaults() {
        let mut db = Db::open_in_memory().unwrap();
        let tx = db.begin_write().unwrap();

        // Nothing written, so nothing overridden: exactly what `Policy::new`
        // says, which is the only place a default is spelled out.
        assert_eq!(policy(&tx, &author(1)).unwrap(), Policy::new(author(1)));
    }

    #[test]
    fn a_written_preference_replaces_its_default() {
        let mut db = Db::open_in_memory().unwrap();
        let tx = db.begin_write().unwrap();

        pref_command::set_as(&tx, keys::GOSSIP, &Scope::Trusted, 10).unwrap();
        pref_command::set_as(&tx, keys::COOL_OFF_MINUTES, &0_i64, 10).unwrap();

        let policy = policy(&tx, &author(1)).unwrap();

        assert_eq!(policy.gossip, Scope::Trusted);
        assert_eq!(policy.cool_off_minutes, 0);
        assert_eq!(policy.accept, Scope::Lenient);
    }

    #[test]
    fn a_preference_this_build_cannot_read_is_an_error() {
        let mut db = Db::open_in_memory().unwrap();
        let tx = db.begin_write().unwrap();

        // Written by a build that knew a tier this one does not. Falling back
        // to the default here would quietly widen the scope the user chose.
        pref_command::set(&tx, keys::GOSSIP, r#""neighbors""#, 10).unwrap();

        assert!(policy(&tx, &author(1)).is_err());
    }

    #[test]
    fn the_graph_reads_the_lists_the_user_published() {
        let mut db = Db::open_in_memory().unwrap();
        let tx = db.begin_write().unwrap();

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

        event_command::save(&tx, &mutes, &[us], 100).unwrap();

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

        event_command::save(&tx, &theirs, &[peer()], 200).unwrap();

        assert_eq!(policy(&tx, &us).unwrap().graph.muted, [author(2)].into());
    }
}
