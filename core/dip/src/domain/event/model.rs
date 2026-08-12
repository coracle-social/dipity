//! What the store adds to a nostr event: where it came from, and how to select
//! on that.
//!
//! The event itself is `coracle-lib`'s. Specifically a
//! [`HashedEvent`](coracle_lib::events::HashedEvent) — the stage with an id and
//! no signature — because that is exactly what this app stores. Content events
//! are never signed, and kind 22242 auth events, the one signed kind it
//! produces, are ephemeral and never reach a table. The type says so, so no
//! column has to.
//!
//! [`Filter`](coracle_lib::filters::Filter) is `coracle-lib`'s too, and is what
//! goes on the wire. [`Seen`] is the part that must not: it reads provenance,
//! which never leaves the device. [`EventFilter`] is the two together, plus the
//! constraints that are neither — the authorship [`Registers`] an event has to
//! sit in, and the [`PeerPolicy`] governing the peer being answered — so that
//! one query answers every caller and none of them assembles the constraints
//! itself.
//!
//! An event's address comes from
//! [`EventExtensionAddress`](coracle_lib::addresses::EventExtensionAddress),
//! which the library's prelude carries.

use std::collections::BTreeSet;

use coracle_lib::filters::Filter;
use coracle_lib::keys::PublicKey;

use crate::domain::pref::model::PeerPolicy;

/// Kind 0: the author's profile.
pub const KIND_PROFILE: u16 = 0;

/// Kind 5: a request to delete the events its `e` and `a` tags name.
pub const KIND_DELETE: u16 = 5;

/// Kind 10000: NIP-51's mute list.
pub const KIND_MUTE: u16 = 10_000;

/// Whether a tag is one a NIP-01 filter can name, which is the set worth
/// indexing. Everything else is read back from the event's tags.
#[must_use]
pub fn is_indexed_tag(name: &str) -> bool {
    name.len() == 1 && name.chars().all(|c| c.is_ascii_alphanumeric())
}

/// Local criteria the relay protocol cannot express: when an event arrived and
/// who it arrived from.
///
/// These read [provenance](Provenance), which never leaves the device and is
/// never served to a peer, so they are not part of a
/// [`Filter`](coracle_lib::filters::Filter) and never serialized. See
/// `docs/privacy.md`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Seen {
    /// Lower bound on the event's seen time, inclusive.
    pub since: Option<i64>,
    /// Upper bound on the event's seen time, inclusive.
    pub until: Option<i64>,
    /// Restrict to events seen from at least one of these peers.
    pub peers: Option<BTreeSet<PublicKey>>,
}

impl Seen {
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

/// Which authorship register an event sits in, from one device's point of view.
///
/// The two `docs/proofs.md` names, plus everything in neither. What separates
/// them is how far the event can travel, so this is the constraint the sending
/// side puts on a query and the one no policy setting relaxes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Register {
    /// Authored by this device. Its own authenticated session establishes
    /// authorship at the first hop, so nothing else is needed.
    Own,
    /// Authored by someone else, with the author's signature naming this
    /// device — the witness a designated-verifier proof is built from, and so
    /// the second hop.
    Forwardable,
    /// Neither. Held for the user, and goes no further.
    Held,
}

/// A register constraint: which registers qualify, and the device they are
/// measured from.
///
/// The identity is part of the constraint because both registers are relative
/// to it — an event is this device's own, or carries a signature naming this
/// device — and the answer changes entirely under another one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Registers {
    /// The device asking.
    pub identity: PublicKey,
    /// The registers admitted. An empty set admits nothing, as it does on a
    /// filter.
    pub registers: BTreeSet<Register>,
}

impl Registers {
    /// A constraint admitting exactly `registers`.
    #[must_use]
    pub fn new(identity: PublicKey, registers: impl IntoIterator<Item = Register>) -> Self {
        Self {
            identity,
            registers: registers.into_iter().collect(),
        }
    }

    /// Everything this device is in a position to put on the wire at all: its
    /// own events, and those it holds the author's signature over.
    #[must_use]
    pub fn offerable(identity: PublicKey) -> Self {
        Self::new(identity, [Register::Own, Register::Forwardable])
    }

    /// Whether this constraint admits `register`.
    #[must_use]
    pub fn admits(&self, register: Register) -> bool {
        self.registers.contains(&register)
    }

    /// Whether the constraint admits nothing.
    #[must_use]
    pub fn matches_nothing(&self) -> bool {
        self.registers.is_empty()
    }
}

/// What a result set is ordered by.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Order {
    /// The author's claimed time, which is what NIP-01 pagination walks and
    /// what a filter's `limit` is measured against. The default, because a
    /// query with no local criteria is answering the wire.
    #[default]
    CreatedAt,
    /// The local arrival time, which is what a reader wants: a note handed over
    /// today is new to them whatever its author stamped it.
    SeenAt,
}

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

#[cfg(test)]
mod tests {
    use super::*;
    use coracle_lib::prelude::*;
    use coracle_lib::tags::Tags;

    use crate::domain::event::fixtures::{author, event};

    #[test]
    fn addresses_come_from_the_library() {
        let pubkey = author(1).to_hex();

        assert!(
            event(author(1), 1, 100, "", Tags::new())
                .address()
                .is_none()
        );
        assert!(
            event(author(1), 22_242, 100, "", Tags::new())
                .address()
                .is_none()
        );
        assert_eq!(
            event(author(1), 0, 100, "", Tags::new())
                .address()
                .unwrap()
                .to_string(),
            format!("0:{pubkey}:")
        );
        assert_eq!(
            event(author(1), 30_023, 100, "", Tags::new().add("d", ["post"]))
                .address()
                .unwrap()
                .to_string(),
            format!("30023:{pubkey}:post")
        );
        // A d tag on a plain replaceable kind is not part of its address.
        assert_eq!(
            event(
                author(1),
                10_002,
                100,
                "",
                Tags::new().add("d", ["ignored"])
            )
            .address()
            .unwrap()
            .to_string(),
            format!("10002:{pubkey}:")
        );
    }

    #[test]
    fn only_single_letter_tags_are_indexed() {
        assert!(is_indexed_tag("e"));
        assert!(is_indexed_tag("A"));
        assert!(!is_indexed_tag("imeta"));
        assert!(!is_indexed_tag("-"));
        assert!(!is_indexed_tag(""));
    }

    #[test]
    fn unsatisfiable_seen_criteria_are_reported() {
        assert!(!Seen::new().matches_nothing());
        assert!(!Seen::new().add_since(100).add_until(200).matches_nothing());
        assert!(Seen::new().add_since(200).add_until(100).matches_nothing());
        assert!(Seen::new().add_peers([]).matches_nothing());
    }

    #[test]
    fn every_part_of_a_query_answers_for_itself() {
        use crate::domain::pref::model::Policy;

        assert!(!EventFilter::new().matches_nothing());

        // Whichever part of the criteria is unsatisfiable, the whole is.
        assert!(
            EventFilter::new()
                .with_filter(coracle_lib::filters::Filter::new().add_authors([]))
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

    #[test]
    fn the_offerable_registers_are_the_two_that_travel() {
        let registers = Registers::offerable(author(1));

        assert!(registers.admits(Register::Own));
        assert!(registers.admits(Register::Forwardable));
        assert!(!registers.admits(Register::Held));
        assert!(!registers.matches_nothing());
    }
}
