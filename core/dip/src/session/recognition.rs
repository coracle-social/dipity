//! Recognizing a paired peer without either side naming anything durable.
//!
//! At pairing both sides derive a pair secret from the authenticated session
//! and store it against the peer. On a later encounter each proves it holds one
//! by MACing this session's handshake hash under it, and the receiver
//! trial-MACs its own secrets against the list. The handshake hash is fresh
//! every time, so a tag is worthless outside the session it was minted in.
//! `docs/discovery.md#recognition`.

use coracle_lib::keys::PublicKey;
use hmac::{Hmac, Mac};
use sha2::Sha256;

/// How many tags go on the wire, padded with random bytes to hide the count.
pub const TAG_COUNT: usize = 32;

/// The MAC over a [`Tag`]'s inputs.
type HmacSha256 = Hmac<Sha256>;

/// One proof of holding a pair secret: `HMAC(pair_secret, handshake_hash)`.
pub type Tag = [u8; 32];

/// The tags a device emits, padded to [`TAG_COUNT`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tags(pub Vec<Tag>);

/// The tag proving possession of `pair_secret` in the session `handshake_hash`
/// identifies.
#[must_use]
pub fn tag(pair_secret: &[u8; 32], handshake_hash: &[u8; 32]) -> Tag {
    let mut mac = HmacSha256::new_from_slice(pair_secret).expect("an array key fits HMAC");

    mac.update(handshake_hash);

    mac.finalize().into_bytes().into()
}

/// Which peer, if any, a list of tags resolves to.
///
/// Trial-MACs every pair secret this device holds against `handshake_hash` and
/// looks each result up in `offered`, one MAC per stored secret.
#[must_use]
pub fn resolve(
    offered: &Tags,
    secrets: &[(PublicKey, [u8; 32])],
    handshake_hash: &[u8; 32],
) -> Option<PublicKey> {
    secrets.iter().find_map(|(pubkey, secret)| {
        offered
            .0
            .contains(&tag(secret, handshake_hash))
            .then_some(*pubkey)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::fixtures::author;

    fn secret(n: u8) -> [u8; 32] {
        [n; 32]
    }

    #[test]
    fn a_tag_is_the_hmac_of_the_hash_under_the_secret() {
        let first = tag(&secret(1), &[7; 32]);
        let repeated = tag(&secret(1), &[7; 32]);

        assert_eq!(first, repeated);
        assert_ne!(
            tag(&secret(2), &[7; 32]),
            first,
            "a different secret differs"
        );
        assert_ne!(tag(&secret(1), &[8; 32]), first, "a different hash differs");
    }

    #[test]
    fn a_matching_tag_resolves_to_the_devices_pubkey() {
        let pubkey = author(2);
        let secrets = [(pubkey, secret(3))];
        let their_tags = Tags(vec![tag(&secret(3), &[9; 32])]);

        assert_eq!(resolve(&their_tags, &secrets, &[9; 32]), Some(pubkey));
    }

    #[test]
    fn no_match_resolves_to_nobody() {
        let pubkey = author(2);
        let secrets = [(pubkey, secret(3))];

        assert_eq!(resolve(&Tags(vec![]), &secrets, &[9; 32]), None);
    }
}
