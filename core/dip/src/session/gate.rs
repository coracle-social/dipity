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

/// One session's consent gate.
#[derive(Debug, Default)]
pub struct Gate {
    /// Whether the gate has passed, by policy or by the user's approval.
    passed: bool,
    /// When the app was last foregrounded, for the cool-off admission window.
    cool_off_since: Option<i64>,
}

impl Gate {
    /// When the app was last foregrounded, which the cool-off admission window
    /// is measured from. `None` means the cool-off is not running.
    pub fn set_cool_off_since(&mut self, since: Option<i64>) {
        self.cool_off_since = since;
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
        resolved: Option<PublicKey>,
    ) -> Result<Verdict> {
        let verdict = match resolved {
            // Paired peers pass silently; a blocked one keeps its tags so the
            // connection can be dropped here, before either side names itself.
            Some(pubkey) => {
                if policy.graph.standing(&pubkey) == Standing::Blocked {
                    Verdict::Blocked
                } else {
                    Verdict::Pass
                }
            }
            // A stranger is admitted by discoverability and the budget, or it
            // waits on the user.
            None => {
                if self.admits_stranger(db, policy)? {
                    Verdict::Pass
                } else {
                    Verdict::Pending
                }
            }
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
        let cool_off = self
            .cool_off_since
            .is_some_and(|since| now - since < policy.cool_off_minutes * 60);

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
            gate.evaluate(&db, &policy, Some(author(2))).unwrap(),
            Verdict::Blocked
        );
        assert!(!gate.passed());
    }

    #[test]
    fn the_gate_passes_a_paired_peer_whatever_the_discoverability() {
        let db = Db::open_in_memory().unwrap();
        let mut gate = Gate::default();

        assert_eq!(
            gate.evaluate(&db, &policy(), Some(author(2))).unwrap(),
            Verdict::Pass
        );
        assert!(gate.passed());
    }

    #[test]
    fn the_gate_admits_a_stranger_during_the_cool_off() {
        let db = Db::open_in_memory().unwrap();
        let mut gate = Gate::default();
        gate.set_cool_off_since(Some(clock::now()));

        assert_eq!(gate.evaluate(&db, &policy(), None).unwrap(), Verdict::Pass);
    }

    #[test]
    fn the_gate_holds_a_stranger_without_admission() {
        let db = Db::open_in_memory().unwrap();
        let mut gate = Gate::default();

        assert_eq!(
            gate.evaluate(&db, &policy(), None).unwrap(),
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
        gate.set_cool_off_since(Some(clock::now()));

        // One stranger already disclosed to spends the budget of one.
        crate::db::command::pair_with(&db, &[author(9)], &[0u8; 32], clock::now()).unwrap();

        assert_eq!(gate.evaluate(&db, &policy, None).unwrap(), Verdict::Pending);
    }

    #[test]
    fn approval_passes_the_gate() {
        let mut gate = Gate::default();

        gate.pass();

        assert!(gate.passed());
    }
}
