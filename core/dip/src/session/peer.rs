//! What a peer has proved about itself.

use std::collections::BTreeSet;

use coracle_lib::events::{HasCreatedAt, HasId, HasKind, HasPubkey, HasTags};
use coracle_lib::keys::PublicKey;

use crate::link::LinkId;
use crate::model::{Policy, PubkeyPolicy};

/// A peer — the device on the other end of the session — that has completed
/// NIP-42, and the policy governing it.
///
/// Built by [`Session`](super::Session) when identification completes and held
/// there for the life of the session. A function that takes one cannot be
/// called before the peer is identified.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Peer {
    /// The link it was proved over. A `Peer` does not outlive its session.
    pub link: LinkId,
    /// This device's own pubkey, which both authorship registers are measured
    /// from.
    pub identity: PublicKey,
    /// The user's policy, bound to each pubkey the peer proved.
    ///
    /// A device may hold several identities and prove them all over one
    /// channel. Each entry names its own pubkey, so this is also the set of
    /// pubkeys ([`pubkeys`](Self::pubkeys)).
    pub policies: Vec<PubkeyPolicy>,
}

impl Peer {
    /// Bind `policy` to every pubkey the peer proved.
    ///
    /// Deduplicated and ordered on the way in, so a peer cannot lengthen the
    /// list by proving the same pubkey twice.
    #[must_use]
    pub fn bind(
        link: LinkId,
        pubkeys: impl IntoIterator<Item = PublicKey>,
        policy: &Policy,
    ) -> Self {
        let policies = pubkeys
            .into_iter()
            .collect::<BTreeSet<_>>()
            .into_iter()
            .map(|pubkey| policy.clone().for_pubkey(pubkey))
            .collect();

        Self {
            link,
            identity: policy.identity,
            policies,
        }
    }

    /// Every pubkey the peer proved.
    pub fn pubkeys(&self) -> impl Iterator<Item = &PublicKey> {
        self.policies.iter().map(PubkeyPolicy::pubkey)
    }

    /// Take the pubkeys back out, for rebinding under a policy that has changed.
    pub fn into_pubkeys(self) -> impl Iterator<Item = PublicKey> {
        self.policies.into_iter().map(|policy| *policy.pubkey())
    }

    /// Whether `event` was authored by this peer.
    #[must_use]
    pub fn authored<E: HasPubkey>(&self, event: &E) -> bool {
        self.pubkeys().any(|pubkey| pubkey == event.pubkey())
    }

    /// Whether any pubkey the peer proved is blocked.
    #[must_use]
    pub fn is_blocked(&self) -> bool {
        self.policies.iter().any(PubkeyPolicy::is_blocked)
    }

    /// Whether any pubkey the peer proved may be served `event`.
    #[must_use]
    pub fn may_be_served<E>(&self, event: &E) -> bool
    where
        E: HasId + HasKind + HasPubkey + HasTags + HasCreatedAt,
    {
        !self.is_blocked()
            && self
                .policies
                .iter()
                .any(|policy| policy.should_gossip(event))
    }

    /// Whether an event this peer offered is one to store, under any pubkey
    /// it proved.
    #[must_use]
    pub fn may_store<E: HasPubkey>(&self, event: &E) -> bool {
        !self.is_blocked()
            && self
                .policies
                .iter()
                .any(|policy| policy.should_accept(event))
    }
}
