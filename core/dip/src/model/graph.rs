//! The user's trust graph, and where a pubkey stands in it.

use std::collections::BTreeSet;

use coracle_lib::keys::PublicKey;

/// Where a pubkey stands in the user's graph.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Standing {
    /// Blocked. Dropped on ingest, never served, sessions refused.
    Blocked,
    /// Explicitly trusted.
    Trusted,
    /// Transitively trusted, two hops out.
    Network,
    /// Everyone else.
    Stranger,
}

/// The user's trust graph: the tiers, as sets of pubkeys.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Graph {
    /// People the user explicitly trusts.
    pub trusted: BTreeSet<PublicKey>,
    /// People a trusted person trusts, which is as far as the graph reaches.
    pub network: BTreeSet<PublicKey>,
    /// People the user has blocked.
    pub blocked: BTreeSet<PublicKey>,
    /// People the user has muted.
    pub muted: BTreeSet<PublicKey>,
}

impl Graph {
    /// Where `pubkey` stands.
    #[must_use]
    pub fn standing(&self, pubkey: &PublicKey) -> Standing {
        if self.blocked.contains(pubkey) {
            Standing::Blocked
        } else if self.trusted.contains(pubkey) {
            Standing::Trusted
        } else if self.network.contains(pubkey) {
            Standing::Network
        } else {
            Standing::Stranger
        }
    }

    /// Whether the user has muted `pubkey`.
    #[must_use]
    pub fn is_muted(&self, pubkey: &PublicKey) -> bool {
        self.muted.contains(pubkey)
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    use crate::fixtures::author;

    /// A graph with one pubkey in each tier: 2 trusted, 3 network, 4 blocked,
    /// 5 muted. Shared with the tests of everything measured against a graph.
    pub(crate) fn graph() -> Graph {
        let mut graph = Graph::default();

        graph.trusted.insert(author(2));
        graph.network.insert(author(3));
        graph.blocked.insert(author(4));
        graph.muted.insert(author(5));

        graph
    }

    #[test]
    fn block_wins_over_trust() {
        let mut graph = Graph::default();

        graph.trusted.insert(author(2));
        graph.blocked.insert(author(2));

        assert_eq!(graph.standing(&author(2)), Standing::Blocked);
    }

    #[test]
    fn standing_reads_the_graph_in_tiers() {
        let graph = graph();

        assert_eq!(graph.standing(&author(2)), Standing::Trusted);
        assert_eq!(graph.standing(&author(3)), Standing::Network);
        assert_eq!(graph.standing(&author(4)), Standing::Blocked);
        assert_eq!(graph.standing(&author(9)), Standing::Stranger);

        // Muting is not a tier: a muted author still gossips normally.
        assert_eq!(graph.standing(&author(5)), Standing::Stranger);
        assert!(graph.is_muted(&author(5)));
        assert!(!graph.is_muted(&author(2)));
    }
}
