//! Recognizing a paired peer without either side naming anything durable.
//!
//! At pairing both sides derive a pair secret from the authenticated session
//! and store it against the peer. On a later encounter each proves it holds one
//! by MACing this session's handshake hash under it, and the receiver
//! trial-MACs its own secrets against the list. The handshake hash is fresh
//! every time, so a tag is worthless outside the session it was minted in.
//! `docs/discovery.md#recognition`.

use coracle_lib::keys::PublicKey;

/// How many tags go on the wire, padded with random bytes to hide the count.
pub const TAG_COUNT: usize = 32;

/// One proof of holding a pair secret: `HMAC(pair_secret, handshake_hash)`.
pub type Tag = [u8; 32];

/// The tags a device emits, padded to [`TAG_COUNT`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tags(pub Vec<Tag>);

/// The tag proving possession of `pair_secret` in the session `handshake_hash`
/// identifies.
#[must_use]
pub fn tag(_pair_secret: &[u8; 32], _handshake_hash: &[u8; 32]) -> Tag {
    todo!("HMAC-SHA256")
}

/// Which peer, if any, a list of tags resolves to.
///
/// Trial-MACs every pair secret this device holds against `handshake_hash` and
/// looks each result up in `offered`, one MAC per stored secret.
#[must_use]
pub fn resolve(
    _offered: &Tags,
    _secrets: &[(PublicKey, [u8; 32])],
    _handshake_hash: &[u8; 32],
) -> Option<PublicKey> {
    todo!("trial-MAC against stored pair secrets")
}
