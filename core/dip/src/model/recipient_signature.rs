//! The author's signature naming a recipient.

use coracle_lib::keys::PublicKey;

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
#[derive(Debug, Clone, PartialEq, Eq)]
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
