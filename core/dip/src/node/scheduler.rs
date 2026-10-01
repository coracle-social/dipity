//! Deciding which advertised peers to dial, and when.
//!
//! Admission is one decision, made here: the RSSI floor, the link cap, the
//! global rate limit, and each peripheral's backoff. [`Node`](super::Node)
//! reports what the radio saw and how each dial turned out; the scheduler
//! answers with the next peripheral worth dialing. bitchat's
//! `BLEConnectionScheduler` is the reference.
//! `docs/discovery.md#connection-scheduling`.

use std::collections::BTreeMap;

use crate::clock;
use crate::link::PeripheralId;

/// Signal floor below which a candidate is queued rather than dialed.
pub const RSSI_FLOOR: i16 = -90;

/// How many central links may be open at once.
pub const MAX_LINKS: usize = 6;

/// Minimum gap between connect attempts, in the whole seconds the clock counts.
const CONNECT_INTERVAL_SECONDS: i64 = 1;

/// How long a peripheral that never answered a connect is left alone before
/// its advertisement may be dialed again.
pub(crate) const NEVER_ANSWERED_BACKOFF_SECONDS: i64 = 60;

/// How long after a connected peer walks away before redialing. They usually
/// come back, so this is short. `docs/discovery.md#connection-scheduling`.
pub(crate) const WALKED_AWAY_BACKOFF_SECONDS: i64 = 15;

/// How long after the core itself dropped a link before redialing.
///
/// Long enough that the redial does not undo the decision inside the same
/// encounter, short enough that a peer refused for a passing reason — a gate
/// that lapsed while the user was away — is not shut out for the length of a
/// whole gathering. `docs/discovery.md#connection-scheduling`.
pub(crate) const REFUSED_BACKOFF_SECONDS: i64 = 60;

/// How long after a consent-gate refusal before trying again. Hard, since the
/// user just said no.
pub(crate) const DECLINED_BACKOFF_SECONDS: i64 = 5 * 60;

/// How long a sighting stays a candidate. A peripheral id rotates about every
/// fifteen minutes, so one older than that names a device nobody can reach.
pub(crate) const CANDIDATE_TTL_SECONDS: i64 = 15 * 60;

/// Everything scheduling dials from an advertisement.
#[derive(Debug, Default)]
pub struct Scheduler {
    /// Candidates waiting for the floor, the cap, the rate limit or a backoff
    /// to clear, strongest first.
    candidates: Vec<Candidate>,
    /// When each peripheral may next be dialed, and what set it.
    backoff: BTreeMap<PeripheralId, Backoff>,
    /// Dials that went out and have not come up yet, by when they went. Each
    /// spends a link slot until it connects or its never-answered backoff
    /// lapses, so a crowd cannot bring up more links than the cap.
    dialing: BTreeMap<PeripheralId, i64>,
    /// When the last connect attempt went out, for the global rate limit.
    last_attempt: Option<i64>,
}

/// One advertised peripheral, as last heard.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Candidate {
    /// Which one.
    peripheral: PeripheralId,
    /// Its latest signal strength.
    rssi: i16,
    /// When it was last heard.
    seen_at: i64,
}

/// One peripheral's backoff: a deadline and the outcome that set it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Backoff {
    /// The earliest this peripheral may be dialed again.
    until: i64,
    /// Why it is backed off.
    tier: Tier,
}

/// What set a backoff, ordered by severity: a worse outcome is never
/// overwritten by a better one, so the user's "no" survives a disconnect
/// report arriving afterwards.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Tier {
    /// A dial that never answered.
    NeverAnswered,
    /// A peer that was connected and walked away.
    WalkedAway,
    /// A peer the core dropped: policy blocks it, its consent gate lapsed, or
    /// its frames stopped being ones the wire could carry.
    Refused,
    /// A peer the user declined at the consent gate.
    Declined,
}

impl Scheduler {
    /// Remember a sighting. A candidate below the floor or behind a backoff
    /// stays queued; [`next_dial`](Self::next_dial) decides what goes out.
    ///
    /// The latest reading stands, weaker or not: a peer walking away should
    /// sort behind one walking up.
    pub fn seen(&mut self, peripheral: PeripheralId, rssi: i16) {
        let seen_at = clock::now();

        self.candidates
            .retain(|candidate| candidate.peripheral != peripheral);
        self.candidates.push(Candidate {
            peripheral,
            rssi,
            seen_at,
        });
        self.candidates
            .sort_by_key(|candidate| std::cmp::Reverse(candidate.rssi));
    }

    /// A dial came up, so it no longer holds a slot as a pending one.
    pub fn connected(&mut self, peripheral: &PeripheralId) {
        self.dialing.remove(peripheral);
    }

    /// The strongest admissible candidate, if the floor, the link cap, the
    /// rate limit and its backoff all allow a dial. `open_links` counts the
    /// central links already up; dials still on their way count too.
    pub fn next_dial(&mut self, open_links: usize) -> Option<PeripheralId> {
        let now = clock::now();

        self.forget_stale(now);

        if open_links + self.dialing.len() >= MAX_LINKS {
            return None;
        }

        if self
            .last_attempt
            .is_some_and(|at| now - at < CONNECT_INTERVAL_SECONDS)
        {
            return None;
        }

        // Strongest first: the first candidate past the floor and off backoff is dialed.
        for index in 0..self.candidates.len() {
            let Candidate {
                peripheral, rssi, ..
            } = self.candidates[index].clone();

            if rssi < RSSI_FLOOR {
                continue;
            }

            let admissible = match self.backoff.get(&peripheral) {
                Some(backoff) => now >= backoff.until,
                None => true,
            };

            if admissible {
                self.candidates.remove(index);
                self.dialing.insert(peripheral.clone(), now);
                self.last_attempt = Some(now);
                // No answer re-advertises before this lapses; a connect supersedes it.
                self.backoff.insert(
                    peripheral.clone(),
                    Backoff {
                        until: now + NEVER_ANSWERED_BACKOFF_SECONDS,
                        tier: Tier::NeverAnswered,
                    },
                );

                return Some(peripheral);
            }
        }

        None
    }

    /// Forget candidates nobody has heard from in a rotation, dials that never
    /// came up, and backoffs that have lapsed, so none of them grows with every
    /// peripheral id a crowd rotates through.
    fn forget_stale(&mut self, now: i64) {
        self.candidates
            .retain(|candidate| now - candidate.seen_at < CANDIDATE_TTL_SECONDS);
        self.dialing
            .retain(|_, at| now - *at < NEVER_ANSWERED_BACKOFF_SECONDS);
        self.backoff.retain(|_, backoff| now < backoff.until);
    }

    /// A peer was connected and left: redial soon, since they usually come
    /// back.
    pub fn walked_away(&mut self, peripheral: &PeripheralId) {
        self.record(peripheral, Tier::WalkedAway, WALKED_AWAY_BACKOFF_SECONDS);
    }

    /// A peer this device dropped rather than lost: hold off longer than a
    /// walk-away, so the redial does not undo the decision.
    pub fn refused(&mut self, peripheral: &PeripheralId) {
        self.record(peripheral, Tier::Refused, REFUSED_BACKOFF_SECONDS);
    }

    /// A peer whose gate was refused: leave them alone.
    pub fn declined(&mut self, peripheral: &PeripheralId) {
        self.record(peripheral, Tier::Declined, DECLINED_BACKOFF_SECONDS);
    }

    /// Merge one outcome into a peripheral's backoff.
    ///
    /// A worse tier always wins and a better one never displaces it; within a
    /// tier the later deadline stands. The one shortening allowed is
    /// walked-away over never-answered: that dial answered, just not for long.
    fn record(&mut self, peripheral: &PeripheralId, tier: Tier, seconds: i64) {
        let deadline = clock::now() + seconds;

        let Some(backoff) = self.backoff.get_mut(peripheral) else {
            self.backoff.insert(
                peripheral.clone(),
                Backoff {
                    until: deadline,
                    tier,
                },
            );
            return;
        };

        match tier.cmp(&backoff.tier) {
            std::cmp::Ordering::Less => {}
            std::cmp::Ordering::Equal => backoff.until = backoff.until.max(deadline),
            std::cmp::Ordering::Greater => {
                backoff.until = if backoff.tier == Tier::NeverAnswered && tier == Tier::WalkedAway {
                    deadline
                } else {
                    backoff.until.max(deadline)
                };
                backoff.tier = tier;
            }
        }
    }
}
