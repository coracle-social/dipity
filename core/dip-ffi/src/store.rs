//! The store, opened once and read by both halves of the app.
//!
//! `docs/overview.md#architecture` has the view addressing the store directly
//! and gossip going through a [`Node`](crate::node::Node), and both are the same
//! SQLite file: one `Db`, two readers. So the shell opens this first and hands
//! it to the node, rather than the node owning a store the view cannot see.
//!
//! # Records in, JSON out
//!
//! A [`Query`] is a record, because that is where authorization rides: the
//! seen-time and peer criteria are their own fields and cannot be smuggled onto
//! the NIP-01 filter, which is the one part of a query that would be safe to
//! hand a peer. Building it in TypeScript and parsing it here would put that
//! boundary in a string.
//!
//! Answers go back as the JSON their serde form already defines. Nostr's types
//! are `coracle-lib`'s and are never redefined here — a uniffi record mirroring
//! `HashedEvent` is a second definition of an event, and the shell is a
//! pass-through that never reads one. Small vocabulary of the core's own, like
//! [`Pref`], crosses as itself.
//!
//! # Liveness
//!
//! [`Store::observe`] is a callback the shell registers and forwards to the
//! webview, and it is a coalescing thread over the store's own change channels —
//! not a re-query on a timer. A burst of fifty stored events is one refresh.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use coracle_lib::events::EventId;
use coracle_lib::filters::Filter;
use coracle_lib::keys::PublicKey;
use dip::clock::now;
use dip::db::{Db, command, query};
use dip::model::{BlobHash, Order as CoreOrder, ProvenanceFilter, Query as CoreQuery};
use tokio::sync::broadcast::error::TryRecvError;

/// How long a change waits for company before the view is told.
const COALESCE: Duration = Duration::from_millis(200);

/// Why the store could not answer.
#[derive(Debug, uniffi::Error)]
pub enum StoreError {
    /// SQLite refused, the migrations did not apply, or a read failed.
    Failed {
        /// What went wrong, for the log.
        reason: String,
    },
    /// The view handed over something that is not what it claims to be.
    Malformed {
        /// What was wrong with it.
        reason: String,
    },
}

impl std::fmt::Display for StoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Failed { reason } => write!(f, "the store failed: {reason}"),
            Self::Malformed { reason } => write!(f, "the query is malformed: {reason}"),
        }
    }
}

impl std::error::Error for StoreError {}

impl From<anyhow::Error> for StoreError {
    fn from(error: anyhow::Error) -> Self {
        Self::Failed {
            reason: format!("{error:#}"),
        }
    }
}

/// Something the view handed over that will not parse.
fn malformed(what: &str, error: &impl std::fmt::Display) -> StoreError {
    StoreError::Malformed {
        reason: format!("{what}: {error}"),
    }
}

/// What a result set is ordered by.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, uniffi::Enum)]
pub enum Order {
    /// The author's claimed time.
    #[default]
    CreatedAt,
    /// The local arrival time, which is what a feed of handed-over events
    /// wants: a note given to you today is new to you whatever its author
    /// stamped it.
    SeenAt,
}

/// One stored preference.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct Pref {
    /// The key, dotted by area.
    pub key: String,
    /// The value, as a JSON document.
    pub value: String,
    /// When it was last written.
    pub updated_at: i64,
}

/// Everything a read of stored events can be narrowed and ordered by.
///
/// The filter is NIP-01 and could be handed to a peer. Everything else is
/// provenance, which never leaves the device — `docs/privacy.md`.
#[derive(Debug, Clone, Default, PartialEq, Eq, uniffi::Record)]
pub struct Query {
    /// A NIP-01 filter, as JSON. Absent is no constraint.
    pub filter: Option<String>,
    /// Restrict to events first seen at or after this unix second.
    pub seen_since: Option<i64>,
    /// Restrict to events first seen at or before this unix second.
    pub seen_until: Option<i64>,
    /// Restrict to events seen from at least one of these pubkeys, hex.
    pub seen_from: Option<Vec<String>>,
    /// What the result is ordered by.
    pub order: Order,
}

/// Which group of tables changed.
///
/// The group, not the row: the view re-reads the query it is showing, so
/// carrying the change would be a second path to the same data that can
/// disagree with the first.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum Change {
    /// An event was stored, seen again, or removed.
    Events,
    /// A blob was recorded, made progress, completed or went.
    Blobs,
    /// A preference was written or removed.
    Preferences,
}

/// The name the view knows a group by.
///
/// Read rather than spelled in Swift and Kotlin, for the same reason
/// [`service_uuid`](crate::node::service_uuid) is: both shells put this string
/// on the same event and the view switches on it, so two copies is one place to
/// disagree.
#[uniffi::export]
#[must_use]
pub fn change_name(group: Change) -> String {
    match group {
        Change::Events => "events",
        Change::Blobs => "blobs",
        Change::Preferences => "preferences",
    }
    .to_owned()
}

/// Where the view is told the store moved.
#[uniffi::export(with_foreign)]
pub trait StoreObserver: Send + Sync {
    /// One or more writes landed in this group since the last call.
    fn changed(&self, group: Change);
}

/// A registration, live until it is dropped or [`stopped`](Self::stop).
#[derive(uniffi::Object)]
pub struct Subscription {
    /// Cleared to end the thread, which notices within [`COALESCE`].
    live: Arc<AtomicBool>,
}

#[uniffi::export]
impl Subscription {
    /// Stop hearing about changes.
    pub fn stop(&self) {
        self.live.store(false, Ordering::Relaxed);
    }
}

impl Drop for Subscription {
    fn drop(&mut self) {
        self.stop();
    }
}

/// One open store.
#[derive(uniffi::Object)]
pub struct Store {
    /// The connection, its migrations, and the channels its writes announce on.
    pub(crate) db: Arc<Db>,
    /// Every live registration, so dropping the store ends them.
    subscriptions: Mutex<Vec<Arc<AtomicBool>>>,
}

#[uniffi::export]
impl Store {
    /// Open the store in `directory`, migrating it to the current schema.
    ///
    /// The shell picks the directory; the core has no opinion about where an
    /// app's data lives on either platform. `docs/storage.md`.
    #[uniffi::constructor]
    pub fn open(directory: String) -> Result<Arc<Self>, StoreError> {
        Ok(Arc::new(Self {
            db: Arc::new(Db::open(directory)?),
            subscriptions: Mutex::new(Vec::new()),
        }))
    }

    // --------------------------------------------------------------- Reads

    /// Events matching every constraint on `query`, as NIP-01 JSON.
    pub fn list_events(&self, query: Query) -> Result<Vec<String>, StoreError> {
        let events = query::list_events(&self.db, &query.try_into()?)?;

        json_each(&events)
    }

    /// The same events, each with the media it references, the peers it arrived
    /// from and the peers it has been handed to.
    ///
    /// Three reads for the page rather than three per event, which is why a feed
    /// asks for this rather than looping over [`list_events`](Self::list_events).
    pub fn list_details(&self, query: Query) -> Result<Vec<String>, StoreError> {
        let events = query::list_events(&self.db, &query.try_into()?)?;
        let details = query::with_details(&self.db, events)?;

        json_each(&details)
    }

    /// One event by id, as NIP-01 JSON.
    pub fn get_event(&self, id: String) -> Result<Option<String>, StoreError> {
        let id = EventId::from_hex(&id).map_err(|error| malformed("event id", &error))?;

        query::get_event(&self.db, &id)?
            .as_ref()
            .map(json)
            .transpose()
    }

    /// Every stored preference.
    pub fn preferences(&self) -> Result<Vec<Pref>, StoreError> {
        Ok(query::preferences(&self.db)?
            .into_iter()
            .map(|pref| Pref {
                key: pref.key,
                value: pref.value,
                updated_at: pref.updated_at,
            })
            .collect())
    }

    /// One preference's value, as the JSON document it was written as.
    pub fn preference(&self, key: String) -> Result<Option<String>, StoreError> {
        Ok(query::preference(&self.db, &key)?)
    }

    /// Blobs a stored event references and this device does not hold, previews
    /// first.
    pub fn wanted_blobs(&self, limit: u32) -> Result<Vec<String>, StoreError> {
        json_each(&query::wanted_blobs(&self.db, limit as usize)?)
    }

    /// One blob's metadata and transfer progress.
    pub fn get_blob(&self, sha256: String) -> Result<Option<String>, StoreError> {
        let sha256 = blob_hash(&sha256)?;

        query::get_blob(&self.db, &sha256)?
            .as_ref()
            .map(json)
            .transpose()
    }

    /// Every stored event that references a hash, by id.
    pub fn events_referencing_blob(&self, sha256: String) -> Result<Vec<String>, StoreError> {
        let sha256 = blob_hash(&sha256)?;

        Ok(query::events_referencing_blob(&self.db, &sha256)?
            .iter()
            .map(EventId::to_hex)
            .collect())
    }

    // -------------------------------------------------------------- Writes

    /// Drop one event and everything hanging off it. Answers whether it was there.
    ///
    /// Local and silent: the sightings, who it was handed to, the author's
    /// signature and the media go with the row, and no peer is told. Asking the network to forget
    /// something is a kind 5 through [`Node::publish`](crate::node::Node::publish),
    /// which only its author can make. `docs/storage.md#dropping-one-thing`.
    pub fn forget_event(&self, id: String) -> Result<bool, StoreError> {
        let id = EventId::from_hex(&id).map_err(|error| malformed("event id", &error))?;

        Ok(command::forget_event(&self.db, &id)?)
    }

    /// Write a preference. `value` is a JSON document.
    ///
    /// Policy is stored as preferences and interpreted by the core, which is
    /// what leaves nothing to compute during a background wake.
    /// `docs/policy.md`.
    pub fn set_preference(&self, key: String, value: String) -> Result<(), StoreError> {
        serde_json::from_str::<serde_json::Value>(&value)
            .map_err(|error| malformed("preference value", &error))?;

        Ok(command::set_preference(&self.db, &key, &value, now())?)
    }

    /// Remove a preference, so its default applies again. Answers whether it
    /// was there.
    pub fn clear_preference(&self, key: String) -> Result<bool, StoreError> {
        Ok(command::clear_preference(&self.db, &key)?)
    }

    // ------------------------------------------------------------ Liveness

    /// Hear about writes as they commit.
    ///
    /// Changes are coalesced over [`COALESCE`] and reported as the group that
    /// moved, so the view re-reads whatever it is showing.
    pub fn observe(&self, observer: Arc<dyn StoreObserver>) -> Arc<Subscription> {
        let live = Arc::new(AtomicBool::new(true));

        self.subscriptions
            .lock()
            .expect("no panic holds the subscription list")
            .push(Arc::clone(&live));

        let mut events = dip::db::event::channel::subscribe(&self.db);
        let mut blobs = dip::db::blob::channel::subscribe(&self.db);
        let mut prefs = dip::db::pref::channel::subscribe(&self.db);
        let watching = Arc::clone(&live);

        std::thread::spawn(move || {
            while watching.load(Ordering::Relaxed) {
                let moved = [
                    (Change::Events, drain(&mut events)),
                    (Change::Blobs, drain(&mut blobs)),
                    (Change::Preferences, drain(&mut prefs)),
                ];

                for (group, changed) in moved {
                    if changed {
                        observer.changed(group);
                    }
                }

                std::thread::sleep(COALESCE);
            }
        });

        Arc::new(Subscription { live })
    }
}

impl Drop for Store {
    fn drop(&mut self) {
        for live in self
            .subscriptions
            .lock()
            .expect("no panic holds the subscription list")
            .iter()
        {
            live.store(false, Ordering::Relaxed);
        }
    }
}

/// Whether anything landed on this channel, taking everything that did.
///
/// A subscriber that fell behind is told the group moved, which is the same
/// answer a caught-up one gets — the view re-reads either way.
fn drain<T: Clone>(channel: &mut tokio::sync::broadcast::Receiver<T>) -> bool {
    let mut moved = false;

    loop {
        match channel.try_recv() {
            Ok(_) | Err(TryRecvError::Lagged(_)) => moved = true,
            Err(TryRecvError::Empty | TryRecvError::Closed) => return moved,
        }
    }
}

/// One answer, as the JSON its serde form defines.
fn json<T: serde::Serialize>(value: &T) -> Result<String, StoreError> {
    serde_json::to_string(value).map_err(|error| StoreError::Failed {
        reason: format!("{error}"),
    })
}

/// A page of them.
fn json_each<T: serde::Serialize>(values: &[T]) -> Result<Vec<String>, StoreError> {
    values.iter().map(json).collect()
}

/// A blob address the view named.
fn blob_hash(value: &str) -> Result<BlobHash, StoreError> {
    BlobHash::parse(value).map_err(|error| malformed("blob hash", &error))
}

impl TryFrom<Query> for CoreQuery {
    type Error = StoreError;

    fn try_from(query: Query) -> Result<Self, Self::Error> {
        let mut provenance = ProvenanceFilter::new();

        if let Some(since) = query.seen_since {
            provenance = provenance.add_since(since);
        }
        if let Some(until) = query.seen_until {
            provenance = provenance.add_until(until);
        }
        if let Some(pubkeys) = query.seen_from {
            let pubkeys: Result<Vec<PublicKey>, StoreError> = pubkeys
                .iter()
                .map(|pubkey| {
                    PublicKey::from_hex(pubkey).map_err(|error| malformed("seen_from", &error))
                })
                .collect();

            provenance = provenance.add_pubkeys(pubkeys?);
        }

        let filter = match query.filter {
            Some(filter) => {
                serde_json::from_str(&filter).map_err(|error| malformed("filter", &error))?
            }
            None => Filter::new(),
        };

        let core = Self::new()
            .with_filter(filter)
            .with_provenance(provenance)
            .with_order(match query.order {
                Order::CreatedAt => CoreOrder::CreatedAt,
                Order::SeenAt => CoreOrder::SeenAt,
            });

        Ok(core)
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;
    use std::sync::mpsc;

    use super::*;

    /// A store on disk that goes when the test does.
    struct Open {
        store: Arc<Store>,
        dir: std::path::PathBuf,
    }

    impl Open {
        fn new(name: &str) -> Self {
            let dir = std::env::temp_dir().join(format!("dip-store-{name}-{}", std::process::id()));

            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();

            let store = Store::open(dir.to_string_lossy().into_owned()).unwrap();

            Self { store, dir }
        }
    }

    impl Drop for Open {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    /// An observer that hands each group to a channel.
    struct Recorder(mpsc::Sender<Change>);

    impl StoreObserver for Recorder {
        fn changed(&self, group: Change) {
            let _ = self.0.send(group);
        }
    }

    #[test]
    fn a_preference_is_written_read_back_and_cleared() {
        let open = Open::new("prefs");

        open.store
            .set_preference("policy.retention_days".to_owned(), "30".to_owned())
            .unwrap();

        assert_eq!(
            open.store
                .preference("policy.retention_days".to_owned())
                .unwrap(),
            Some("30".to_owned())
        );

        let stored = open.store.preferences().unwrap();
        assert_eq!(stored.len(), 1);
        assert_eq!(stored[0].key, "policy.retention_days");

        assert!(
            open.store
                .clear_preference("policy.retention_days".to_owned())
                .unwrap()
        );
        assert!(open.store.preferences().unwrap().is_empty());
    }

    #[test]
    fn a_value_that_is_not_json_is_refused_before_it_is_stored() {
        let open = Open::new("bad-pref");

        assert!(matches!(
            open.store
                .set_preference("policy.gossip".to_owned(), "network".to_owned()),
            Err(StoreError::Malformed { .. })
        ));
        assert!(open.store.preferences().unwrap().is_empty());
    }

    #[test]
    fn a_filter_that_is_not_one_is_refused_rather_than_ignored() {
        let open = Open::new("bad-filter");
        let query = Query {
            filter: Some("kinds".to_owned()),
            ..Query::default()
        };

        assert!(matches!(
            open.store.list_events(query),
            Err(StoreError::Malformed { .. })
        ));
    }

    #[test]
    fn a_query_carries_its_provenance_criteria_separately_from_its_filter() {
        let query = Query {
            filter: Some(r#"{"kinds":[1]}"#.to_owned()),
            seen_since: Some(100),
            seen_until: Some(200),
            seen_from: Some(vec![
                "0000000000000000000000000000000000000000000000000000000000000002".to_owned(),
            ]),
            order: Order::SeenAt,
        };

        let core: CoreQuery = query.try_into().unwrap();

        assert_eq!(core.provenance.since, Some(100));
        assert_eq!(core.provenance.until, Some(200));
        assert_eq!(core.provenance.pubkeys.as_ref().map(BTreeSet::len), Some(1));
        assert_eq!(core.order, CoreOrder::SeenAt);

        // The peer's policy is the one half nothing the view can say reaches.
        assert!(core.registers.is_none());
        assert!(core.policy.is_none());
    }

    #[test]
    fn a_pubkey_that_is_not_one_is_refused() {
        let query = Query {
            seen_from: Some(vec!["nope".to_owned()]),
            ..Query::default()
        };

        assert!(CoreQuery::try_from(query).is_err());
    }

    #[test]
    fn a_write_tells_the_view_which_group_moved() {
        let open = Open::new("observe");
        let (sender, received) = mpsc::channel();
        let subscription = open.store.observe(Arc::new(Recorder(sender)));

        open.store
            .set_preference("policy.gossip".to_owned(), r#""network""#.to_owned())
            .unwrap();

        assert_eq!(
            received.recv_timeout(Duration::from_secs(5)).unwrap(),
            Change::Preferences
        );

        subscription.stop();
    }

    #[test]
    fn a_stopped_subscription_hears_nothing_more() {
        let open = Open::new("stop");
        let (sender, received) = mpsc::channel();
        let subscription = open.store.observe(Arc::new(Recorder(sender)));

        subscription.stop();
        std::thread::sleep(COALESCE * 3);

        open.store
            .set_preference("policy.gossip".to_owned(), r#""trusted""#.to_owned())
            .unwrap();

        assert!(received.recv_timeout(COALESCE * 3).is_err());
    }

    #[test]
    fn every_group_has_its_own_name() {
        let named = [Change::Events, Change::Blobs, Change::Preferences].map(change_name);

        assert_eq!(named, ["events", "blobs", "preferences"]);
        assert_eq!(named.iter().collect::<BTreeSet<_>>().len(), named.len());
    }
}
