//! The author's signature naming a recipient.

use std::fmt;

use coracle_lib::events::EventId;
use coracle_lib::keys::PublicKey;

use crate::util::Bare;

/// A signature by an event's author over `event_id ‖ recipient_pubkey`, held by
/// the peer it names. It is the witness an authorship proof is built from,
/// not the proof. The recipient proves in zero knowledge that it holds this
/// signature, designated to the peer it is forwarding to.
#[derive(Clone, PartialEq, Eq)]
pub struct RecipientSignature {
    /// The event the signature commits to.
    pub event_id: EventId,
    /// The author whose signature this is, and the event's own pubkey.
    pub author_pubkey: PublicKey,
    /// The pubkey the signature names.
    pub recipient_pubkey: PublicKey,
    /// The BIP-340 signature.
    pub sig: [u8; 64],
}

impl fmt::Debug for RecipientSignature {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Enough of the id to correlate a line with an event, and no more.
        let event_id = format!("{}…", &self.event_id.to_hex()[..8]);

        // Hex rather than `PublicKey`'s derive, which prints the secp256k1 internals.
        let author = self.author_pubkey.to_hex();
        let recipient = self.recipient_pubkey.to_hex();

        f.debug_struct("RecipientSignature")
            .field("event_id", &Bare(&event_id))
            .field("author_pubkey", &Bare(&author))
            .field("recipient_pubkey", &Bare(&recipient))
            .field("sig", &Bare("<redacted>"))
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::fixtures::{author, secret};

    fn signature() -> RecipientSignature {
        RecipientSignature {
            event_id: EventId::new([0xee; 32]),
            author_pubkey: author(1),
            recipient_pubkey: author(2),
            sig: [7u8; 64],
        }
    }

    #[test]
    fn debug_redacts_the_signature() {
        let printed = format!("{:?}", signature());

        assert!(!printed.contains(&hex::encode(signature().sig)));
        assert!(printed.contains("<redacted>"));
    }

    #[test]
    fn debug_abbreviates_the_event_id() {
        let printed = format!("{:?}", signature());

        assert!(printed.contains("eeeeeeee…"));
        assert!(!printed.contains(&"ee".repeat(32)));
    }

    #[test]
    fn debug_keeps_the_parties() {
        // Which peer a signature names is the whole reason to print one.
        let printed = format!("{:?}", signature());

        assert!(printed.contains(&author(1).to_hex()));
        assert!(printed.contains(&author(2).to_hex()));
    }

    // No test for a short or non-hex event id: `EventId` is 64 hex characters or nothing.

    #[test]
    fn a_secret_key_is_redacted_too() {
        // The precedent this follows, asserted so the two stay in step.
        assert!(!format!("{:?}", secret(1)).contains(&secret(1).to_hex()));
    }
}
