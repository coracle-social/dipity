//! Everything a query over stored events can be narrowed by.

use coracle_lib::filters::Filter;

use crate::model::{Order, PeerPolicy, Registers, Seen};

/// Everything a query over stored events can be narrowed by.
///
/// Four constraints, kept apart because they are answerable by different
/// parties and only the first may ever be shown to one:
///
/// | Field | Is | Comes from |
/// | --- | --- | --- |
/// | [`filter`](Self::filter) | a NIP-01 filter | the caller, or a peer's `REQ` |
/// | [`seen`](Self::seen) | local arrival criteria | the view |
/// | [`registers`](Self::registers) | how far the event may travel | `docs/proofs.md` |
/// | [`policy`](Self::policy) | who may be served what | the user's preferences |
///
/// A query for the user's own screen sets the first two and leaves the rest
/// alone. A query answering a peer sets all four.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct EventFilter {
    /// The NIP-01 filter: the half of the criteria that may go on the wire.
    pub filter: Filter,
    /// Local arrival criteria: the half that must not.
    pub seen: Seen,
    /// Which authorship registers qualify. `None` is no constraint, which is
    /// right for a local read and wrong for anything being sent.
    pub registers: Option<Registers>,
    /// The policy governing the peer being answered. `None` when the answer is
    /// for the user rather than for a peer.
    pub policy: Option<PeerPolicy>,
    /// What the result is ordered by.
    pub order: Order,
}

impl EventFilter {
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

    /// Narrow by local arrival criteria.
    #[must_use]
    pub fn with_seen(mut self, seen: Seen) -> Self {
        self.seen = seen;
        self
    }

    /// Restrict to events first seen at or after `since`.
    #[must_use]
    pub fn add_seen_since(mut self, since: i64) -> Self {
        self.seen = self.seen.add_since(since);
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
            || self.seen.matches_nothing()
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
        assert!(!EventFilter::new().matches_nothing());

        // Whichever part of the criteria is unsatisfiable, the whole is.
        assert!(
            EventFilter::new()
                .with_filter(Filter::new().add_authors([]))
                .matches_nothing()
        );
        assert!(
            EventFilter::new()
                .with_seen(Seen::new().add_peers([]))
                .matches_nothing()
        );
        assert!(
            EventFilter::new()
                .with_registers(Registers::new(author(1), []))
                .matches_nothing()
        );

        let mut policy = Policy::new(author(1));
        policy.graph.blocked.insert(author(2));

        assert!(
            EventFilter::new()
                .with_policy(policy.clone().for_peer(author(2)))
                .matches_nothing()
        );
        assert!(
            !EventFilter::new()
                .with_policy(policy.for_peer(author(3)))
                .matches_nothing()
        );
    }
}
