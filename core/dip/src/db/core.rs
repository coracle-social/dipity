//! The store instance, the migration runner, and [`Tx`].
//!
//! Private: what the rest of the core uses is re-exported by [`super`], which
//! is where the overview lives.

use std::cell::RefCell;
use std::fs;
use std::ops::Deref;
use std::path::Path;
use std::sync::{Mutex, MutexGuard, PoisonError};

use anyhow::{Context, Result, bail};
use rusqlite::{Connection, Transaction, TransactionBehavior};

use crate::db::channels::Channels;

/// A schema step, applied in order and recorded in SQLite's `user_version`.
struct Migration {
    /// File name without extension, for error messages.
    name: &'static str,
    /// The statements, run as one batch inside a transaction.
    sql: &'static str,
}

/// Every migration, oldest first. Append only — the index in this slice is the
/// `user_version` a database is left at, so reordering or removing an entry
/// re-runs the wrong statements against an existing store.
const MIGRATIONS: &[Migration] = &[
    Migration {
        name: "0001_init",
        sql: include_str!("../../migrations/0001_init.sql"),
    },
    Migration {
        name: "0002_blob_blake3_tree",
        sql: include_str!("../../migrations/0002_blob_blake3_tree.sql"),
    },
    Migration {
        name: "0003_drop_blob_blake3_tree",
        sql: include_str!("../../migrations/0003_drop_blob_blake3_tree.sql"),
    },
];

/// The database file's name inside the directory the shell provides.
const FILENAME: &str = "dip.sqlite";

/// One open store: its connection, and the channels its writes announce on.
///
/// Instanced rather than global so we can use multiple in tests. `Send + Sync`,
/// because the shell holds one across the FFI while the session layer reads it
/// on whatever thread the radio called in on. The connection is behind a mutex:
/// SQLite connections are not `Sync`, and one file wants one writer.
pub struct Db {
    /// The connection, locked for the life of one transaction.
    connection: Mutex<Connection>,
    /// What this store's writes announce on. Used by the domain `channel`
    /// modules to subscribe; nothing else should need it.
    pub(crate) channels: Channels,
}

impl Db {
    /// Open the store in `directory`, which the shell owns and passes in at
    /// startup, creating and migrating it if it is not already there.
    ///
    /// Idempotent: the shell may open the same directory on a background
    /// relaunch as well as at startup.
    pub fn open(directory: impl AsRef<Path>) -> Result<Self> {
        let directory = directory.as_ref();

        fs::create_dir_all(directory)
            .with_context(|| format!("creating database directory {}", directory.display()))?;

        let path = directory.join(FILENAME);
        let mut connection =
            Connection::open(&path).with_context(|| format!("opening {}", path.display()))?;

        configure_connection(&connection)?;
        migrate(&mut connection)?;

        Ok(Self::new(connection))
    }

    /// A migrated in-memory store, sharing neither tables nor channels with any
    /// other. For tests and tooling.
    pub fn open_in_memory() -> Result<Self> {
        Ok(Self::new(in_memory()?))
    }

    /// Wrap a connection that is already open, configured and migrated.
    fn new(connection: Connection) -> Self {
        Self {
            connection: Mutex::new(connection),
            channels: Channels::default(),
        }
    }

    /// Run `f` inside a read transaction against this store.
    pub(crate) fn read<T>(&self, f: impl FnOnce(&Tx<'_>) -> Result<T>) -> Result<T> {
        let mut connection = self.lock();
        let tx = Tx::begin(
            &mut connection,
            &self.channels,
            TransactionBehavior::Deferred,
        )?;

        f(&tx)
    }

    /// Run `f` inside a write transaction against this store and commit it.
    pub(crate) fn write<T>(&self, f: impl FnOnce(&Tx<'_>) -> Result<T>) -> Result<T> {
        let mut connection = self.lock();
        let tx = Tx::begin(
            &mut connection,
            &self.channels,
            TransactionBehavior::Immediate,
        )?;
        let value = f(&tx)?;

        tx.commit()?;

        Ok(value)
    }

    /// Lock the connection, for the life of one transaction.
    ///
    /// Private, and taken in exactly two places, because this mutex is not
    /// reentrant: a second transaction opened while one is live deadlocks
    /// rather than failing. Composing domain functions inside one
    /// [`read`](Self::read) or [`write`](Self::write) is what them taking a
    /// [`Tx`] is for.
    ///
    /// A panic inside a transaction poisons the mutex but leaves SQLite
    /// consistent — the transaction rolls back on drop — so the poison is
    /// recovered from rather than propagated, which would brick the store for
    /// the rest of the run.
    fn lock(&self) -> MutexGuard<'_, Connection> {
        self.connection
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }

    /// Begin a write transaction directly, for the tests down the tree that
    /// exercise one group's queries and commands against a store of their own.
    ///
    /// Takes `&mut self` and skips the lock, so it cannot be reached through
    /// the shared handle the rest of the core holds. [`read`](Self::read) and
    /// [`write`](Self::write) are the way in.
    #[cfg(test)]
    pub(crate) fn begin_write(&mut self) -> Result<Tx<'_>> {
        let Self {
            connection,
            channels,
        } = self;
        let connection = connection.get_mut().unwrap_or_else(PoisonError::into_inner);

        Tx::begin(connection, channels, TransactionBehavior::Immediate)
    }
}

/// Open a migrated in-memory database.
fn in_memory() -> Result<Connection> {
    let mut connection = Connection::open_in_memory().context("opening an in-memory database")?;

    configure_connection(&connection)?;
    migrate(&mut connection)?;

    Ok(connection)
}

/// Apply every migration the database has not seen yet.
pub fn migrate(connection: &mut Connection) -> Result<()> {
    let applied: usize = connection
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .map(|version: i64| usize::try_from(version).unwrap_or(0))
        .context("reading user_version")?;

    if applied > MIGRATIONS.len() {
        bail!(
            "database is at migration {applied}, but this build only knows {}",
            MIGRATIONS.len()
        );
    }

    for (index, migration) in MIGRATIONS.iter().enumerate().skip(applied) {
        let tx = connection.transaction()?;

        tx.execute_batch(migration.sql)
            .with_context(|| format!("applying migration {}", migration.name))?;

        // Bound by MIGRATIONS.len(), so the cast cannot wrap in any real build.
        tx.pragma_update(None, "user_version", index as i64 + 1)?;
        tx.commit()?;
    }

    Ok(())
}

/// The pragmas, in one place so an in-memory test database behaves like the
/// real one. Foreign keys are the load-bearing one: the event tables hang off
/// `event` by cascade, and SQLite leaves enforcement off by default.
fn configure_connection(connection: &Connection) -> Result<()> {
    connection.pragma_update(None, "journal_mode", "WAL")?;
    connection.pragma_update(None, "synchronous", "NORMAL")?;
    connection.pragma_update(None, "foreign_keys", true)?;
    connection.busy_timeout(std::time::Duration::from_secs(5))?;

    Ok(())
}

/// A transaction, plus the notifications its writes queued and the channels
/// they go out on.
///
/// Commands announce their writes through their domain's `channel` module, which
/// registers the send here rather than firing it. Draining happens in
/// [`commit`](Tx::commit), so a subscriber never hears about a row that a later
/// error rolled back, and a reactive read always finds what it was told about.
///
/// The channels come from the [`Db`] the transaction was opened on, so a write
/// is announced to that store's subscribers and to no other's.
///
/// Derefs to [`rusqlite::Transaction`], so `tx.prepare(…)` and `tx.execute(…)`
/// work as they would on a bare connection.
pub struct Tx<'a> {
    inner: Transaction<'a>,
    /// The channels this transaction's writes announce on. Used by the
    /// domain `channel` modules; nothing else should need it.
    pub(crate) channels: &'a Channels,
    on_commit: RefCell<Vec<Box<dyn FnOnce()>>>,
}

impl<'a> Tx<'a> {
    /// Begin a transaction on `connection`, announcing on `channels`.
    ///
    /// Deferred takes no write lock, so a reader never blocks a writer under
    /// WAL; Immediate takes it up front, so a busy database fails here rather
    /// than halfway through.
    fn begin(
        connection: &'a mut Connection,
        channels: &'a Channels,
        behavior: TransactionBehavior,
    ) -> Result<Self> {
        Ok(Self {
            inner: Transaction::new(connection, behavior)?,
            channels,
            on_commit: RefCell::new(Vec::new()),
        })
    }

    /// Queue `f` to run once this transaction commits. Used by the domain
    /// `channel` modules to defer a broadcast; nothing else should need it.
    pub(crate) fn after_commit(&self, f: impl FnOnce() + 'static) {
        self.on_commit.borrow_mut().push(Box::new(f));
    }

    /// Commit, then run whatever the writes queued, in the order they happened.
    pub fn commit(self) -> Result<()> {
        let Self {
            inner, on_commit, ..
        } = self;

        inner.commit()?;

        for f in on_commit.into_inner() {
            f();
        }

        Ok(())
    }
}

impl<'a> Deref for Tx<'a> {
    type Target = Transaction<'a>;

    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn migrations_create_every_table() {
        let connection = in_memory().unwrap();

        let objects: Vec<(String, String)> = connection
            .prepare("SELECT name, sql FROM sqlite_master WHERE type IN ('table', 'view')")
            .unwrap()
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();

        let shadowed: Vec<&str> = objects
            .iter()
            .filter(|(_, sql)| sql.starts_with("CREATE VIRTUAL TABLE"))
            .map(|(name, _)| name.as_str())
            .collect();

        let mut names: Vec<&str> = objects
            .iter()
            .map(|(name, _)| name.as_str())
            .filter(|name| {
                !shadowed
                    .iter()
                    .any(|owner| name.starts_with(&format!("{owner}_")))
            })
            .collect();

        names.sort();

        assert_eq!(
            names,
            [
                "blob",
                "disclosure",
                "event",
                "event_fts",
                "event_seen",
                "event_tag",
                "pair_secret",
                "pref",
                "recipient_signature",
            ]
        );
    }

    #[test]
    fn migrating_twice_is_a_no_op() {
        let mut connection = in_memory().unwrap();

        migrate(&mut connection).unwrap();

        let version: i64 = connection
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .unwrap();

        assert_eq!(version, MIGRATIONS.len() as i64);
    }

    #[test]
    fn foreign_keys_are_enforced() {
        let connection = in_memory().unwrap();

        let orphan = connection.execute(
            "INSERT INTO event_seen (event_id, pubkey, seen_at) VALUES ('nope', 'peer', 1)",
            [],
        );

        assert!(orphan.is_err(), "a row hanging off no event was accepted");
    }

    #[test]
    fn commit_runs_queued_notifications() {
        use std::rc::Rc;

        let mut db = Db::open_in_memory().unwrap();
        let ran = Rc::new(RefCell::new(false));

        let tx = db.begin_write().unwrap();
        tx.after_commit({
            let ran = Rc::clone(&ran);
            move || *ran.borrow_mut() = true
        });

        assert!(!*ran.borrow(), "notification fired before the commit");

        tx.commit().unwrap();

        assert!(*ran.borrow(), "notification did not fire after the commit");
    }

    #[test]
    fn rollback_drops_queued_notifications() {
        use std::rc::Rc;

        let mut db = Db::open_in_memory().unwrap();
        let ran = Rc::new(RefCell::new(false));

        {
            let tx = db.begin_write().unwrap();
            tx.after_commit({
                let ran = Rc::clone(&ran);
                move || *ran.borrow_mut() = true
            });
        }

        assert!(!*ran.borrow(), "a rolled back write still notified");
    }

    #[test]
    fn a_store_is_shareable_across_threads() {
        const fn assert_send_sync<T: Send + Sync>() {}

        assert_send_sync::<Db>();
    }

    #[test]
    fn two_stores_do_not_hear_each_other() {
        use crate::db::pref::channel::{self, PrefChange};

        let one = Db::open_in_memory().unwrap();
        let two = Db::open_in_memory().unwrap();

        let mut listening = channel::subscribe(&one);

        two.write(|tx| {
            crate::db::pref::command::set(tx, "some.key", r#""value""#, 10)?;

            Ok(())
        })
        .unwrap();

        assert!(
            listening.try_recv().is_err(),
            "a write to one store was announced on the other's channel"
        );

        one.write(|tx| {
            crate::db::pref::command::set(tx, "some.key", r#""value""#, 10)?;

            Ok(())
        })
        .unwrap();

        assert!(
            matches!(listening.try_recv(), Ok(PrefChange::Set(_))),
            "a write to the subscribed store was not announced"
        );
    }
}
