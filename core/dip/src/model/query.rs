//! A question asked of the stored events.

use coracle_lib::filters::Filter;

use crate::model::{Order, PeerPolicy, ProvenanceFilter, Registers};

/// Everything a read of stored events can be narrowed and ordered by.
///
/// | Field | Is | Comes from |
/// | --- | --- | --- |
/// | [`filter`](Self::filter) | a NIP-01 filter | the caller, or a peer's `REQ` |
/// | [`provenance`](Self::provenance) | where it came from | the view |
/// | [`registers`](Self::registers) | how far the event may travel | `docs/proofs.md` |
/// | [`policy`](Self::policy) | who may be served what | the user's preferences |
///
/// A query for the user's own screen sets the first two and leaves the rest
/// alone. A query answering a peer sets all four.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Query {
    /// The NIP-01 filter: the half of the criteria that may go on the wire.
    pub filter: Filter,
    /// Criteria over where the event came from: the half that must not.
    pub provenance: ProvenanceFilter,
    /// Which authorship registers qualify. `None` is no constraint, which is
    /// right for a local read and wrong for anything being sent.
    pub registers: Option<Registers>,
    /// The policy governing the peer being answered. `None` when the answer is
    /// for the user rather than for a peer.
    pub policy: Option<PeerPolicy>,
    /// What the result is ordered by.
    pub order: Order,
}

impl Query {
    /// Everything the store holds, newest by `created_at` first.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Narrow by a NIP-01 filter.
    #[must_use]
    pub fn with_filter(mut self, filter: Filter) -> Self {
        self.filter = filter;
        self
    }

    /// Narrow by where the event came from.
    #[must_use]
    pub fn with_provenance(mut self, provenance: ProvenanceFilter) -> Self {
        self.provenance = provenance;
        self
    }

    /// Restrict to events first seen at or after `since`.
    #[must_use]
    pub fn add_seen_since(mut self, since: i64) -> Self {
        self.provenance = self.provenance.add_since(since);
        self
    }

    /// Restrict to events sitting in one of `registers`.
    #[must_use]
    pub fn with_registers(mut self, registers: Registers) -> Self {
        self.registers = Some(registers);
        self
    }

    /// Apply the policy governing the peer being answered.
    #[must_use]
    pub fn with_policy(mut self, policy: PeerPolicy) -> Self {
        self.policy = Some(policy);
        self
    }

    /// Order the result by `order`.
    #[must_use]
    pub fn with_order(mut self, order: Order) -> Self {
        self.order = order;
        self
    }

    /// Whether these criteria can match anything at all.
    #[must_use]
    pub fn matches_nothing(&self) -> bool {
        self.filter.matches_nothing()
            || self.provenance.matches_nothing()
            || self
                .registers
                .as_ref()
                .is_some_and(Registers::matches_nothing)
            || self.policy.as_ref().is_some_and(PeerPolicy::is_blocked)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::fixtures::author;
    use crate::model::Policy;

    #[test]
    fn every_part_of_a_query_answers_for_itself() {
        assert!(!Query::new().matches_nothing());

        // Whichever part of the criteria is unsatisfiable, the whole is.
        assert!(
            Query::new()
                .with_filter(Filter::new().add_authors([]))
                .matches_nothing()
        );
        assert!(
            Query::new()
                .with_provenance(ProvenanceFilter::new().add_pubkeys([]))
                .matches_nothing()
        );
        assert!(
            Query::new()
                .with_registers(Registers::new(&[author(1)], []))
                .matches_nothing()
        );

        let mut policy = Policy::new(author(1));
        policy.graph.blocked.insert(author(2));

        assert!(
            Query::new()
                .with_policy(policy.clone().for_pubkey(author(2)))
                .matches_nothing()
        );
        assert!(
            !Query::new()
                .with_policy(policy.for_pubkey(author(3)))
                .matches_nothing()
        );
    }
}
