//! The consent gate: whether a peer may proceed before anyone is named.
//!
//! What the gate protects is the disclosure of a nostr pubkey, which names a
//! long-term identity and, on a proximity transport, places it somewhere at a
//! given time. A peer the recognition exchange resolved passes or is blocked
//! on standing alone. A stranger is held in a quiet time; otherwise one is
//! admitted outright while the app is in front, where the user is there to
//! meet whoever is nearby, and from the disclosure bucket while it is not, if
//! the user allows discovery in the background at all.
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
use crate::model::{Policy, STRANGERS_PER_DAY, Standing};

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

/// Where the app is: in front, a stranger costs the disclosure bucket nothing,
/// and moving an identity between phones needs somebody looking.
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
    /// bucket alone, which is what spends from it. A recognized peer, a
    /// stranger met with the app in front, and one the user approved by hand
    /// spend nothing.
    pub spends_budget: bool,
    /// Whether a receiver let the exchange run on to see who the dialer is
    /// before holding them, which it may because the dialer discloses first.
    pub deferred: bool,
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
            // A stranger is admitted outright, by the bucket, or waits on the user.
            match self.admits_stranger(db, policy)? {
                Admission::Free => Verdict::Pass,
                Admission::Budget => {
                    self.spends_budget = true;
                    Verdict::Pass
                }
                Admission::Held => Verdict::Pending,
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

    /// How an unrecognized peer may proceed without a prompt, if it may. An
    /// app the shell has not reported on counts as not in front.
    fn admits_stranger(&self, db: &Db, policy: &Policy) -> Result<Admission> {
        if policy.is_quiet_at(clock::minute_of_day()) {
            return Ok(Admission::Held);
        }

        if self.presence == Some(Presence::Foreground) {
            return Ok(Admission::Free);
        }

        if !policy.discover_in_background {
            return Ok(Admission::Held);
        }

        let now = clock::now();
        let bucket = crate::db::query::disclosure_bucket(db, now)?;

        Ok(if bucket.has_room(STRANGERS_PER_DAY, now) {
            Admission::Budget
        } else {
            Admission::Held
        })
    }
}

/// How a stranger gets past the gate.
enum Admission {
    /// Outright, with the app in front.
    Free,
    /// On the disclosure bucket, which the session spends from.
    Budget,
    /// Not without the user.
    Held,
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
            crate::db::command::spend_disclosure(&db, STRANGERS_PER_DAY, clock::now()).unwrap();
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
    fn only_a_stranger_met_in_the_background_spends_the_budget() {
        let db = Db::open_in_memory().unwrap();
        let mut gate = Gate {
            presence: Some(Presence::Background),
            ..Default::default()
        };

        assert_eq!(gate.evaluate(&db, &policy(), &[]).unwrap(), Verdict::Pass);
        assert!(gate.spends_budget);

        let mut recognized = Gate {
            presence: Some(Presence::Background),
            ..Default::default()
        };

        assert_eq!(
            recognized.evaluate(&db, &policy(), &[author(2)]).unwrap(),
            Verdict::Pass
        );
        assert!(!recognized.spends_budget);
    }

    #[test]
    fn a_stranger_met_with_the_app_in_front_is_admitted_whatever_the_bucket() {
        let db = Db::open_in_memory().unwrap();

        for _ in 0..3 {
            crate::db::command::spend_disclosure(&db, STRANGERS_PER_DAY, clock::now()).unwrap();
        }

        let mut gate = Gate {
            presence: Some(Presence::Foreground),
            ..Default::default()
        };

        assert_eq!(gate.evaluate(&db, &policy(), &[]).unwrap(), Verdict::Pass);
        assert!(!gate.spends_budget);
    }

    #[test]
    fn with_background_discovery_off_a_stranger_waits_unless_the_app_is_in_front() {
        let db = Db::open_in_memory().unwrap();
        let mut policy = policy();
        policy.discover_in_background = false;
        let mut behind = Gate {
            presence: Some(Presence::Background),
            ..Default::default()
        };
        let mut in_front = Gate {
            presence: Some(Presence::Foreground),
            ..Default::default()
        };

        assert_eq!(
            behind.evaluate(&db, &policy, &[]).unwrap(),
            Verdict::Pending
        );
        assert_eq!(in_front.evaluate(&db, &policy, &[]).unwrap(), Verdict::Pass);
    }
}
