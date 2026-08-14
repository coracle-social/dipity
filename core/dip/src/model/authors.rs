//! The authors a resolved scope admits.

use std::collections::BTreeSet;

use coracle_lib::keys::PublicKey;

/// The authors admitted for a resolved standing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Authors {
    /// Every author.
    Any,
    /// Only these. An empty set admits nobody.
    Only(BTreeSet<PublicKey>),
    /// Everyone but these. An empty set excludes nobody.
    Except(BTreeSet<PublicKey>),
}
