//! Recognizing a paired peer without either side naming anything durable.
//!
//! At pairing both sides derive a pair secret from the authenticated session
//! and store it against the peer. On a later encounter each proves it holds one
//! by MACing this session's handshake hash under it, and the receiver
//! trial-MACs its own secrets against the list. The handshake hash is fresh
//! every time, so a tag is worthless outside the session it was minted in.
//! `docs/discovery.md#recognition`.

use anyhow::{Context, Result, bail};
use coracle_lib::keys::PublicKey;
use hmac::{Hmac, Mac};
use sha2::{Digest, Sha256};

/// How many tags go on the wire, whatever the device holds: padded up with
/// random bytes, sampled down when it holds more pair secrets than this.
pub const TAG_COUNT: usize = 32;

/// What one tag occupies on the wire.
const TAG_BYTES: usize = 32;

/// The MAC over a [`Tag`]'s inputs.
type HmacSha256 = Hmac<Sha256>;

/// One proof of holding a pair secret: `HMAC(pair_secret, handshake_hash)`.
pub type Tag = [u8; 32];

/// The tags a device emits, always exactly [`TAG_COUNT`] of them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tags(pub Vec<Tag>);

impl Tags {
    /// The list as it goes on the wire: the raw tags, concatenated.
    ///
    /// Bytes rather than JSON, because the MTU is the binding constraint on
    /// this link and a JSON array of decimal integers costs three times what
    /// the tags themselves do. `docs/transport.md`.
    #[must_use]
    pub fn encode(&self) -> Vec<u8> {
        self.0.concat()
    }

    /// Read a peer's list, which carries exactly [`TAG_COUNT`] tags.
    ///
    /// Any other length is either malformed or an attempt to make this device
    /// trial-MAC against an unbounded list; the fixed count is also what keeps
    /// the length from disclosing how many peers the sender has paired with.
    pub fn decode(payload: &[u8]) -> Result<Self> {
        if payload.len() != TAG_COUNT * TAG_BYTES {
            bail!(
                "a recognition list carried {} bytes rather than {}",
                payload.len(),
                TAG_COUNT * TAG_BYTES
            );
        }

        Ok(Self(
            payload
                .chunks_exact(TAG_BYTES)
                .map(|chunk| Tag::try_from(chunk).expect("a chunk of exactly TAG_BYTES"))
                .collect(),
        ))
    }
}

/// Derive the pair secret both sides of a completed session agree on, from the
/// handshake hash they both hold. Domain-separated so it is not a value any
/// other use of the hash could collide with.
#[must_use]
pub fn derive_secret(handshake_hash: &[u8; 32]) -> [u8; 32] {
    let mut hasher = Sha256::new();

    hasher.update(b"dip/pair-secret");
    hasher.update(handshake_hash);

    hasher.finalize().into()
}

/// The tag proving possession of `pair_secret` in the session `handshake_hash`
/// identifies.
#[must_use]
pub fn tag(pair_secret: &[u8; 32], handshake_hash: &[u8; 32]) -> Tag {
    let mut mac = HmacSha256::new_from_slice(pair_secret).expect("an array key fits HMAC");

    mac.update(handshake_hash);

    mac.finalize().into_bytes().into()
}

/// The list to put on the wire for `secrets` in the session `handshake_hash`
/// identifies: [`TAG_COUNT`] tags, however many pair secrets the device holds.
///
/// Beyond that count the tags are a random sample, drawn again every session.
/// A peer left out of one list is not recognized on that encounter — it arrives
/// as a stranger and the consent gate judges it — but it is in the running for
/// the next one, which a fixed truncation would not leave it. The alternative,
/// sending everything, puts the pairing count on the wire exactly and grows the
/// frame without bound.
pub fn select(secrets: &[(PublicKey, [u8; 32])], handshake_hash: &[u8; 32]) -> Result<Tags> {
    let mut tags: Vec<Tag> = secrets
        .iter()
        .map(|(_, secret)| tag(secret, handshake_hash))
        .collect();

    if tags.len() > TAG_COUNT {
        shuffle(&mut tags)?;
        tags.truncate(TAG_COUNT);
    }

    while tags.len() < TAG_COUNT {
        let mut padding = [0u8; TAG_BYTES];
        getrandom::getrandom(&mut padding).context("padding the recognition list")?;

        tags.push(padding);
    }

    // Shuffled so a peer finding its own tag learns nothing from where it sat.
    shuffle(&mut tags)?;

    Ok(Tags(tags))
}

/// Which peers a list of tags resolves to.
///
/// Trial-MACs every pair secret this device holds against `handshake_hash` and
/// looks each result up in `offered`, one MAC per stored secret. Every match is
/// returned: pairing stores one secret against each pubkey the peer proved, so
/// a device that authenticated as several resolves to all of them and the gate
/// judges the whole set rather than whichever one happened to come first.
#[must_use]
pub fn resolve(
    offered: &Tags,
    secrets: &[(PublicKey, [u8; 32])],
    handshake_hash: &[u8; 32],
) -> Vec<PublicKey> {
    secrets
        .iter()
        .filter(|(_, secret)| offered.0.contains(&tag(secret, handshake_hash)))
        .map(|(pubkey, _)| *pubkey)
        .collect()
}

/// Fisher-Yates over `items`, drawing from the system CSPRNG.
///
/// The modulo bias over a 64-bit draw is far below anything that could tell the
/// result from uniform at this length.
fn shuffle<T>(items: &mut [T]) -> Result<()> {
    for index in (1..items.len()).rev() {
        let mut bytes = [0u8; 8];
        getrandom::getrandom(&mut bytes).context("shuffling the recognition list")?;

        let pick = u64::from_le_bytes(bytes) % (index as u64 + 1);

        items.swap(index, pick as usize);
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::fixtures::author;

    fn secret(n: u8) -> [u8; 32] {
        [n; 32]
    }

    /// `count` pair secrets, each against its own pubkey.
    fn secrets(count: u8) -> Vec<(PublicKey, [u8; 32])> {
        (1..=count)
            .map(|seed| (author(seed), secret(seed)))
            .collect()
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

        assert_eq!(resolve(&their_tags, &secrets, &[9; 32]), vec![pubkey]);
    }

    #[test]
    fn every_pubkey_a_shared_secret_is_stored_against_resolves() {
        // What pairing writes for a peer that authenticated as two identities: one secret, twice.
        let shared = secret(3);
        let secrets = [(author(2), shared), (author(3), shared)];
        let their_tags = Tags(vec![tag(&shared, &[9; 32])]);

        assert_eq!(
            resolve(&their_tags, &secrets, &[9; 32]),
            vec![author(2), author(3)]
        );
    }

    #[test]
    fn no_match_resolves_to_nobody() {
        let pubkey = author(2);
        let secrets = [(pubkey, secret(3))];

        assert!(resolve(&Tags(vec![]), &secrets, &[9; 32]).is_empty());
    }

    #[test]
    fn a_list_is_the_same_length_whatever_the_device_holds() {
        let empty = select(&[], &[9; 32]).unwrap();
        let few = select(&secrets(3), &[9; 32]).unwrap();
        let many = select(&secrets(200), &[9; 32]).unwrap();

        assert_eq!(empty.0.len(), TAG_COUNT);
        assert_eq!(few.0.len(), TAG_COUNT);
        assert_eq!(many.0.len(), TAG_COUNT);
        assert_eq!(many.encode().len(), TAG_COUNT * TAG_BYTES);
    }

    #[test]
    fn a_device_past_the_count_sends_a_sample_of_what_it_holds() {
        let held = secrets(200);
        let real: Vec<Tag> = held
            .iter()
            .map(|(_, secret)| tag(secret, &[9; 32]))
            .collect();

        let sent = select(&held, &[9; 32]).unwrap();

        // Every tag is one it holds — no room for padding — and the sample differs each session.
        assert!(sent.0.iter().all(|tag| real.contains(tag)));
        assert_ne!(sent, select(&held, &[9; 32]).unwrap());
    }

    #[test]
    fn a_list_round_trips_through_the_wire_encoding() {
        let sent = select(&secrets(3), &[9; 32]).unwrap();

        assert_eq!(Tags::decode(&sent.encode()).unwrap(), sent);
    }

    #[test]
    fn a_list_of_any_other_length_is_refused() {
        let sent = select(&secrets(3), &[9; 32]).unwrap();
        let mut short = sent.encode();
        short.truncate(TAG_BYTES);

        assert!(Tags::decode(&short).is_err());
        assert!(Tags::decode(&[]).is_err());

        // A long list is what would make resolution unbounded work.
        let mut long = sent.encode();
        long.extend_from_slice(&[0u8; TAG_BYTES]);
        assert!(Tags::decode(&long).is_err());
    }
}
