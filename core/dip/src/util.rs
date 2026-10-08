//! Small types that belong to no model.
//!
//! Nostr's own primitives are `coracle-lib`'s, and [`crate::clock`] holds the
//! clock. What is here is the handful of shapes this crate needs that are
//! neither nostr's nor any one domain's.

use std::fmt;

use serde::{Deserialize, Serialize};

/// A string printed as itself, without the quotes and escapes `str`'s own
/// `Debug` adds.
///
/// For hand-written `Debug` impls, where a field is a rendering rather than a
/// value: a redaction, a hex encoding, an abbreviation. `debug_struct` takes
/// `&dyn Debug` for each field. A bare `&str` there would come out quoted
/// and a `<redacted>` would read as a string someone stored.
pub(crate) struct Bare<'a>(pub &'a str);

impl fmt::Debug for Bare<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.0)
    }
}

/// A stretch of the day as minutes from local midnight.
///
/// Half-open: `start` is inside the window and `end` is not, and two windows
/// meeting at a minute do not overlap on it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Window {
    /// When it opens, in minutes from local midnight.
    pub start: u16,
    /// When it closes, in minutes from local midnight. Before `start` for a
    /// window that runs past midnight.
    pub end: u16,
}

impl Window {
    /// Whether `minute`, counted from local midnight, is inside the window.
    #[must_use]
    pub fn contains(self, minute: u16) -> bool {
        if self.start <= self.end {
            minute >= self.start && minute < self.end
        } else {
            minute >= self.start || minute < self.end
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bare_prints_a_string_as_itself() {
        assert_eq!(format!("{:?}", Bare("<redacted>")), "<redacted>");
        assert_eq!(format!("{:?}", "<redacted>"), "\"<redacted>\"");
    }

    #[test]
    fn bare_does_not_escape() {
        // The point of it: `3f8a1c04…` prints as itself rather than as an escaped string.
        assert_eq!(format!("{:?}", Bare("3f8a1c04…")), "3f8a1c04…");
    }

    #[test]
    fn a_window_is_half_open() {
        let day = Window {
            start: 480,
            end: 1080,
        };

        assert!(day.contains(480));
        assert!(day.contains(600));
        assert!(!day.contains(1080));
        assert!(!day.contains(60));
    }

    #[test]
    fn a_window_may_run_past_midnight() {
        let overnight = Window {
            start: 1_380,
            end: 360,
        };

        assert!(overnight.contains(1_400));
        assert!(overnight.contains(10));
        assert!(!overnight.contains(600));
    }

    #[test]
    fn a_window_that_opens_and_closes_together_is_empty() {
        // The one minute it names is excluded by its own end, because the window is half-open.
        let instant = Window {
            start: 600,
            end: 600,
        };

        assert!(!instant.contains(600));
    }
}
