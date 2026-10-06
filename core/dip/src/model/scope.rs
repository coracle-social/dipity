//! The tiers every policy setting is expressed on.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use crate::model::{Authors, Graph, Standing};

/// The tiers every policy setting is expressed on, narrowest first.
///
/// Ordered, so a wider scope admits everyone a narrower one does and
/// [`admits`](Self::admits) is a comparison rather than a table.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Scope {
    /// Nobody. A visibility rule only, for what is never served.
    Nothing,
    /// People the user explicitly trusts.
    Trusted,
    /// People the user paired with, and the people they trust.
    Contacts,
    /// People the user transitively trusts, two hops out, and their contacts.
    Network,
    /// Anyone who connects, except blocked pubkeys.
    Lenient,
    /// Anyone. Visibility settings only.
    Public,
}

impl Scope {
    /// Whether a pubkey standing here falls inside this scope.
    #[must_use]
    pub fn admits(self, standing: Standing) -> bool {
        match standing {
            Standing::Blocked => false,
            Standing::Trusted => self >= Self::Trusted,
            Standing::Contact => self >= Self::Contacts,
            Standing::Network => self >= Self::Network,
            Standing::Stranger => self >= Self::Lenient,
        }
    }

    /// Which authors this scope admits, resolved against `graph`.
    #[must_use]
    pub fn authors(self, graph: &Graph) -> Authors {
        match self {
            Self::Nothing => Authors::Only(BTreeSet::new()),
            Self::Trusted => Authors::Only(graph.trusted.clone()),
            Self::Contacts => Authors::Only(
                graph
                    .trusted
                    .union(&graph.contacts)
                    .copied()
                    .collect::<BTreeSet<_>>(),
            ),
            Self::Network => Authors::Only(
                graph
                    .trusted
                    .iter()
                    .chain(&graph.contacts)
                    .chain(&graph.network)
                    .copied()
                    .collect::<BTreeSet<_>>(),
            ),
            Self::Lenient => Authors::Except(graph.blocked.clone()),
            Self::Public => Authors::Any,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::fixtures::author;
    use crate::model::graph::tests::graph;

    #[test]
    fn a_wider_scope_admits_everyone_a_narrower_one_does() {
        for standing in [
            Standing::Trusted,
            Standing::Contact,
            Standing::Network,
            Standing::Stranger,
        ] {
            let admitted: Vec<bool> = [
                Scope::Nothing,
                Scope::Trusted,
                Scope::Contacts,
                Scope::Network,
                Scope::Lenient,
                Scope::Public,
            ]
            .into_iter()
            .map(|scope| scope.admits(standing))
            .collect();

            assert!(
                admitted.windows(2).all(|pair| pair[0] <= pair[1]),
                "{standing:?} fell out of a scope it was in: {admitted:?}"
            );
        }

        // Block is not a tier, so it is outside even the widest scope.
        assert!(!Scope::Public.admits(Standing::Blocked));
        assert!(Scope::Trusted.admits(Standing::Trusted));
        assert!(!Scope::Trusted.admits(Standing::Contact));
        assert!(Scope::Contacts.admits(Standing::Contact));
        assert!(!Scope::Contacts.admits(Standing::Network));
        assert!(!Scope::Trusted.admits(Standing::Network));
        assert!(Scope::Network.admits(Standing::Network));
        assert!(!Scope::Network.admits(Standing::Stranger));
        assert!(Scope::Lenient.admits(Standing::Stranger));
        assert!(!Scope::Nothing.admits(Standing::Trusted));
    }

    #[test]
    fn a_scope_is_stored_as_the_word_the_document_uses() {
        assert_eq!(
            serde_json::to_string(&Scope::Lenient).unwrap(),
            r#""lenient""#
        );
        assert_eq!(
            serde_json::from_str::<Scope>(r#""network""#).unwrap(),
            Scope::Network
        );
        assert!(serde_json::from_str::<Scope>(r#""whatever""#).is_err());
    }

    #[test]
    fn a_scope_resolves_to_the_authors_it_admits() {
        let graph = graph();

        assert_eq!(
            Scope::Nothing.authors(&graph),
            Authors::Only(BTreeSet::new())
        );
        assert_eq!(
            Scope::Trusted.authors(&graph),
            Authors::Only([author(2)].into())
        );
        assert_eq!(
            Scope::Contacts.authors(&graph),
            Authors::Only([author(2), author(6)].into())
        );
        assert_eq!(
            Scope::Network.authors(&graph),
            Authors::Only([author(2), author(3), author(6)].into())
        );
        assert_eq!(
            Scope::Lenient.authors(&graph),
            Authors::Except([author(4)].into())
        );
        assert_eq!(Scope::Public.authors(&graph), Authors::Any);
    }
}
