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

/// Minimum gap between connect attempts. The doc calls for roughly one per
/// 0.5s; the clock counts in whole seconds, so one per second.
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

/// Everything scheduling dials from an advertisement.
#[derive(Debug, Default)]
pub struct Scheduler {
    /// Candidates waiting for the floor, the cap, the rate limit or a backoff
    /// to clear, strongest first.
    candidates: Vec<(PeripheralId, i16)>,
    /// When each peripheral may next be dialed, and what set it.
    backoff: BTreeMap<PeripheralId, Backoff>,
    /// When the last connect attempt went out, for the global rate limit.
    last_attempt: Option<i64>,
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
    pub fn seen(&mut self, peripheral: PeripheralId, rssi: i16) {
        match self
            .candidates
            .iter()
            .position(|(candidate, _)| *candidate == peripheral)
        {
            Some(index) => {
                // A stronger reading replaces the queued one.
                if self.candidates[index].1 < rssi {
                    self.candidates[index] = (peripheral, rssi);
                }
            }
            None => {
                self.candidates.push((peripheral, rssi));
                self.candidates
                    .sort_by_key(|(_, rssi)| std::cmp::Reverse(*rssi));
            }
        }
    }

    /// The strongest admissible candidate, if the floor, the link cap, the
    /// rate limit and its backoff all allow a dial.
    pub fn next_dial(&mut self, open_links: usize) -> Option<PeripheralId> {
        if open_links >= MAX_LINKS {
            return None;
        }

        let now = clock::now();

        if self
            .last_attempt
            .is_some_and(|at| now - at < CONNECT_INTERVAL_SECONDS)
        {
            return None;
        }

        // Strongest first: the first candidate past the floor and not under a
        // backoff is dialed, and the rest stay queued for a later tick.
        for index in 0..self.candidates.len() {
            let (peripheral, rssi) = self.candidates[index].clone();

            if rssi < RSSI_FLOOR {
                continue;
            }

            let admissible = match self.backoff.get(&peripheral) {
                Some(backoff) => now >= backoff.until,
                None => true,
            };

            if admissible {
                self.candidates.remove(index);
                self.last_attempt = Some(now);
                // A dial that gets no answer shows up again as an
                // advertisement before this lapses; a dial that connects
                // supersedes it with the walked-away or declined tier.
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
