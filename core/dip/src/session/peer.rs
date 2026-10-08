//! What a peer has proved about itself.

use coracle_lib::keys::PublicKey;

use crate::link::LinkId;
use crate::model::{Identity, PeerPolicy, Policy, Standing};

/// A peer — the device on the other end of the session — that has completed
/// NIP-42, and what the user's settings say about it.
///
/// What a device proved is a set, because it may hold several identities and
/// prove them all over one channel. What the user's settings say about it is not:
/// the contact graph names people, and every question the sync layer asks — what
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
    pub pubkeys: Identity,
    /// The user's policy, resolved against the whole set.
    pub policy: PeerPolicy,
}

impl Peer {
    /// Bind `policy` to the device that proved `pubkeys`.
    ///
    /// Deduplicated on the way in, so that a peer cannot weigh the set by proving
    /// one pubkey twice. A peer that proved nothing binds as blocked: it is not
    /// a peer, and every path below reads that as "pass nothing".
    #[must_use]
    pub fn bind(
        link: LinkId,
        pubkeys: impl IntoIterator<Item = PublicKey>,
        policy: &Policy,
    ) -> Self {
        let pubkeys: Identity = pubkeys.into_iter().collect();
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
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::fixtures::author;

    /// A graph with one of each tier, so that a set can mix them.
    fn policy() -> Policy {
        let mut policy = Policy::new(author(1));

        policy.graph.contacts.insert(author(2));
        policy.graph.network.insert(author(3));
        policy.graph.blocked.insert(author(9));

        policy
    }

    #[test]
    fn a_device_takes_the_best_standing_it_proved() {
        // The set reduces to the best, because proving an extra key claims more access, never less.
        let policy = policy();

        assert_eq!(
            Peer::bind(LinkId(1), [author(3), author(2)], &policy)
                .policy
                .standing,
            Standing::Contact
        );
        assert_eq!(
            Peer::bind(LinkId(1), [author(3), author(4)], &policy)
                .policy
                .standing,
            Standing::Network
        );
        assert_eq!(
            Peer::bind(LinkId(1), [author(4)], &policy).policy.standing,
            Standing::Stranger
        );
    }

    #[test]
    fn one_blocked_identity_blocks_the_device() {
        // The other direction: blocking is about a person, whatever else the device signs with.
        let peer = Peer::bind(LinkId(1), [author(2), author(9)], &policy());

        assert!(peer.policy.is_blocked());
        assert_ne!(peer.policy.standing, Standing::Contact);
        assert!(!peer.policy.signs());
    }

    #[test]
    fn proving_one_pubkey_twice_does_not_weigh_the_set() {
        let peer = Peer::bind(LinkId(1), [author(2), author(2)], &policy());

        assert_eq!(peer.pubkeys.len(), 1);
    }

    #[test]
    fn a_peer_that_proved_nothing_passes_nothing() {
        let peer = Peer::bind(LinkId(1), [], &policy());

        assert!(peer.policy.is_blocked());
        assert!(!peer.policy.signs());
    }

    #[test]
    fn signing_follows_the_devices_standing() {
        // One proved contact's key qualifies the device, because a contact is signed for.
        let policy = policy();

        assert!(
            Peer::bind(LinkId(1), [author(2), author(4)], &policy)
                .policy
                .signs()
        );
        assert!(!Peer::bind(LinkId(1), [author(3)], &policy).policy.signs());
    }
}
