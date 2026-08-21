//! Where an event came from, and selecting on it.

use std::collections::BTreeSet;

use coracle_lib::events::EventId;
use coracle_lib::keys::PublicKey;

/// One sighting: an event, a pubkey it was seen from, and when.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Provenance {
    /// The event seen.
    pub event_id: EventId,
    /// The pubkey of the peer it came from.
    pub pubkey: PublicKey,
    /// When it arrived, by the local clock.
    pub seen_at: i64,
}

/// Criteria over an event's sightings: when it arrived, and who from.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ProvenanceFilter {
    /// Lower bound on the event's seen time, inclusive.
    pub since: Option<i64>,
    /// Upper bound on the event's seen time, inclusive.
    pub until: Option<i64>,
    /// Restrict to events seen from at least one of these pubkeys.
    pub pubkeys: Option<BTreeSet<PublicKey>>,
}

impl ProvenanceFilter {
    /// No constraint.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Restrict to events first seen at or after `since`.
    #[must_use]
    pub fn add_since(mut self, since: i64) -> Self {
        self.since = Some(since);
        self
    }

    /// Restrict to events first seen at or before `until`.
    #[must_use]
    pub fn add_until(mut self, until: i64) -> Self {
        self.until = Some(until);
        self
    }

    /// Restrict to events seen from any of `pubkeys`. An empty set matches
    /// nothing, as it does on a filter.
    #[must_use]
    pub fn add_pubkeys(mut self, pubkeys: impl IntoIterator<Item = PublicKey>) -> Self {
        self.pubkeys
            .get_or_insert_with(BTreeSet::new)
            .extend(pubkeys);
        self
    }

    /// Whether these criteria are unsatisfiable, the way
    /// [`Filter::matches_nothing`](coracle_lib::filters::Filter::matches_nothing)
    /// is: an empty pubkey set, or a window that ends before it starts.
    #[must_use]
    pub fn matches_nothing(&self) -> bool {
        self.pubkeys.as_ref().is_some_and(BTreeSet::is_empty)
            || matches!((self.since, self.until), (Some(s), Some(u)) if s > u)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unsatisfiable_criteria_are_reported() {
        assert!(!ProvenanceFilter::new().matches_nothing());
        assert!(
            !ProvenanceFilter::new()
                .add_since(100)
                .add_until(200)
                .matches_nothing()
        );
        assert!(
            ProvenanceFilter::new()
                .add_since(200)
                .add_until(100)
                .matches_nothing()
        );
        assert!(ProvenanceFilter::new().add_pubkeys([]).matches_nothing());
    }
}
