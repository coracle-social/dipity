//! The connection, the migration runner, and [`Tx`].
//!
//! Private: what the rest of the core uses is re-exported by [`super`], which
//! is where the overview lives.

use std::cell::RefCell;
use std::fs;
use std::ops::Deref;
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex, MutexGuard, OnceLock, PoisonError};

use anyhow::{Context, Result, bail};
use rusqlite::{Connection, Transaction, TransactionBehavior};

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
const MIGRATIONS: &[Migration] = &[Migration {
    name: "0001_init",
    sql: include_str!("../../migrations/0001_init.sql"),
}];

/// The database file's name inside the directory the shell provides.
const FILENAME: &str = "dip.sqlite";

/// Where the database lives, set once by [`configure`] before first use.
static DIRECTORY: OnceLock<PathBuf> = OnceLock::new();

/// The connection. Opened and migrated on first access, and behind a mutex
/// because SQLite connections are not `Sync` and one file wants one writer.
static DATABASE: LazyLock<Mutex<Connection>> = LazyLock::new(|| {
    let directory = DIRECTORY
        .get()
        .expect("db::configure must be called before the database is used");
    let mut connection = open(&directory.join(FILENAME)).expect("failed to open the database");

    migrate(&mut connection).expect("failed to migrate the database");

    Mutex::new(connection)
});

/// Point the database at `directory`, which the shell owns and passes in at
/// startup. Must happen before the first query or command; the connection opens
/// lazily on first use and the path cannot change after that.
///
/// # Errors
///
/// If the directory cannot be created, or if a different one was already set.
pub fn configure(directory: impl AsRef<Path>) -> Result<()> {
    let directory = directory.as_ref();

    fs::create_dir_all(directory)
        .with_context(|| format!("creating database directory {}", directory.display()))?;

    match DIRECTORY.set(directory.to_path_buf()) {
        Ok(()) => Ok(()),
        Err(_) if DIRECTORY.get().is_some_and(|set| set == directory) => Ok(()),
        Err(rejected) => bail!(
            "database directory is already {}, refusing to move it to {}",
            DIRECTORY
                .get()
                .map(|set| set.display().to_string())
                .unwrap_or_default(),
            rejected.display()
        ),
    }
}

/// Run `f` inside a read transaction against the store.
///
/// The transaction rolls back when `f` returns, so nothing written inside one
/// survives; [`write`] is for that.
///
/// Crate-private, because a transaction is not something the store hands out:
/// its public surface is the named use cases in [`crate::db::query`] and
/// [`crate::db::command`], and those are the only callers here.
pub(crate) fn read<T>(f: impl FnOnce(&Tx<'_>) -> Result<T>) -> Result<T> {
    let mut connection = lock();
    let tx = Tx::begin_read(&mut connection)?;

    f(&tx)
}

/// Run `f` inside a write transaction against the store and commit it.
///
/// An error from `f` rolls the whole transaction back, queued notifications
/// included, so the tables and everything listening to them stay consistent.
pub(crate) fn write<T>(f: impl FnOnce(&Tx<'_>) -> Result<T>) -> Result<T> {
    let mut connection = lock();
    let tx = Tx::begin_write(&mut connection)?;
    let value = f(&tx)?;

    tx.commit()?;

    Ok(value)
}

/// Lock the connection, for the life of one transaction.
///
/// Private, and taken in exactly two places, because this mutex is not
/// reentrant: a second transaction opened while one is live deadlocks rather
/// than failing. Composing domain functions inside one [`read`] or [`write`] is
/// what them taking a [`Tx`] is for.
///
/// A panic inside a transaction poisons the mutex but leaves SQLite consistent —
/// the transaction rolls back on drop — so the poison is recovered from rather
/// than propagated, which would brick the store for the rest of the run.
fn lock() -> MutexGuard<'static, Connection> {
    DATABASE.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Open a migrated in-memory database. For tests and tooling: it shares nothing
/// with the singleton, so each one starts empty and they do not contend.
pub fn open_in_memory() -> Result<Connection> {
    let mut connection = Connection::open_in_memory().context("opening an in-memory database")?;

    configure_connection(&connection)?;
    migrate(&mut connection)?;

    Ok(connection)
}

/// Apply every migration the database has not seen yet.
///
/// # Errors
///
/// If a migration fails, in which case that migration's statements are rolled
/// back and `user_version` still names the last one that succeeded.
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

/// Open the file and put it in the state the rest of the core assumes.
fn open(path: &Path) -> Result<Connection> {
    let connection =
        Connection::open(path).with_context(|| format!("opening {}", path.display()))?;

    configure_connection(&connection)?;

    Ok(connection)
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

/// A transaction, plus the notifications its writes queued.
///
/// Commands announce their writes through their domain's `events` module, which
/// registers the send here rather than firing it. Draining happens in
/// [`commit`](Tx::commit), so a subscriber never hears about a row that a later
/// error rolled back, and a reactive read always finds what it was told about.
///
/// Derefs to [`rusqlite::Transaction`], so `tx.prepare(…)` and `tx.execute(…)`
/// work as they would on a bare connection.
pub struct Tx<'a> {
    inner: Transaction<'a>,
    on_commit: RefCell<Vec<Box<dyn FnOnce()>>>,
}

impl<'a> Tx<'a> {
    /// Begin a read transaction on a connection of the caller's own. Deferred,
    /// so it takes no write lock and a reader never blocks a writer under WAL.
    ///
    /// [`read`] is how the store is read; this is for a connection from
    /// [`open_in_memory`], which is what the domain tests use.
    pub fn begin_read(connection: &'a mut Connection) -> Result<Self> {
        Self::begin(connection, TransactionBehavior::Deferred)
    }

    /// Begin a write transaction on a connection of the caller's own.
    /// Immediate, so the write lock is taken up front and a busy database fails
    /// here rather than halfway through.
    pub fn begin_write(connection: &'a mut Connection) -> Result<Self> {
        Self::begin(connection, TransactionBehavior::Immediate)
    }

    fn begin(connection: &'a mut Connection, behavior: TransactionBehavior) -> Result<Self> {
        Ok(Self {
            inner: Transaction::new(connection, behavior)?,
            on_commit: RefCell::new(Vec::new()),
        })
    }

    /// Queue `f` to run once this transaction commits. Used by the domain
    /// `events` modules to defer a broadcast; nothing else should need it.
    pub fn after_commit(&self, f: impl FnOnce() + 'static) {
        self.on_commit.borrow_mut().push(Box::new(f));
    }

    /// Commit, then run whatever the writes queued, in the order they happened.
    ///
    /// # Errors
    ///
    /// If the commit fails, in which case the queue is dropped unrun.
    pub fn commit(self) -> Result<()> {
        let Self { inner, on_commit } = self;

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
        let connection = open_in_memory().unwrap();

        let mut names: Vec<String> = connection
            .prepare("SELECT name FROM sqlite_master WHERE type IN ('table', 'view')")
            .unwrap()
            .query_map([], |row| row.get(0))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();

        names.sort();
        names.dedup();

        for table in [
            "event",
            "event_tag",
            "event_fts",
            "event_seen",
            "proof",
            "pref",
            "blob",
        ] {
            assert!(names.contains(&table.to_string()), "missing table {table}");
        }
    }

    #[test]
    fn migrating_twice_is_a_no_op() {
        let mut connection = open_in_memory().unwrap();

        migrate(&mut connection).unwrap();

        let version: i64 = connection
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .unwrap();

        assert_eq!(version, MIGRATIONS.len() as i64);
    }

    #[test]
    fn foreign_keys_are_enforced() {
        let connection = open_in_memory().unwrap();

        let orphan = connection.execute(
            "INSERT INTO event_seen (event_id, peer_pubkey, seen_at) VALUES ('nope', 'peer', 1)",
            [],
        );

        assert!(orphan.is_err(), "a row hanging off no event was accepted");
    }

    #[test]
    fn commit_runs_queued_notifications() {
        use std::rc::Rc;

        let mut connection = open_in_memory().unwrap();
        let ran = Rc::new(RefCell::new(false));

        let tx = Tx::begin_write(&mut connection).unwrap();
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

        let mut connection = open_in_memory().unwrap();
        let ran = Rc::new(RefCell::new(false));

        {
            let tx = Tx::begin_write(&mut connection).unwrap();
            tx.after_commit({
                let ran = Rc::clone(&ran);
                move || *ran.borrow_mut() = true
            });
        }

        assert!(!*ran.borrow(), "a rolled back write still notified");
    }
}
