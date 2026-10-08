//! The clock, mockable for tests.

use std::cell::Cell;

thread_local! {
    /// The instant this thread is pinned to, if any.
    static PINNED: Cell<Option<i64>> = const { Cell::new(None) };
}

/// The current unix second.
#[must_use]
pub fn now() -> i64 {
    PINNED
        .with(Cell::get)
        .unwrap_or_else(coracle_lib::util::now)
}

/// The minute of the current day, UTC.
///
/// `Policy::is_discoverable_at` reads the device's local time; this is a
/// stand-in until the shell reports a timezone offset for the core to fold in.
#[must_use]
pub fn minute_of_day() -> u16 {
    (now().rem_euclid(86_400) / 60) as u16
}

/// Run `f` with the clock pinned to `instant`.
pub fn at<T>(instant: i64, f: impl FnOnce() -> T) -> T {
    let _restore = Restore(PINNED.with(|pinned| pinned.replace(Some(instant))));

    f()
}

/// Puts the previous instant back, including while unwinding, so that a failing test
/// cannot leave the clock pinned for the ones after it.
struct Restore(Option<i64>);

impl Drop for Restore {
    fn drop(&mut self) {
        PINNED.with(|pinned| pinned.set(self.0));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_unpinned_clock_is_the_real_one() {
        assert!(now() > 1_700_000_000, "the wall clock is not plausible");
    }

    #[test]
    fn pinning_lasts_for_one_call() {
        assert_eq!(at(1_000, now), 1_000);
        assert!(now() > 1_700_000_000, "the pin outlived its scope");
    }

    #[test]
    fn pins_nest() {
        at(1_000, || {
            assert_eq!(at(2_000, now), 2_000);
            assert_eq!(now(), 1_000, "the inner pin did not restore the outer");
        });
    }

    #[test]
    fn a_panic_does_not_leave_the_clock_pinned() {
        let panicked = std::panic::catch_unwind(|| at(1_000, || panic!("boom")));

        assert!(panicked.is_err());
        assert!(now() > 1_700_000_000, "the pin survived a panic");
    }
}
