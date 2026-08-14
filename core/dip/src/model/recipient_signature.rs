//! The author's signature naming a recipient.

use std::fmt;

use coracle_lib::keys::PublicKey;

use crate::util::Bare;

/// A signature by an event's author over `event_id ‖ recipient_pubkey`, held by
/// the peer it names. It is the witness an authorship proof is built from,
/// not the proof. The recipient proves in zero knowledge that it holds this
/// signature, designated to the peer it is forwarding to.
///
/// The signature is BIP-340 over a message that is not an event, so it is bare
/// bytes rather than anything in `coracle_lib::events`.
///
/// Everything needed to check it is here, author included, because a signature
/// without the key it is by is not a claim about anything — it neither verifies
/// nor proves. The store keeps the author against the event's own by foreign
/// key, so the two cannot disagree.
#[derive(Clone, PartialEq, Eq)]
pub struct RecipientSignature {
    /// The event the signature commits to, as a lowercase hex id.
    pub event_id: String,
    /// The author whose signature this is, and the event's own pubkey.
    pub author_pubkey: PublicKey,
    /// The pubkey the signature names.
    pub recipient_pubkey: PublicKey,
    /// The BIP-340 signature.
    pub sig: [u8; 64],
}

impl fmt::Debug for RecipientSignature {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Enough of the id to correlate a line with an event, and not the whole
        // of it: what is being debugged here is which signature, not which
        // event, and the rest is a record of who this device has been near.
        let event_id = match self.event_id.get(..8) {
            Some(prefix) => format!("{prefix}…"),
            None => self.event_id.clone(),
        };

        // Hex, rather than `PublicKey`'s own derive, which prints the internal
        // secp256k1 representation and is unreadable next to anything else.
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
            event_id: "e".repeat(64),
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
        assert!(!printed.contains(&"e".repeat(64)));
    }

    #[test]
    fn debug_keeps_the_parties() {
        // Both are public keys, and which peer a signature names is the whole
        // reason to print one.
        let printed = format!("{:?}", signature());

        assert!(printed.contains(&author(1).to_hex()));
        assert!(printed.contains(&author(2).to_hex()));
    }

    #[test]
    fn a_short_event_id_is_printed_whole() {
        // Not a real id, so there is nothing to abbreviate and nothing that
        // would panic on a slice that is not a character boundary.
        let mut signature = signature();
        signature.event_id = "hé".to_string();

        assert!(format!("{signature:?}").contains("hé"));
    }

    #[test]
    fn a_secret_key_is_redacted_too() {
        // The precedent this follows, asserted so the two stay in step.
        assert!(!format!("{:?}", secret(1)).contains(&secret(1).to_hex()));
    }
}
