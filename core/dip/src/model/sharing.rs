//! How far the user's own activity travels. `docs/policy.md#sharing`.

use serde::{Deserialize, Serialize};

use crate::model::Scope;

/// Who is handed the user's own events, and who may carry them a hop further.
///
/// Carrying an event on needs the author's signature over it, which is
/// permanent evidence of authorship to anyone it reaches. So a signature only
/// ever goes to somebody the user paired with, and the widest setting widens
/// who sees an event first-hand rather than who can attribute it.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Sharing {
    /// Contacts are handed the user's events and nobody may carry them further.
    Contacts,
    /// Contacts are handed the user's events with a signature, so each may
    /// carry them on to their own contacts.
    Network,
    /// Anyone not blocked is handed the user's events, and contacts get a
    /// signature as under [`Network`](Self::Network).
    #[default]
    Anyone,
}

impl Sharing {
    /// Who is handed the user's own events, before the rules for their lists narrow it.
    #[must_use]
    pub fn audience(self) -> Scope {
        match self {
            Self::Contacts | Self::Network => Scope::Contacts,
            Self::Anyone => Scope::Lenient,
        }
    }

    /// Whether a contact handed an own event also gets the signature over it.
    #[must_use]
    pub fn signs(self) -> bool {
        self != Self::Contacts
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::model::Standing;

    #[test]
    fn only_anyone_reaches_a_stranger_first_hand() {
        assert!(!Sharing::Contacts.audience().admits(Standing::Stranger));
        assert!(!Sharing::Network.audience().admits(Standing::Network));
        assert!(Sharing::Anyone.audience().admits(Standing::Stranger));
        assert!(Sharing::Contacts.audience().admits(Standing::Contact));
    }

    #[test]
    fn a_setting_is_stored_as_the_word_the_document_uses() {
        assert_eq!(
            serde_json::to_string(&Sharing::Network).unwrap(),
            r#""network""#
        );
        assert_eq!(
            serde_json::from_str::<Sharing>(r#""anyone""#).unwrap(),
            Sharing::Anyone
        );
    }
}
