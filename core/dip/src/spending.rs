//! How much a peer has written to this device, measured against its quota.
//!
//! One meter, the rolling 24 h window: [`SpendingLedger`], shared across
//! sessions, with [`SessionSpending`] as one session's view onto it. Every
//! accept lands in the window, so the rolling ceiling bounds the session that
//! did the accepting as much as the reconnect that follows it — a drive-by
//! cannot refill its budget by dropping the link, and no separate per-session
//! counter is needed (one would double-count what the window already holds).
//!
//! The ledger lives in memory and dies with the process: durable billing rows
//! of "who handed me how much, when" are exactly the provenance the app
//! refuses to keep (`docs/privacy.md`), and a rolling meter that resets on
//! restart costs nothing, since a peer cannot kill the process to refill it.
//! The window is bounded — each peer's entries older than it are pruned on
//! every write, so the ledger cannot grow without bound either.

use std::collections::{BTreeMap, VecDeque};
use std::sync::{Arc, Mutex};

use coracle_lib::events::HashedEvent;
use coracle_lib::keys::PublicKey;

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

/// The ledger, one queue per peer, most recent at the back.
#[derive(Debug, Default)]
struct Ledger {
    /// Each peer's accepted events, `(seen_at, bytes)` in arrival order.
    by_peer: BTreeMap<PublicKey, VecDeque<Entry>>,
}

impl Ledger {
    /// Record one accepted event, pruning the peer's expired entries first so
    /// the deque stays bounded by one window of the peer's own approvals.
    fn record(&mut self, pubkey: &PublicKey, at: i64, bytes: u64) {
        self.prune(pubkey, at - WINDOW_SECONDS);

        let queue = self.by_peer.entry(*pubkey).or_default();
        queue.push_back((at, bytes));
    }

    /// The accepted events and their bytes from `pubkey` inside the window.
    fn since(&mut self, pubkey: &PublicKey, now: i64) -> (u32, u64) {
        let cutoff = now - WINDOW_SECONDS;
        self.prune(pubkey, cutoff);

        let mut events = 0u32;
        let mut bytes = 0u64;

        if let Some(queue) = self.by_peer.get(pubkey) {
            for (_, size) in queue.iter().filter(|(at, _)| *at >= cutoff) {
                events = events.saturating_add(1);
                bytes = bytes.saturating_add(*size);
            }
        }

        (events, bytes)
    }

    /// Drop a peer's entries older than `cutoff`.
    fn prune(&mut self, pubkey: &PublicKey, cutoff: i64) {
        let Some(queue) = self.by_peer.get_mut(pubkey) else {
            return;
        };

        while queue.front().is_some_and(|(at, _)| *at < cutoff) {
            queue.pop_front();
        }

        if queue.is_empty() {
            self.by_peer.remove(pubkey);
        }
    }
}

/// The shared, thread-safe rolling ledger, metered across sessions.
#[derive(Debug, Default)]
pub struct SpendingLedger {
    inner: Mutex<Ledger>,
}

impl SpendingLedger {
    /// Record one accepted event's bytes against the peer, at the current
    /// instant. Prunes that peer's expired entries in the same turn.
    pub fn record(&self, pubkey: &PublicKey, bytes: usize) {
        self.inner
            .lock()
            .unwrap()
            .record(pubkey, crate::clock::now(), bytes as u64);
    }

    /// The accepted events and their bytes from `pubkey` within the rolling
    /// window, for testing the peer's quota across sessions.
    pub fn since(&self, pubkey: &PublicKey) -> (u32, u64) {
        self.inner
            .lock()
            .unwrap()
            .since(pubkey, crate::clock::now())
    }
}

/// One session's view onto the shared rolling window, keyed to its peer.
///
/// The ledger keys on the peer's first pubkey: a device proving several
/// identities is metered as one peer, though one that proves different key
/// sets across encounters splits its meter across them. Each split is still
/// bounded by the same window.
#[derive(Debug)]
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
    pub fn total(&self, peer: &Peer) -> (u32, u64) {
        match peer.pubkeys().next() {
            Some(pubkey) => self.ledger.since(pubkey),
            None => (0, 0),
        }
    }

    /// Count one accepted event against the rolling window.
    pub fn record(&mut self, peer: &Peer, event: &HashedEvent) {
        if let Some(pubkey) = peer.pubkeys().next().copied() {
            self.ledger.record(&pubkey, event_size(event));
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
            ledger.record(&author(1), 40);
            ledger.record(&author(1), 60);
            ledger.record(&author(2), 1);

            assert_eq!(ledger.since(&author(1)), (2, 100));
            assert_eq!(ledger.since(&author(2)), (1, 1));
        });
    }

    #[test]
    fn entries_older_than_the_window_are_pruned() {
        let ledger = SpendingLedger::default();

        // One entry well inside the window, one far inside it, one new.
        clock::at(1_000, || ledger.record(&author(1), 10));
        clock::at(2_000, || ledger.record(&author(1), 20));
        clock::at(1_000 + WINDOW_SECONDS + 1, || {
            ledger.record(&author(1), 30);

            // The first entry fell out of the window.
            assert_eq!(ledger.since(&author(1)), (2, 50));
        });
    }

    #[test]
    fn a_dormant_peer_leaves_no_rows_behind() {
        let ledger = SpendingLedger::default();

        clock::at(1_000, || {
            ledger.record(&author(1), 10);
        });
        clock::at(1_000 + WINDOW_SECONDS + 1, || {
            ledger.record(&author(2), 1);
            assert_eq!(ledger.since(&author(1)), (0, 0));
        });
    }

    #[test]
    fn an_accept_counts_once_against_the_shared_window() {
        let ledger = Arc::new(SpendingLedger::default());
        let peer = Peer::bind(LinkId(1), [author(2)], &Policy::new(author(1)));

        clock::at(1_000, || {
            // A past session already put one event in the window.
            ledger.record(&author(2), 10);

            // This session accepts one more; it lands in the same window and
            // is not double-counted against the session that accepted it.
            let mut spending = SessionSpending::new(Arc::clone(&ledger));
            let event = note(author(2), 100, "counted", coracle_lib::tags::Tags::new());
            spending.record(&peer, &event);

            let (events, bytes) = spending.total(&peer);
            assert_eq!(events, 2);
            assert!(bytes > 10);
        });
    }
}
