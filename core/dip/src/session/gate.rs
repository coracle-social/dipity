//! The consent gate: whether a peer may proceed before anyone is named.
//!
//! What the gate protects is the disclosure of a nostr pubkey, which names a
//! long-term identity and, on a proximity transport, places it somewhere at a
//! given time. A peer the recognition exchange resolved passes or is blocked
//! on standing alone; a stranger is admitted while the disclosure bucket has
//! room and it is not a quiet time, whether the app is open or in a pocket.
//! `docs/discovery.md#the-consent-gate`.
//!
//! The gate reads the bucket; the session spends from it, when it answers the
//! peer's challenge. Charging on admission instead would let a stranger that
//! connects and drops without ever asking for an identity close the device to
//! everyone else.

use anyhow::Result;
use coracle_lib::keys::PublicKey;

use crate::clock;
use crate::db::Db;
use crate::model::{Policy, Standing};

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

/// Where the app is. Strangers are admitted the same either way; what needs
/// somebody in front of the screen is moving an identity between phones.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Presence {
    /// The user has the app open.
    Foreground,
    /// The app is in the background.
    Background,
}

/// One session's consent gate.
#[derive(Debug, Default)]
pub struct Gate {
    /// Whether the gate has passed, by policy or by the user's approval.
    /// Nothing that names an identity — a challenge answered, an `AUTH`
    /// response accepted — happens before it has.
    pub passed: bool,
    /// Whether the peer got in as an unrecognized stranger on the disclosure
    /// bucket alone, which is what spends from it. A recognized peer passes on
    /// standing and never consulted the bucket; a stranger the user approved
    /// by hand was looked at by the person the bucket stands in for.
    pub spends_budget: bool,
    /// Where the app is, or `None` if the shell has not said this run.
    pub presence: Option<Presence>,
}

impl Gate {
    /// Judge the peer the recognition exchange resolved, recording a pass.
    pub fn evaluate(
        &mut self,
        db: &Db,
        policy: &Policy,
        resolved: &[PublicKey],
    ) -> Result<Verdict> {
        let verdict = if resolved.is_empty() {
            // A stranger is admitted by the bucket, or waits on the user.
            if self.admits_stranger(db, policy)? {
                self.spends_budget = true;
                Verdict::Pass
            } else {
                Verdict::Pending
            }
        } else if resolved
            .iter()
            .any(|pubkey| policy.graph.standing(pubkey) == Standing::Blocked)
        {
            // Blocked wins over every identity the tags resolved to: one device, one decision.
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

    /// Whether an unrecognized peer may proceed without a prompt: it is not a
    /// quiet time, and the disclosure bucket has room.
    fn admits_stranger(&self, db: &Db, policy: &Policy) -> Result<bool> {
        if policy.is_quiet_at(clock::minute_of_day()) {
            return Ok(false);
        }

        let now = clock::now();
        let bucket = crate::db::query::disclosure_bucket(db, now)?;

        Ok(bucket.has_room(policy.strangers_per_day, now))
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
        assert!(!gate.passed);
    }

    #[test]
    fn one_blocked_identity_sinks_a_peer_that_proved_several() {
        // Judging only the pubkey that sorted first let a blocked identity through.
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
        assert!(!gate.passed);
    }

    #[test]
    fn the_gate_passes_a_paired_peer_whatever_the_discoverability() {
        let db = Db::open_in_memory().unwrap();
        let mut gate = Gate::default();

        assert_eq!(
            gate.evaluate(&db, &policy(), &[author(2)]).unwrap(),
            Verdict::Pass
        );
        assert!(gate.passed);
    }

    #[test]
    fn the_gate_admits_a_stranger_in_the_background_while_the_bucket_has_room() {
        let db = Db::open_in_memory().unwrap();
        let mut gate = Gate {
            presence: Some(Presence::Background),
            ..Default::default()
        };

        assert_eq!(
            clock::at(86_400, || gate.evaluate(&db, &policy(), &[])).unwrap(),
            Verdict::Pass
        );
    }

    #[test]
    fn the_gate_holds_a_stranger_once_the_bucket_is_empty() {
        let db = Db::open_in_memory().unwrap();
        let policy = policy();
        let mut gate = Gate::default();

        for _ in 0..3 {
            crate::db::command::spend_disclosure(&db, policy.strangers_per_day, clock::now())
                .unwrap();
        }

        assert_eq!(gate.evaluate(&db, &policy, &[]).unwrap(), Verdict::Pending);
        assert!(!gate.passed);
        assert!(!gate.spends_budget);
    }

    #[test]
    fn the_gate_holds_a_stranger_in_a_quiet_time() {
        let db = Db::open_in_memory().unwrap();
        let mut policy = policy();
        policy.quiet_times.push(crate::util::Window {
            start: 0,
            end: 1_439,
        });
        let mut gate = Gate::default();

        assert_eq!(gate.evaluate(&db, &policy, &[]).unwrap(), Verdict::Pending);
    }

    #[test]
    fn only_a_stranger_spends_the_budget() {
        let db = Db::open_in_memory().unwrap();
        let mut gate = Gate {
            presence: Some(Presence::Foreground),
            ..Default::default()
        };

        assert_eq!(gate.evaluate(&db, &policy(), &[]).unwrap(), Verdict::Pass);
        assert!(gate.spends_budget);

        let mut recognized = Gate {
            presence: Some(Presence::Foreground),
            ..Default::default()
        };

        assert_eq!(
            recognized.evaluate(&db, &policy(), &[author(2)]).unwrap(),
            Verdict::Pass
        );
        assert!(!recognized.spends_budget);
    }
}
