//! Liveness for one link: when to prove this device is alive, and when to
//! conclude the peer no longer is.
//!
//! Liveness, not authorization. The transport already guarantees proximity,
//! which leaves the beat only cleanup to do, and cleanup can afford to be
//! lenient. `docs/discovery.md#heartbeat-and-teardown`.

use anyhow::{Context, Result};

use crate::clock;

/// How long without hearing from the peer before an idle session drains.
pub const TIMEOUT_SECONDS: i64 = 60;

/// The low bound on the jittered beat interval.
pub const MIN_INTERVAL_SECONDS: i64 = 15;

/// The high bound on the jittered beat interval.
pub const MAX_INTERVAL_SECONDS: i64 = 30;

/// One link's liveness clock: what was last heard, and when to beat next.
#[derive(Debug)]
pub struct Heartbeat {
    /// When a frame was last heard from the peer.
    pub last_heard: i64,
    /// When the next beat is due, so that a quiet session still proves it is alive
    /// before the peer's own timeout drains it.
    pub next_beat_at: i64,
}

impl Heartbeat {
    /// A heartbeat starting now, with the first beat jittered out.
    pub fn new() -> Result<Self> {
        let now = clock::now();

        Ok(Self {
            last_heard: now,
            next_beat_at: now + jittered_interval()?,
        })
    }

    /// A frame arrived, whatever it carried: the peer is alive.
    pub fn heard(&mut self) {
        self.last_heard = clock::now();
    }

    /// Whether the peer has been silent past the timeout.
    #[must_use]
    pub fn timed_out(&self) -> bool {
        clock::now() >= self.timeout_deadline()
    }

    /// Whether a beat is due.
    #[must_use]
    pub fn due(&self) -> bool {
        clock::now() >= self.next_beat_at
    }

    /// Push the next beat out one jittered interval — after beating, or after
    /// skipping one, so that a long transfer does not emit a beat the moment it
    /// drains.
    pub fn reschedule(&mut self) -> Result<()> {
        self.next_beat_at = clock::now() + jittered_interval()?;

        Ok(())
    }

    /// When the peer's silence would time this session out.
    #[must_use]
    pub fn timeout_deadline(&self) -> i64 {
        self.last_heard + TIMEOUT_SECONDS
    }
}

/// A fresh beat interval, jittered between the documented bounds.
///
/// An error rather than a fallback, because the fallback is a constant: two
/// devices that both lost their randomness would beat in lockstep, which is
/// the one thing the jitter exists to prevent.
fn jittered_interval() -> Result<i64> {
    let mut byte = [0u8; 1];
    getrandom::getrandom(&mut byte).context("jittering the heartbeat interval")?;

    Ok(MIN_INTERVAL_SECONDS
        + i64::from(byte[0] % (MAX_INTERVAL_SECONDS - MIN_INTERVAL_SECONDS + 1) as u8))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_silent_peer_times_out_at_the_boundary() {
        let heartbeat = clock::at(1_000, Heartbeat::new).unwrap();
        let timeout = 1_000 + TIMEOUT_SECONDS;

        assert!(clock::at(timeout - 1, || !heartbeat.timed_out()));
        assert!(clock::at(timeout, || heartbeat.timed_out()));
        assert_eq!(heartbeat.timeout_deadline(), timeout);
    }

    #[test]
    fn hearing_a_frame_resets_the_timeout() {
        let mut heartbeat = clock::at(1_000, Heartbeat::new).unwrap();

        clock::at(1_030, || heartbeat.heard());

        assert!(clock::at(1_000 + TIMEOUT_SECONDS, || !heartbeat.timed_out()));
        assert!(clock::at(1_030 + TIMEOUT_SECONDS, || heartbeat.timed_out()));
    }

    #[test]
    fn a_beat_comes_due_within_the_jittered_bounds() {
        let heartbeat = clock::at(1_000, Heartbeat::new).unwrap();

        assert!(clock::at(1_000 + MIN_INTERVAL_SECONDS - 1, || !heartbeat.due()));
        assert!(clock::at(1_000 + MAX_INTERVAL_SECONDS, || heartbeat.due()));
    }

    #[test]
    fn rescheduling_pushes_the_next_beat_out() {
        let mut heartbeat = clock::at(1_000, Heartbeat::new).unwrap();

        clock::at(2_000, || heartbeat.reschedule().unwrap());

        assert!(clock::at(2_000 + MIN_INTERVAL_SECONDS - 1, || !heartbeat.due()));
        assert!(clock::at(2_000 + MAX_INTERVAL_SECONDS, || heartbeat.due()));
    }
}
