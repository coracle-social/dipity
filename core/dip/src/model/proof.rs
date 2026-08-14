//! The author's signature naming a recipient.

use coracle_lib::keys::PublicKey;

/// A signature by an event's author over `event_id ‖ recipient_pubkey`, held by
/// the peer it names.
///
/// This is not itself an authorship proof. It is the witness one is built from:
/// the recipient proves in zero knowledge that it holds this signature,
/// designated to the peer it is forwarding to. The signature verifies for
/// anyone, so it is portable evidence of who wrote the event, and it is
/// **never served to a peer** under any policy. See `docs/proofs.md`.
///
/// The signature is BIP-340 over a message that is not an event, so it is bare
/// bytes rather than anything in `coracle_lib::events` — the same 64 bytes
/// [`SecretKey::sign`](coracle_lib::keys::SecretKey::sign) returns.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Proof {
    /// The event the signature commits to, as a lowercase hex id.
    pub event_id: String,
    /// The pubkey the signature names. Normally this device's own: a signature
    /// naming anyone else is not one it can build a proof from, since the proof
    /// rests on the recipient being the party named.
    pub recipient_pubkey: PublicKey,
    /// The BIP-340 signature.
    pub sig: [u8; 64],
}
