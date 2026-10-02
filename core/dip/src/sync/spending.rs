//! How much has been written to this device, measured against the
//! [`Quota`](super::Quota) allowing it.
//!
//! Two meters over the same rolling 24 h window, because the threat has two
//! shapes:
//!
//! | Meter | Keyed on | Bounds |
//! | --- | --- | --- |
//! | Per peer | the peer's pubkey | one identity, however often it reconnects |
//! | The stranger pool | nothing | every untrusted peer together |
//!
//! The per-peer meter is the one that means something when identity does. A
//! peer the user trusts holds a key they chose, so metering it bounds that
//! person; every accept lands in the window, so dropping the link and
//! reconnecting refills nothing.
//!
//! Against a stranger it bounds very little, and the reason is structural:
//! content events carry no signature, so an identity costs an attacker one
//! keypair. Metering per pubkey assumes identity is expensive, and here it is
//! free. The pool is the answer — it is not keyed on identity, so there is
//! nothing for a burner to reset. `docs/sync.md#quotas`.
//!
//! Both are read from memory and written through to the store's `spending`
//! table, which a ledger opened over a store reads back at startup. A
//! background relaunch is routine on both platforms, and a window that died
//! with the process would refill the pool every time it happened. The window
//! prunes on every write, in memory and in the table, so neither grows without
//! bound.
//!
//! Blob bytes taken from a peer and served to one are two more meters over the
//! same window, per peer only: a stranger is allowed no blob bytes at all, so
//! there is nothing for a pool to bound.

use std::collections::{BTreeMap, VecDeque};
use std::sync::{Arc, Mutex};

use coracle_lib::events::HashedEvent;
use coracle_lib::keys::PublicKey;

use crate::db::{Db, command, query};
use crate::model::{Charge, Meter, Standing};
use crate::session::Peer;

/// How far back spending counts, in seconds.
pub const WINDOW_SECONDS: i64 = 24 * 60 * 60;

/// An event's count against the byte budget.
#[must_use]
pub fn event_size(event: &HashedEvent) -> usize {
    serde_json::to_vec(event).map_or(0, |encoded| encoded.len())
}

/// One accepted event's entry in the ledger.
type Entry = (i64, u64);

/// The ledger: one queue per peer, and one for every stranger together.
#[derive(Debug, Default)]
struct Ledger {
    /// Each peer's accepted events, `(seen_at, bytes)` in arrival order.
    by_peer: BTreeMap<PublicKey, VecDeque<Entry>>,
    /// What untrusted peers have written, pooled. No pubkey, which is the
    /// point: a fresh one buys nothing here.
    strangers: VecDeque<Entry>,
}

impl Ledger {
    /// Record one accepted event, pruning the peer's expired entries first so
    /// the deque stays bounded by one window of the peer's own approvals.
    fn record(&mut self, pubkey: &PublicKey, trusted: bool, at: i64, bytes: u64) {
        let cutoff = at - WINDOW_SECONDS;

        self.prune(pubkey, cutoff);
        self.by_peer
            .entry(*pubkey)
            .or_default()
            .push_back((at, bytes));

        if !trusted {
            prune_queue(&mut self.strangers, cutoff);
            self.strangers.push_back((at, bytes));
        }
    }

    /// What every untrusted peer together has written inside the window.
    fn strangers_since(&mut self, now: i64) -> (u32, u64) {
        let cutoff = now - WINDOW_SECONDS;
        prune_queue(&mut self.strangers, cutoff);

        total(&self.strangers, cutoff)
    }

    /// The accepted events and their bytes from `pubkey` inside the window.
    fn since(&mut self, pubkey: &PublicKey, now: i64) -> (u32, u64) {
        let cutoff = now - WINDOW_SECONDS;
        self.prune(pubkey, cutoff);

        self.by_peer
            .get(pubkey)
            .map_or((0, 0), |queue| total(queue, cutoff))
    }

    /// Drop a peer's entries older than `cutoff`, and the peer with them once
    /// it has nothing left inside the window.
    fn prune(&mut self, pubkey: &PublicKey, cutoff: i64) {
        let Some(queue) = self.by_peer.get_mut(pubkey) else {
            return;
        };

        prune_queue(queue, cutoff);

        if queue.is_empty() {
            self.by_peer.remove(pubkey);
        }
    }
}

/// Drop entries that fell out of the window. Arrival order, so the expired
/// ones are always at the front.
fn prune_queue(queue: &mut VecDeque<Entry>, cutoff: i64) {
    while queue.front().is_some_and(|(at, _)| *at < cutoff) {
        queue.pop_front();
    }
}

/// The events and bytes in `queue` at or after `cutoff`.
fn total(queue: &VecDeque<Entry>, cutoff: i64) -> (u32, u64) {
    queue
        .iter()
        .filter(|(at, _)| *at >= cutoff)
        .fold((0u32, 0u64), |(events, bytes), (_, size)| {
            (events.saturating_add(1), bytes.saturating_add(*size))
        })
}

/// The shared, thread-safe rolling ledger, metered across sessions.
///
/// One built with [`default`](Default::default) lives in memory alone, which
/// is what a test wants; one built with [`open`](Self::open) writes every
/// charge through to the store and starts from what the store remembers.
#[derive(Default)]
pub struct SpendingLedger {
    /// The event meter and the stranger pool.
    events: Mutex<Ledger>,
    /// Blob bytes taken from each peer.
    blobs: Mutex<Ledger>,
    /// Blob bytes served to each peer.
    served: Mutex<Ledger>,
    /// The store the window is written through to, if there is one.
    db: Option<Arc<Db>>,
}

impl SpendingLedger {
    /// A ledger over `db`, starting from the charges it holds inside the window.
    pub fn open(db: Arc<Db>) -> Self {
        let ledger = Self::default();
        let cutoff = crate::clock::now() - WINDOW_SECONDS;

        match query::charges_since(&db, cutoff) {
            Ok(charges) => {
                for charge in charges {
                    ledger.meter(charge.meter).record(
                        &charge.pubkey,
                        !charge.pooled,
                        charge.at,
                        charge.bytes,
                    );
                }
            }
            Err(error) => log::error!("reading the quota ledger failed: {error:#}"),
        }

        Self {
            db: Some(db),
            ..ledger
        }
    }

    /// Record one accepted event's bytes against the peer, and against the
    /// stranger pool when the peer is not one the user trusts.
    pub fn record(&self, pubkey: &PublicKey, trusted: bool, bytes: usize) {
        self.charge(Meter::Event, pubkey, !trusted, bytes as u64);
    }

    /// Record blob bytes taken from the peer.
    pub fn record_blob(&self, pubkey: &PublicKey, bytes: u64) {
        self.charge(Meter::Blob, pubkey, false, bytes);
    }

    /// Record blob bytes served to the peer.
    pub fn record_served(&self, pubkey: &PublicKey, bytes: u64) {
        self.charge(Meter::Served, pubkey, false, bytes);
    }

    /// The accepted events and their bytes from `pubkey` within the rolling
    /// window, for testing the peer's quota across sessions.
    pub fn since(&self, pubkey: &PublicKey) -> (u32, u64) {
        self.events
            .lock()
            .unwrap()
            .since(pubkey, crate::clock::now())
    }

    /// The blob bytes taken from `pubkey` within the rolling window.
    pub fn blob_since(&self, pubkey: &PublicKey) -> u64 {
        self.blobs
            .lock()
            .unwrap()
            .since(pubkey, crate::clock::now())
            .1
    }

    /// The blob bytes served to `pubkey` within the rolling window.
    pub fn served_since(&self, pubkey: &PublicKey) -> u64 {
        self.served
            .lock()
            .unwrap()
            .since(pubkey, crate::clock::now())
            .1
    }

    /// What every untrusted peer together has written within the window.
    pub fn strangers(&self) -> (u32, u64) {
        self.events
            .lock()
            .unwrap()
            .strangers_since(crate::clock::now())
    }

    /// Charge one meter in memory, and in the store when there is one.
    fn charge(&self, meter: Meter, pubkey: &PublicKey, pooled: bool, bytes: u64) {
        let at = crate::clock::now();

        self.meter(meter).record(pubkey, !pooled, at, bytes);

        if let Some(db) = &self.db {
            let charge = Charge {
                pubkey: *pubkey,
                meter,
                pooled,
                at,
                bytes,
            };

            if let Err(error) = command::record_charge(db, &charge, at - WINDOW_SECONDS) {
                log::error!("recording a charge to {pubkey} failed: {error:#}");
            }
        }
    }

    /// One meter's ledger, locked.
    fn meter(&self, meter: Meter) -> std::sync::MutexGuard<'_, Ledger> {
        match meter {
            Meter::Event => self.events.lock().unwrap(),
            Meter::Blob => self.blobs.lock().unwrap(),
            Meter::Served => self.served.lock().unwrap(),
        }
    }
}

/// What has been accepted, ready to test against a [`Quota`](crate::sync::Quota).
///
/// Carries both meters because a stranger is bounded by both: its own budget,
/// and what every stranger together has already taken.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Spent {
    /// Events accepted from this peer inside the window.
    pub events: u32,
    /// Bytes of them.
    pub bytes: u64,
    /// Events accepted from every untrusted peer together.
    pub pooled_events: u32,
    /// Bytes of those.
    pub pooled_bytes: u64,
}

/// One session's view onto the shared rolling window.
///
/// A device proving several identities is charged against every one of them
/// and read at the highest, so dropping a key from the set next time carries
/// the history forward rather than shedding it. A wholly fresh set still gets
/// a fresh meter — that is what the pool is for.
pub struct SessionSpending {
    /// The rolling window, shared with every other session.
    ledger: Arc<SpendingLedger>,
}

impl SessionSpending {
    /// A fresh session's metering over the shared ledger.
    #[must_use]
    pub fn new(ledger: Arc<SpendingLedger>) -> Self {
        Self { ledger }
    }

    /// The events and bytes accepted from the peer within the rolling window,
    /// this session's own accepts included, ready to test against its quota.
    ///
    /// A peer that proved no pubkey — which cannot happen once identified —
    /// has spent nothing.
    #[must_use]
    pub fn spent(&self, peer: &Peer) -> Spent {
        // The most-spent identity binds: no headroom bought with a quiet key beside a busy one.
        let (events, bytes) = peer
            .pubkeys
            .iter()
            .map(|pubkey| self.ledger.since(pubkey))
            .fold((0, 0), |(events, bytes), (their_events, their_bytes)| {
                (events.max(their_events), bytes.max(their_bytes))
            });

        // A trusted peer is not measured against the pool, so reading it would only cost a lock.
        let (pooled_events, pooled_bytes) = if peer.policy.standing == Standing::Trusted {
            (0, 0)
        } else {
            self.ledger.strangers()
        };

        Spent {
            events,
            bytes,
            pooled_events,
            pooled_bytes,
        }
    }

    /// The blob bytes taken from the peer within the rolling window, at the
    /// most-spent of the identities it proved.
    #[must_use]
    pub fn blob_spent(&self, peer: &Peer) -> u64 {
        peer.pubkeys
            .iter()
            .map(|pubkey| self.ledger.blob_since(pubkey))
            .max()
            .unwrap_or(0)
    }

    /// Count blob bytes taken from the peer against the rolling window,
    /// charged to every identity it proved.
    pub fn record_blob(&mut self, peer: &Peer, bytes: u64) {
        for pubkey in &peer.pubkeys {
            self.ledger.record_blob(pubkey, bytes);
        }
    }

    /// The blob bytes served to the peer within the rolling window, at the
    /// most-served of the identities it proved.
    #[must_use]
    pub fn served(&self, peer: &Peer) -> u64 {
        peer.pubkeys
            .iter()
            .map(|pubkey| self.ledger.served_since(pubkey))
            .max()
            .unwrap_or(0)
    }

    /// Count blob bytes served to the peer against the rolling window.
    pub fn record_served(&mut self, peer: &Peer, bytes: u64) {
        for pubkey in &peer.pubkeys {
            self.ledger.record_served(pubkey, bytes);
        }
    }

    /// Count one accepted event against the rolling window.
    ///
    /// Charged to every identity the peer proved, and once to the pool.
    pub fn record(&mut self, peer: &Peer, event: &HashedEvent) {
        let size = event_size(event);
        let trusted = peer.policy.standing == Standing::Trusted;

        for (index, pubkey) in peer.pubkeys.iter().enumerate() {
            // The pool counts the event, not the identities behind it.
            let pooled = index == 0;

            self.ledger.record(pubkey, trusted || !pooled, size);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::clock;
    use crate::fixtures::{author, note};
    use crate::link::LinkId;
    use crate::model::Policy;

    #[test]
    fn spending_sums_the_window_per_peer() {
        let ledger = SpendingLedger::default();

        clock::at(1_000, || {
            ledger.record(&author(1), false, 40);
            ledger.record(&author(1), false, 60);
            ledger.record(&author(2), false, 1);

            assert_eq!(ledger.since(&author(1)), (2, 100));
            assert_eq!(ledger.since(&author(2)), (1, 1));
        });
    }

    #[test]
    fn entries_older_than_the_window_are_pruned() {
        let ledger = SpendingLedger::default();

        // One entry well inside the window, one far inside it, one new.
        clock::at(1_000, || ledger.record(&author(1), false, 10));
        clock::at(2_000, || ledger.record(&author(1), false, 20));
        clock::at(1_000 + WINDOW_SECONDS + 1, || {
            ledger.record(&author(1), false, 30);

            // The first entry fell out of the window.
            assert_eq!(ledger.since(&author(1)), (2, 50));
        });
    }

    #[test]
    fn a_dormant_peer_leaves_no_rows_behind() {
        let ledger = SpendingLedger::default();

        clock::at(1_000, || {
            ledger.record(&author(1), false, 10);
        });
        clock::at(1_000 + WINDOW_SECONDS + 1, || {
            ledger.record(&author(2), false, 1);
            assert_eq!(ledger.since(&author(1)), (0, 0));
        });
    }

    #[test]
    fn a_fresh_pubkey_resets_its_own_meter_but_not_the_pool() {
        // The hole the pool closes: unsigned events make a fresh keypair, and a fresh meter, free.
        let ledger = Arc::new(SpendingLedger::default());

        clock::at(1_000, || {
            for seed in 10..20u8 {
                ledger.record(&author(seed), false, 1_000);
            }

            // Each burner looks untouched to the per-peer meter...
            assert_eq!(ledger.since(&author(19)), (1, 1_000));

            // ...while the pool has seen all of it.
            assert_eq!(ledger.strangers(), (10, 10_000));
        });
    }

    #[test]
    fn a_trusted_peer_is_not_charged_to_the_stranger_pool() {
        // The pool is the untrusted ceiling; charging trusted traffic to it crowds out the chosen.
        let ledger = SpendingLedger::default();

        clock::at(1_000, || {
            ledger.record(&author(1), true, 500);

            assert_eq!(ledger.since(&author(1)), (1, 500));
            assert_eq!(ledger.strangers(), (0, 0));
        });
    }

    #[test]
    fn a_ledger_over_a_store_remembers_the_window_across_a_relaunch() {
        let db = Arc::new(crate::db::Db::open_in_memory().unwrap());

        clock::at(1_000, || {
            let ledger = SpendingLedger::open(Arc::clone(&db));
            ledger.record(&author(1), false, 40);
            ledger.record_blob(&author(2), 4_096);
            ledger.record_served(&author(2), 1_024);
        });

        clock::at(2_000, || {
            let relaunched = SpendingLedger::open(Arc::clone(&db));

            assert_eq!(relaunched.since(&author(1)), (1, 40));
            assert_eq!(relaunched.strangers(), (1, 40));
            assert_eq!(relaunched.blob_since(&author(2)), 4_096);
            assert_eq!(relaunched.served_since(&author(2)), 1_024);
        });

        clock::at(1_000 + WINDOW_SECONDS + 1, || {
            let later = SpendingLedger::open(Arc::clone(&db));

            assert_eq!(later.since(&author(1)), (0, 0));
        });
    }

    #[test]
    fn an_accept_counts_once_against_the_shared_window() {
        let ledger = Arc::new(SpendingLedger::default());
        let peer = Peer::bind(LinkId(1), [author(2)], &Policy::new(author(1)));

        clock::at(1_000, || {
            // A past session already put one event in the window.
            ledger.record(&author(2), false, 10);

            // This session accepts one more, in the same window and not double-counted.
            let mut spending = SessionSpending::new(Arc::clone(&ledger));
            let event = note(author(2), 100, "counted", coracle_lib::tags::Tags::new());
            spending.record(&peer, &event);

            let spent = spending.spent(&peer);
            assert_eq!(spent.events, 2);
            assert!(spent.bytes > 10);
        });
    }
}
