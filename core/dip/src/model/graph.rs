//! The user's contact graph, and where a pubkey stands in it.

use std::collections::BTreeSet;

use coracle_lib::keys::PublicKey;

/// Where a pubkey stands in the user's graph.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Standing {
    /// Blocked. Dropped on ingest, never served, sessions refused.
    Blocked,
    /// Somebody the user paired with and named.
    Contact,
    /// Somebody a contact named, two hops out.
    Network,
    /// Everyone else.
    Stranger,
}

impl Standing {
    /// Where a device stands, given that it proved two identities.
    ///
    /// Blocked is a veto and the rest take the more privileged of the two. The
    /// asymmetry is the point: blocking is a decision about a person, and a
    /// device holding a blocked key is that person's device whatever else it
    /// also signs with. Access runs the other way — proving an extra key is a
    /// claim to more, never less. The union is simply the best of them, because
    /// the peer could have made the better claim alone.
    #[must_use]
    pub fn combine(self, other: Self) -> Self {
        match (self, other) {
            (Self::Blocked, _) | (_, Self::Blocked) => Self::Blocked,
            (Self::Contact, _) | (_, Self::Contact) => Self::Contact,
            (Self::Network, _) | (_, Self::Network) => Self::Network,
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
    /// People the user has blocked.
    pub blocked: BTreeSet<PublicKey>,
    /// People the user has muted.
    pub muted: BTreeSet<PublicKey>,
    /// Topics the user has muted, as the mute list's `t` tags name them.
    pub muted_topics: BTreeSet<String>,
}

impl Graph {
    /// Where `pubkey` stands.
    #[must_use]
    pub fn standing(&self, pubkey: &PublicKey) -> Standing {
        if self.blocked.contains(pubkey) {
            Standing::Blocked
        } else if self.contacts.contains(pubkey) {
            Standing::Contact
        } else if self.network.contains(pubkey) {
            Standing::Network
        } else {
            Standing::Stranger
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    use crate::fixtures::author;

    /// A graph with one pubkey in each tier: 2 a contact, 3 network, 4 blocked,
    /// 5 muted. Shared with the tests of everything measured against a graph.
    pub(crate) fn graph() -> Graph {
        let mut graph = Graph::default();

        graph.contacts.insert(author(2));
        graph.network.insert(author(3));
        graph.blocked.insert(author(4));
        graph.muted.insert(author(5));

        graph
    }

    #[test]
    fn block_wins_over_naming() {
        let mut graph = Graph::default();

        graph.contacts.insert(author(2));
        graph.blocked.insert(author(2));

        assert_eq!(graph.standing(&author(2)), Standing::Blocked);
    }

    #[test]
    fn standing_reads_the_graph_in_tiers() {
        let graph = graph();

        assert_eq!(graph.standing(&author(2)), Standing::Contact);
        assert_eq!(graph.standing(&author(3)), Standing::Network);
        assert_eq!(graph.standing(&author(4)), Standing::Blocked);
        assert_eq!(graph.standing(&author(9)), Standing::Stranger);

        // Muting is not a tier: a muted author still gossips normally.
        assert_eq!(graph.standing(&author(5)), Standing::Stranger);
        assert!(graph.muted.contains(&author(5)));
        assert!(!graph.muted.contains(&author(2)));
    }
}
