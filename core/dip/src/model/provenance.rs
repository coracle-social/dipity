//! Where an event came from, and selecting on it.

use std::collections::BTreeSet;

use coracle_lib::keys::PublicKey;

/// One sighting: an event, a peer it was seen from, and when.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Provenance {
    /// The event seen, as a lowercase hex id.
    pub event_id: String,
    /// The peer it came from.
    pub peer_pubkey: PublicKey,
    /// When it arrived, by the local clock.
    pub seen_at: i64,
}

/// Criteria over an event's sightings: when it arrived, and who from.
///
/// The relay protocol cannot express these, and must not — they read
/// [`Provenance`], which records the user's movements and who they were with,
/// and is never served to a peer. So they are kept off the
/// [`Filter`](coracle_lib::filters::Filter) that may go on the wire, and are
/// never serialized. See `docs/privacy.md`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ProvenanceFilter {
    /// Lower bound on the event's seen time, inclusive.
    pub since: Option<i64>,
    /// Upper bound on the event's seen time, inclusive.
    pub until: Option<i64>,
    /// Restrict to events seen from at least one of these peers.
    pub peers: Option<BTreeSet<PublicKey>>,
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

    /// Restrict to events seen from any of `peers`. An empty set matches
    /// nothing, as it does on a filter.
    #[must_use]
    pub fn add_peers(mut self, peers: impl IntoIterator<Item = PublicKey>) -> Self {
        self.peers.get_or_insert_with(BTreeSet::new).extend(peers);
        self
    }

    /// Whether these criteria are unsatisfiable, the way
    /// [`Filter::matches_nothing`](coracle_lib::filters::Filter::matches_nothing)
    /// is: an empty peer set, or a window that ends before it starts.
    #[must_use]
    pub fn matches_nothing(&self) -> bool {
        self.peers.as_ref().is_some_and(BTreeSet::is_empty)
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
        assert!(ProvenanceFilter::new().add_peers([]).matches_nothing());
    }
}
