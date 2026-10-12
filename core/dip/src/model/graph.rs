//! The user's contact graph, and where a pubkey stands in it.

use std::collections::BTreeSet;

use coracle_lib::keys::PublicKey;

/// Where a pubkey stands in the user's graph.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Standing {
    /// Muted and withheld both, which is what refusing somebody is: their
    /// events are dropped on ingest and never served, and their sessions are refused.
    Refused,
    /// Somebody the user paired with and named.
    Contact,
    /// Somebody a contact named, two hops out.
    Network,
    /// Somebody anybody named, by a contact card this device holds.
    Named,
    /// Everyone else.
    Stranger,
}

impl Standing {
    /// Where a device stands, given that it proved two identities.
    ///
    /// Refused is a veto and the rest take the more privileged of the two. The
    /// asymmetry is the point: refusing is a decision about a person, and a
    /// device holding a refused key is that person's device whatever else it
    /// also signs with. Access runs the other way — proving an extra key is a
    /// claim to more, never less. The union is simply the best of them, because
    /// the peer could have made the better claim alone.
    #[must_use]
    pub fn combine(self, other: Self) -> Self {
        match (self, other) {
            (Self::Refused, _) | (_, Self::Refused) => Self::Refused,
            (Self::Contact, _) | (_, Self::Contact) => Self::Contact,
            (Self::Network, _) | (_, Self::Network) => Self::Network,
            (Self::Named, _) | (_, Self::Named) => Self::Named,
            _ => Self::Stranger,
        }
    }
}

/// A topic as a `t` tag names it, without the `#` a writer may have left on
/// it, which is how the view reads one too.
#[must_use]
pub fn topic(value: &str) -> &str {
    value.strip_prefix('#').unwrap_or(value)
}

/// The user's contact graph: the tiers, as sets of pubkeys.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Graph {
    /// People the user paired with, which is everybody they have named.
    pub contacts: BTreeSet<PublicKey>,
    /// People a contact has named, which is as far as the graph reaches.
    pub network: BTreeSet<PublicKey>,
    /// People anybody not refused has named, by a card this device holds.
    pub named: BTreeSet<PublicKey>,
    /// People whose events the user never passes on, and whom the user's cards vouch for to nobody.
    pub withheld: BTreeSet<PublicKey>,
    /// People the user has muted.
    pub muted: BTreeSet<PublicKey>,
    /// Topics the user has muted, as the mute list's `t` tags name them.
    pub muted_topics: BTreeSet<String>,
}

impl Graph {
    /// Whether the user has both muted and withheld `pubkey`, which leaves this
    /// device nothing to do with them. `docs/policy.md#social-graph`.
    #[must_use]
    pub fn refuses(&self, pubkey: &PublicKey) -> bool {
        self.muted.contains(pubkey) && self.withheld.contains(pubkey)
    }

    /// Mute and withhold `pubkey` at once.
    #[cfg(test)]
    pub(crate) fn refuse(&mut self, pubkey: PublicKey) {
        self.muted.insert(pubkey);
        self.withheld.insert(pubkey);
    }

    /// Where `pubkey` stands.
    #[must_use]
    pub fn standing(&self, pubkey: &PublicKey) -> Standing {
        if self.refuses(pubkey) {
            Standing::Refused
        } else if self.contacts.contains(pubkey) {
            Standing::Contact
        } else if self.network.contains(pubkey) {
            Standing::Network
        } else if self.named.contains(pubkey) {
            Standing::Named
        } else {
            Standing::Stranger
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    use crate::fixtures::author;

    /// A graph with one pubkey in each tier: 2 a contact, 3 network, 4 refused,
    /// 5 muted. Shared with the tests of everything measured against a graph.
    pub(crate) fn graph() -> Graph {
        let mut graph = Graph::default();

        graph.contacts.insert(author(2));
        graph.network.insert(author(3));
        graph.refuse(author(4));
        graph.muted.insert(author(5));

        graph
    }

    #[test]
    fn refusing_wins_over_naming() {
        let mut graph = Graph::default();

        graph.contacts.insert(author(2));
        graph.refuse(author(2));

        assert_eq!(graph.standing(&author(2)), Standing::Refused);
    }

    #[test]
    fn standing_reads_the_graph_in_tiers() {
        let mut graph = graph();

        graph.withheld.insert(author(6));

        assert_eq!(graph.standing(&author(2)), Standing::Contact);
        assert_eq!(graph.standing(&author(3)), Standing::Network);
        assert_eq!(graph.standing(&author(4)), Standing::Refused);
        assert_eq!(graph.standing(&author(9)), Standing::Stranger);

        // Muting or withholding alone is not a tier: it takes both to refuse somebody.
        assert_eq!(graph.standing(&author(5)), Standing::Stranger);
        assert_eq!(graph.standing(&author(6)), Standing::Stranger);
        assert!(graph.muted.contains(&author(5)));
        assert!(!graph.muted.contains(&author(2)));
    }
}
