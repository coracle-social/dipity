//! The disclosure bucket: how many strangers this device tells who it is.
//!
//! Every time the device answers a stranger's `AUTH` challenge it hands over a
//! pubkey at a place and a time, which is one sample for anybody trying to
//! follow it. Nothing else the protocol discloses outlives a session, so the
//! rate of these samples is what tracking costs, whoever and however many the
//! strangers are. A token bucket bounds that rate twice: a few at once, and on
//! average no more than [`STRANGERS_PER_DAY`], spread across the day rather
//! than spent in one place. It applies only while the app is not in front: a
//! user looking at the screen is there to meet whoever is nearby.
//! `docs/policy.md#discoverability`.

/// How many strangers a day, on average, the device tells who the user is
/// while the app is not in front.
pub const STRANGERS_PER_DAY: u32 = 12;

/// The most disclosures the bucket holds, which is how many can happen at once.
pub const BURST: f64 = 3.0;

const DAY_SECONDS: f64 = 86_400.0;

/// The bucket's level at a moment. A level below zero is debt, from sessions
/// that passed the gate together and each disclosed.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DisclosureBucket {
    /// How many disclosures were available at [`at`](Self::at).
    pub tokens: f64,
    /// When the level was last written.
    pub at: i64,
}

impl DisclosureBucket {
    /// A bucket nothing has been spent from.
    #[must_use]
    pub fn full(at: i64) -> Self {
        Self { tokens: BURST, at }
    }

    /// How many disclosures are available at `now`, refilled at `per_day`.
    #[must_use]
    pub fn level(self, per_day: u32, now: i64) -> f64 {
        let elapsed = (now - self.at).max(0) as f64;
        let refilled = self.tokens + elapsed * f64::from(per_day) / DAY_SECONDS;

        refilled.min(capacity(per_day))
    }

    /// Whether a stranger may be told who this device is at `now`.
    #[must_use]
    pub fn has_room(self, per_day: u32, now: i64) -> bool {
        per_day > 0 && self.level(per_day, now) >= 1.0
    }

    /// The bucket after one disclosure at `now`.
    #[must_use]
    pub fn spend(self, per_day: u32, now: i64) -> Self {
        Self {
            tokens: self.level(per_day, now) - 1.0,
            at: now,
        }
    }
}

/// The burst, or the whole day's allowance when that is smaller.
fn capacity(per_day: u32) -> f64 {
    BURST.min(f64::from(per_day))
}

#[cfg(test)]
mod tests {
    use super::*;

    const HOUR: i64 = 3_600;

    #[test]
    fn a_full_bucket_allows_a_burst_and_then_nothing() {
        let mut bucket = DisclosureBucket::full(0);

        for _ in 0..3 {
            assert!(bucket.has_room(12, 0));
            bucket = bucket.spend(12, 0);
        }

        assert!(!bucket.has_room(12, 0));
    }

    #[test]
    fn an_empty_bucket_refills_one_disclosure_per_share_of_the_day() {
        let empty = DisclosureBucket { tokens: 0.0, at: 0 };

        // Twelve a day is one every two hours.
        assert!(!empty.has_room(12, 2 * HOUR - 1));
        assert!(empty.has_room(12, 2 * HOUR));
    }

    #[test]
    fn a_day_of_steady_spending_never_passes_the_daily_number_plus_the_burst() {
        let mut bucket = DisclosureBucket::full(0);
        let mut disclosed = 0;

        for minute in 0..24 * 60 {
            let now = minute * 60;

            if bucket.has_room(12, now) {
                bucket = bucket.spend(12, now);
                disclosed += 1;
            }
        }

        assert!(disclosed <= 12 + 3, "{disclosed} disclosures in a day");
    }

    #[test]
    fn a_small_allowance_caps_the_burst_too() {
        let mut bucket = DisclosureBucket::full(0);

        bucket = bucket.spend(1, 0);

        assert!(!bucket.has_room(1, 0));
    }

    #[test]
    fn no_strangers_a_day_means_none_at_all() {
        assert!(!DisclosureBucket::full(0).has_room(0, 0));
    }

    #[test]
    fn debt_is_repaid_before_another_disclosure() {
        let owing = DisclosureBucket {
            tokens: -1.0,
            at: 0,
        };

        assert!(!owing.has_room(12, 2 * HOUR));
        assert!(owing.has_room(12, 4 * HOUR));
    }
}
