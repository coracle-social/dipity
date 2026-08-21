//! What a peer has proved about itself.

use std::collections::BTreeSet;

use coracle_lib::events::{HasCreatedAt, HasId, HasKind, HasPubkey, HasTags};
use coracle_lib::keys::PublicKey;

use crate::link::LinkId;
use crate::model::{PeerPolicy, Policy, Standing};

/// A peer — the device on the other end of the session — that has completed
/// NIP-42, and what the user's settings say about it.
///
/// A device may hold several identities and prove them all over one channel,
/// so what it proved is a set. What the user's settings say about it is not:
/// the trust graph names people, and every question the sync layer asks — what
/// may be served, what may be stored, how much it may write — is a question
/// about the device. The set is therefore reduced once, at binding, to the one
/// [`Standing`] everything downstream reads.
///
/// The pubkeys themselves are kept because the cryptography still names them:
/// a recipient signature names a recipient and an authorship proof designates
/// a verifier, and neither folds into a standing.
///
/// Built by [`Session`](super::Session) when identification completes and held
/// there for the life of the session. A function that takes one cannot be
/// called before the peer is identified.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Peer {
    /// The link it was proved over. A `Peer` does not outlive its session.
    pub link: LinkId,
    /// Every pubkey the peer proved.
    pubkeys: BTreeSet<PublicKey>,
    /// The user's policy, resolved against the whole set.
    policy: PeerPolicy,
}

impl Peer {
    /// Bind `policy` to the device that proved `pubkeys`.
    ///
    /// Deduplicated on the way in, so a peer cannot weigh the set by proving
    /// one pubkey twice. A peer that proved nothing binds as blocked: it is not
    /// a peer, and every path below reads that as "pass nothing".
    #[must_use]
    pub fn bind(
        link: LinkId,
        pubkeys: impl IntoIterator<Item = PublicKey>,
        policy: &Policy,
    ) -> Self {
        let pubkeys: BTreeSet<PublicKey> = pubkeys.into_iter().collect();
        let standing = pubkeys
            .iter()
            .map(|pubkey| policy.graph.standing(pubkey))
            .reduce(Standing::combine)
            .unwrap_or(Standing::Blocked);

        Self {
            link,
            pubkeys,
            policy: policy.clone().for_standing(standing),
        }
    }

    /// Every pubkey the peer proved.
    pub fn pubkeys(&self) -> impl Iterator<Item = &PublicKey> {
        self.pubkeys.iter()
    }

    /// The user's settings as they apply to this device.
    #[must_use]
    pub fn policy(&self) -> &PeerPolicy {
        &self.policy
    }

    /// Where the device stands in the user's graph.
    #[must_use]
    pub fn standing(&self) -> Standing {
        self.policy.standing()
    }

    /// Whether `event` was authored by this peer.
    #[must_use]
    pub fn authored<E: HasPubkey>(&self, event: &E) -> bool {
        self.pubkeys.contains(event.pubkey())
    }

    /// Whether the user has blocked this device.
    #[must_use]
    pub fn is_blocked(&self) -> bool {
        self.policy.is_blocked()
    }

    /// Whether the user trusts this device.
    #[must_use]
    pub fn is_trusted(&self) -> bool {
        self.standing() == Standing::Trusted
    }

    /// Whether `event` may be served to this peer.
    #[must_use]
    pub fn may_be_served<E>(&self, event: &E) -> bool
    where
        E: HasId + HasKind + HasPubkey + HasTags + HasCreatedAt,
    {
        self.policy.should_gossip(event)
    }

    /// Whether an event this peer offered is one to store.
    #[must_use]
    pub fn may_store<E: HasPubkey>(&self, event: &E) -> bool {
        self.policy.should_accept(event)
    }

    /// Whether this peer may be handed the author's signature that lets it
    /// forward an event one more hop.
    #[must_use]
    pub fn may_forward(&self) -> bool {
        self.policy.may_forward()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::fixtures::author;

    /// A graph with one of each tier, so a set can mix them.
    fn policy() -> Policy {
        let mut policy = Policy::new(author(1));

        policy.graph.trusted.insert(author(2));
        policy.graph.network.insert(author(3));
        policy.graph.blocked.insert(author(9));

        policy
    }

    #[test]
    fn a_device_takes_the_best_standing_it_proved() {
        // Proving an extra key is a claim to more access, never less, and the
        // peer could have made the better claim on its own — so the set
        // reduces to the best of them.
        let policy = policy();

        assert_eq!(
            Peer::bind(LinkId(1), [author(3), author(2)], &policy).standing(),
            Standing::Trusted
        );
        assert_eq!(
            Peer::bind(LinkId(1), [author(3), author(4)], &policy).standing(),
            Standing::Network
        );
        assert_eq!(
            Peer::bind(LinkId(1), [author(4)], &policy).standing(),
            Standing::Stranger
        );
    }

    #[test]
    fn one_blocked_identity_blocks_the_device() {
        // The other direction: blocking is a decision about a person, and a
        // device holding that key is theirs whatever else it also signs with.
        let peer = Peer::bind(LinkId(1), [author(2), author(9)], &policy());

        assert!(peer.is_blocked());
        assert!(!peer.is_trusted());
        assert!(!peer.may_forward());
    }

    #[test]
    fn proving_one_pubkey_twice_does_not_weigh_the_set() {
        let peer = Peer::bind(LinkId(1), [author(2), author(2)], &policy());

        assert_eq!(peer.pubkeys().count(), 1);
    }

    #[test]
    fn a_peer_that_proved_nothing_passes_nothing() {
        let peer = Peer::bind(LinkId(1), [], &policy());

        assert!(peer.is_blocked());
        assert!(!peer.may_forward());
    }

    #[test]
    fn forwarding_follows_the_devices_standing() {
        // `forward` defaults to Trusted, so a device that proved a trusted key
        // qualifies even alongside one the user has never heard of.
        let policy = policy();

        assert!(Peer::bind(LinkId(1), [author(2), author(4)], &policy).may_forward());
        assert!(!Peer::bind(LinkId(1), [author(3)], &policy).may_forward());
    }
}
