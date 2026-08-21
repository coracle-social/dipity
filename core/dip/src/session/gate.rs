//! The consent gate: whether a peer may proceed before anyone is named.
//!
//! What the gate protects is the disclosure of a nostr pubkey, which names a
//! long-term identity and, on a proximity transport, places it somewhere at a
//! given time. A peer the recognition exchange resolved passes or is blocked
//! on standing alone; a stranger is admitted by the cool-off window or a
//! discoverable time, and only while the disclosure budget has headroom.
//! `docs/discovery.md#the-consent-gate`.

use anyhow::Result;
use coracle_lib::keys::PublicKey;

use crate::clock;
use crate::db::Db;
use crate::model::{DISCLOSURE_WINDOW_SECONDS, Policy, Standing};

/// What the gate decided about a peer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    /// Admitted, or already paired.
    Pass,
    /// Blocked; drop the link before either side names itself.
    Blocked,
    /// An unadmitted stranger; hold for the user.
    Pending,
}

/// Where the app is, which is what the cool-off window is measured against.
///
/// `policy.md`: foregrounding the app starts it accepting connections, and the
/// cool-off is how long it keeps accepting them **after** it is backgrounded.
/// Timing the window from the moment of foregrounding instead would spend it
/// while the user was still looking at the screen — at the default, a user who
/// reads for ten minutes and pockets the phone would get no cool-off at all,
/// which is the one case the doc works through.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Presence {
    /// The user has the app open. Unknown peers are accepted.
    Foreground,
    /// Backgrounded at this instant, and the cool-off runs from it.
    Background {
        /// When the app went to the background.
        since: i64,
    },
}

/// One session's consent gate.
#[derive(Debug, Default)]
pub struct Gate {
    /// Whether the gate has passed, by policy or by the user's approval.
    passed: bool,
    /// Where the app is, or `None` if it has not been in the foreground this
    /// run — in which case there is no cool-off to spend.
    presence: Option<Presence>,
}

impl Gate {
    /// Where the app is, which the cool-off admission window is measured
    /// against.
    pub fn set_presence(&mut self, presence: Option<Presence>) {
        self.presence = presence;
    }

    /// Whether the gate has passed. Nothing that names an identity — a
    /// challenge answered, an `AUTH` response accepted — happens before it has.
    #[must_use]
    pub fn passed(&self) -> bool {
        self.passed
    }

    /// Pass the gate on the user's approval.
    pub fn pass(&mut self) {
        self.passed = true;
    }

    /// Judge the peer the recognition exchange resolved, recording a pass.
    pub fn evaluate(
        &mut self,
        db: &Db,
        policy: &Policy,
        resolved: &[PublicKey],
    ) -> Result<Verdict> {
        let verdict = if resolved.is_empty() {
            // A stranger is admitted by discoverability and the budget, or it
            // waits on the user.
            if self.admits_stranger(db, policy)? {
                Verdict::Pass
            } else {
                Verdict::Pending
            }
        } else if resolved
            .iter()
            .any(|pubkey| policy.graph.standing(pubkey) == Standing::Blocked)
        {
            // Blocked wins over every identity the tags resolved to, exactly as
            // it does once the peer authenticates: one device, and blocking any
            // of its pubkeys blocks the device. A blocked peer keeps its tags so
            // the connection can be dropped here, before either side names
            // itself.
            Verdict::Blocked
        } else {
            // Paired peers pass silently.
            Verdict::Pass
        };

        if verdict == Verdict::Pass {
            self.passed = true;
        }

        Ok(verdict)
    }

    /// Whether an unrecognized peer may proceed without a prompt: the cool-off
    /// window or a discoverable time admits it, and the disclosure budget has
    /// headroom.
    fn admits_stranger(&self, db: &Db, policy: &Policy) -> Result<bool> {
        let now = clock::now();
        let discoverable = policy.is_discoverable_at(clock::minute_of_day());
        let cool_off = match self.presence {
            Some(Presence::Foreground) => true,
            Some(Presence::Background { since }) => now - since < policy.cool_off_minutes * 60,
            None => false,
        };

        if !(discoverable || cool_off) {
            return Ok(false);
        }

        let spent = crate::db::query::disclosures_since(db, now - DISCLOSURE_WINDOW_SECONDS)?;

        Ok(spent < policy.disclosure_budget)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::fixtures::author;

    fn policy() -> Policy {
        Policy::new(author(1))
    }

    #[test]
    fn the_gate_blocks_a_resolved_blocked_peer() {
        let db = Db::open_in_memory().unwrap();
        let mut policy = policy();
        policy.graph.blocked.insert(author(2));
        let mut gate = Gate::default();

        assert_eq!(
            gate.evaluate(&db, &policy, &[author(2)]).unwrap(),
            Verdict::Blocked
        );
        assert!(!gate.passed());
    }

    #[test]
    fn one_blocked_identity_sinks_a_peer_that_proved_several() {
        // Pairing stores one secret against every pubkey a peer proved, so all
        // of them tag alike and the list resolves to the whole set. Judging
        // only whichever sorted first let a blocked identity through whenever
        // an unblocked one happened to sort ahead of it.
        let db = Db::open_in_memory().unwrap();
        let mut policy = policy();
        policy.graph.blocked.insert(author(3));
        let mut gate = Gate::default();

        // The blocked pubkey is not the one that sorts first.
        let mut resolved = [author(2), author(3)];
        resolved.sort();
        assert_ne!(resolved[0], author(3), "the fixture stopped being a test");

        assert_eq!(
            gate.evaluate(&db, &policy, &resolved).unwrap(),
            Verdict::Blocked
        );
        assert!(!gate.passed());
    }

    #[test]
    fn the_gate_passes_a_paired_peer_whatever_the_discoverability() {
        let db = Db::open_in_memory().unwrap();
        let mut gate = Gate::default();

        assert_eq!(
            gate.evaluate(&db, &policy(), &[author(2)]).unwrap(),
            Verdict::Pass
        );
        assert!(gate.passed());
    }

    #[test]
    fn the_gate_admits_a_stranger_during_the_cool_off() {
        let db = Db::open_in_memory().unwrap();
        let mut gate = Gate::default();
        gate.set_presence(Some(Presence::Foreground));

        assert_eq!(gate.evaluate(&db, &policy(), &[]).unwrap(), Verdict::Pass);
    }

    #[test]
    fn the_cool_off_runs_from_backgrounding_not_foregrounding() {
        // The doc's worked example: a user checks the app on a bus, reads for
        // longer than the cool-off, and pockets the phone. Timing the window
        // from foregrounding would have spent it before they put the phone
        // away, leaving the bus shut out rather than open for ten minutes.
        let db = Db::open_in_memory().unwrap();
        let policy = policy();
        let window = policy.cool_off_minutes * 60;
        let mut gate = Gate::default();

        // Open on the screen, well past the window: still accepting.
        gate.set_presence(Some(Presence::Foreground));
        assert_eq!(
            clock::at(10_000 + window * 2, || gate.evaluate(&db, &policy, &[])).unwrap(),
            Verdict::Pass
        );

        // Pocketed at 20_000: the window starts there.
        gate.set_presence(Some(Presence::Background { since: 20_000 }));
        assert_eq!(
            clock::at(20_000 + window - 1, || gate.evaluate(&db, &policy, &[])).unwrap(),
            Verdict::Pass
        );
        assert_eq!(
            clock::at(20_000 + window, || gate.evaluate(&db, &policy, &[])).unwrap(),
            Verdict::Pending
        );
    }

    #[test]
    fn the_gate_holds_a_stranger_without_admission() {
        let db = Db::open_in_memory().unwrap();
        let mut gate = Gate::default();

        assert_eq!(
            gate.evaluate(&db, &policy(), &[]).unwrap(),
            Verdict::Pending
        );
        assert!(!gate.passed());
    }

    #[test]
    fn the_gate_honors_the_disclosure_budget() {
        let db = Db::open_in_memory().unwrap();
        let mut policy = policy();
        policy.disclosure_budget = 1;
        let mut gate = Gate::default();
        gate.set_presence(Some(Presence::Foreground));

        // One stranger already disclosed to spends the budget of one.
        crate::db::command::pair_with(&db, &[author(9)], &[0u8; 32], clock::now()).unwrap();

        assert_eq!(gate.evaluate(&db, &policy, &[]).unwrap(), Verdict::Pending);
    }

    #[test]
    fn approval_passes_the_gate() {
        let mut gate = Gate::default();

        gate.pass();

        assert!(gate.passed());
    }
}
