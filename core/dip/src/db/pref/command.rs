//! Writes over `pref`.

use anyhow::{Context, Result};
use rusqlite::params;
use serde::Serialize;

use crate::db::Tx;
use crate::model::Pref;

use super::channel::{self, PrefChange};

/// Write a preference, replacing whatever was there.
///
/// `value` is a JSON document. Prefer [`set_as`], which encodes for you; this
/// is for a value that arrived already encoded, such as one crossing the bridge
/// from the view.
pub fn set(tx: &Tx<'_>, key: &str, value: &str, updated_at: i64) -> Result<()> {
    serde_json::from_str::<serde_json::Value>(value)
        .with_context(|| format!("preference {key} was given a value that is not JSON"))?;

    tx.prepare_cached(
        "INSERT INTO pref (key, value, updated_at) VALUES (?1, ?2, ?3)
         ON CONFLICT (key) DO UPDATE SET value = excluded.value, updated_at = excluded.updated_at",
    )?
    .execute(params![key, value, updated_at])
    .with_context(|| format!("writing preference {key}"))?;

    channel::notify(
        tx,
        PrefChange::Set(Pref {
            key: key.to_string(),
            value: value.to_string(),
            updated_at,
        }),
    );

    Ok(())
}

/// Encode `value` and write it.
pub fn set_as<T: Serialize>(tx: &Tx<'_>, key: &str, value: &T, updated_at: i64) -> Result<()> {
    let encoded =
        serde_json::to_string(value).with_context(|| format!("encoding preference {key}"))?;

    set(tx, key, &encoded, updated_at)
}

/// Remove a preference, so its default applies again. Returns whether it was there.
pub fn remove(tx: &Tx<'_>, key: &str) -> Result<bool> {
    let removed = tx
        .prepare_cached("DELETE FROM pref WHERE key = ?1")?
        .execute(params![key])
        .with_context(|| format!("removing preference {key}"))?;

    if removed == 0 {
        return Ok(false);
    }

    channel::notify(tx, PrefChange::Removed(key.to_string()));

    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::Db;
    use crate::db::pref::query;
    use crate::model::keys;

    #[test]
    fn a_preference_round_trips() {
        let mut db = Db::open_in_memory().unwrap();
        let tx = db.begin_write().unwrap();

        set_as(&tx, keys::ACCEPT, &"lenient", 10).unwrap();

        assert_eq!(
            query::get(&tx, keys::ACCEPT).unwrap().as_deref(),
            Some(r#""lenient""#)
        );
        assert_eq!(
            query::get_as::<String>(&tx, keys::ACCEPT)
                .unwrap()
                .as_deref(),
            Some("lenient")
        );
    }

    #[test]
    fn an_unwritten_preference_is_its_default() {
        let mut db = Db::open_in_memory().unwrap();
        let tx = db.begin_write().unwrap();

        // Absent rather than defaulted: a default is `Policy::new`'s to say, not this layer's.
        assert_eq!(query::get(&tx, keys::GOSSIP).unwrap(), None);
        assert_eq!(
            query::get_as::<i64>(&tx, keys::COOL_OFF_MINUTES).unwrap(),
            None
        );
    }

    #[test]
    fn writing_again_replaces_the_value() {
        let mut db = Db::open_in_memory().unwrap();
        let tx = db.begin_write().unwrap();

        set_as(&tx, keys::DISCLOSURE_BUDGET, &3_i64, 10).unwrap();
        set_as(&tx, keys::DISCLOSURE_BUDGET, &5_i64, 20).unwrap();

        let stored = query::all(&tx).unwrap();

        assert_eq!(stored.len(), 1);
        assert_eq!(stored[0].value, "5");
        assert_eq!(stored[0].updated_at, 20);
    }

    #[test]
    fn a_value_that_is_not_json_is_refused() {
        let mut db = Db::open_in_memory().unwrap();
        let tx = db.begin_write().unwrap();

        // Bare text reads fine and decodes into nothing, so the policy silently defaults.
        assert!(set(&tx, keys::ACCEPT, "lenient", 10).is_err());
        assert!(set(&tx, keys::ACCEPT, r#""lenient""#, 10).is_ok());
    }

    #[test]
    fn removing_restores_the_default() {
        let mut db = Db::open_in_memory().unwrap();
        let tx = db.begin_write().unwrap();

        set_as(&tx, keys::ACCEPT, &"trusted", 10).unwrap();

        assert!(remove(&tx, keys::ACCEPT).unwrap());
        assert!(!remove(&tx, keys::ACCEPT).unwrap());
        assert_eq!(query::get(&tx, keys::ACCEPT).unwrap(), None);
    }
}
