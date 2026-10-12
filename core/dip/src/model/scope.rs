//! The tiers every policy setting is expressed on.

use serde::{Deserialize, Serialize};

use crate::model::Standing;

/// The tiers every policy setting is expressed on, narrowest first.
///
/// The tiers are ordered. A wider scope admits everyone a narrower one does,
/// and [`admits`](Self::admits) is a comparison rather than a table.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Scope {
    /// Nobody. A visibility rule only, for what is never served.
    Nothing,
    /// People the user paired with, and the value older builds stored as `"trusted"`.
    #[serde(alias = "trusted")]
    Contacts,
    /// Contacts, and the people they have named, two hops out.
    Network,
    /// Anyone somebody has named, by a contact card this device holds. As far
    /// as Accept reaches.
    Named,
    /// Anyone who connects, except refused pubkeys. Sharing and visibility only.
    Lenient,
    /// Anyone. Visibility settings only.
    Public,
}

impl Scope {
    /// Whether a pubkey standing here falls inside this scope.
    #[must_use]
    pub fn admits(self, standing: Standing) -> bool {
        match standing {
            Standing::Refused => false,
            Standing::Contact => self >= Self::Contacts,
            Standing::Network => self >= Self::Network,
            Standing::Named => self >= Self::Named,
            Standing::Stranger => self >= Self::Lenient,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_wider_scope_admits_everyone_a_narrower_one_does() {
        for standing in [
            Standing::Contact,
            Standing::Network,
            Standing::Named,
            Standing::Stranger,
        ] {
            let admitted: Vec<bool> = [
                Scope::Nothing,
                Scope::Contacts,
                Scope::Network,
                Scope::Named,
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

        // Refused is not a tier and is outside even the widest scope.
        assert!(!Scope::Public.admits(Standing::Refused));
        assert!(Scope::Contacts.admits(Standing::Contact));
        assert!(!Scope::Contacts.admits(Standing::Network));
        assert!(Scope::Network.admits(Standing::Network));
        assert!(!Scope::Network.admits(Standing::Stranger));
        assert!(Scope::Named.admits(Standing::Named));
        assert!(!Scope::Named.admits(Standing::Stranger));
        assert!(Scope::Lenient.admits(Standing::Stranger));
        assert!(!Scope::Nothing.admits(Standing::Contact));
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
        assert_eq!(
            serde_json::from_str::<Scope>(r#""trusted""#).unwrap(),
            Scope::Contacts
        );
        assert!(serde_json::from_str::<Scope>(r#""whatever""#).is_err());
    }
}
