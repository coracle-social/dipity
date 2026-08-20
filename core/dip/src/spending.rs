//! The rolling 24 h spending ledger, in memory.
//!
//! What a peer has written to this device recently, metered against its quota
//! across sessions. It lives in memory and dies with the process: durable
//! billing rows of "who handed me how much, when" are exactly the provenance
//! the app refuses to keep (`docs/privacy.md`), and a rolling meter that resets
//! on restart costs nothing, since a peer cannot kill the process to refill
//! it. The window is bounded — each peer's entries older than it are pruned on
//! every write, so the ledger cannot grow without bound either; entries
//! outside the window cost no disk, no index and no scan.

use std::collections::{BTreeMap, VecDeque};
use std::sync::Mutex;

use coracle_lib::keys::PublicKey;

/// How far back spending counts, in seconds.
pub const WINDOW_SECONDS: i64 = 24 * 60 * 60;

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

/// The shared, thread-safe ledger the sync layer meters against.
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

#[cfg(test)]
mod tests {
    use super::*;

    use crate::clock;
    use crate::fixtures::author;

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
}
