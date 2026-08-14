//! Everything the user has said about who gets what.
//!
//! [`Policy`] is `docs/policy.md` in full — every preference the document
//! defines, plus the trust graph its tiers are measured against — and it is
//! what the rest of the core asks rather than reading preference keys one at a
//! time.
//!
//! Bind it to the peer on the other end of a session with [`Policy::for_peer`]
//! and the questions a session asks have answers:
//!
//! ```
//! use coracle_lib::keys::SecretKey;
//! use dip::model::Policy;
//!
//! let identity = SecretKey::generate().public_key();
//! let peer = SecretKey::generate().public_key();
//! let policy = Policy::new(identity).for_peer(peer);
//!
//! assert!(!policy.is_blocked());
//! ```

use coracle_lib::events::{HasCreatedAt, HasId, HasKind, HasPubkey, HasTags};
use coracle_lib::keys::PublicKey;

use crate::model::{Authors, Graph, Scope, Standing, Visibility};
use crate::util::Window;

/// How long the device keeps accepting unknown peers after backgrounding.
pub const DEFAULT_COOL_OFF_MINUTES: i64 = 10;

/// How many new pubkeys the device discloses to per discoverable window.
pub const DEFAULT_DISCLOSURE_BUDGET: u32 = 10;

/// Everything the user has said about who gets what, ready to apply.
///
/// The preference keys of `docs/policy.md`, read back with their defaults, plus
/// the [`Graph`] the tiers are measured against. Assembled once per session by
/// [`query::policy`](crate::db::pref::query::policy) rather than re-read per
/// event, since a session answers the same questions of the same peer many
/// times over.
///
/// `identity` is the device's own pubkey, which is what makes an event the
/// user's own and so governed by a visibility setting rather than by gossip.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Policy {
    /// This device's pubkey.
    pub identity: PublicKey,
    /// How long the device keeps accepting unknown peers after backgrounding.
    pub cool_off_minutes: i64,
    /// When the user is willing to be passively discoverable.
    pub discoverable_times: Vec<Window>,
    /// How many new pubkeys the device will disclose to per discoverable
    /// window.
    pub disclosure_budget: u32,
    /// Who can see what the user publishes.
    pub visibility: Visibility,
    /// Whose events the device stores from a peer.
    pub accept: Scope,
    /// Whose events the device relays onward.
    pub gossip: Scope,
    /// The trust graph the scopes above are measured against.
    pub graph: Graph,
}

impl Policy {
    /// Default settings
    #[must_use]
    pub fn new(identity: PublicKey) -> Self {
        Self {
            identity,
            cool_off_minutes: DEFAULT_COOL_OFF_MINUTES,
            discoverable_times: Vec::new(),
            disclosure_budget: DEFAULT_DISCLOSURE_BUDGET,
            visibility: Visibility::default(),
            accept: Scope::Lenient,
            gossip: Scope::Network,
            graph: Graph::default(),
        }
    }

    /// Whether the user has muted `pubkey`.
    #[must_use]
    pub fn is_muted(&self, pubkey: &PublicKey) -> bool {
        self.graph.is_muted(pubkey)
    }

    /// Whether the device is passively discoverable at `minute` of the local
    /// day, ignoring the cool-off window and the disclosure budget.
    #[must_use]
    pub fn is_discoverable_at(&self, minute: u16) -> bool {
        self.discoverable_times
            .iter()
            .any(|window| window.contains(minute))
    }

    /// Bind this policy to the peer on the other end of a session.
    #[must_use]
    pub fn for_peer(self, peer: PublicKey) -> PeerPolicy {
        let standing = self.graph.standing(&peer);

        PeerPolicy {
            policy: self,
            peer,
            standing,
        }
    }
}

/// A [`Policy`] bound to the peer on the other end of a session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PeerPolicy {
    policy: Policy,
    peer: PublicKey,
    standing: Standing,
}

impl PeerPolicy {
    /// The policy this was bound from.
    #[must_use]
    pub fn policy(&self) -> &Policy {
        &self.policy
    }

    /// The peer it is bound to.
    #[must_use]
    pub fn peer(&self) -> &PublicKey {
        &self.peer
    }

    /// This device's own pubkey.
    #[must_use]
    pub fn identity(&self) -> &PublicKey {
        &self.policy.identity
    }

    /// Where the peer stands in the user's graph.
    #[must_use]
    pub fn standing(&self) -> Standing {
        self.standing
    }

    /// Whether the peer is blocked, in which case nothing passes in either
    /// direction and the session should not have been accepted at all.
    #[must_use]
    pub fn is_blocked(&self) -> bool {
        self.standing == Standing::Blocked
    }

    /// Whether the peer may see this event of the user's own.
    #[must_use]
    pub fn is_visible<E>(&self, event: &E) -> bool
    where
        E: HasId + HasKind + HasPubkey + HasTags + HasCreatedAt,
    {
        !self.is_blocked()
            && self
                .policy
                .visibility
                .scope_for(event)
                .admits(self.standing)
    }

    /// Whether an event this peer offered is one to store.
    #[must_use]
    pub fn should_accept<E: HasPubkey>(&self, event: &E) -> bool {
        if self.is_blocked() {
            return false;
        }

        event.pubkey() == self.identity()
            || self
                .policy
                .accept
                .admits(self.policy.graph.standing(event.pubkey()))
    }

    /// Whether an event may be shared with this peer.
    #[must_use]
    pub fn should_gossip<E>(&self, event: &E) -> bool
    where
        E: HasId + HasKind + HasPubkey + HasTags + HasCreatedAt,
    {
        if self.is_blocked() {
            return false;
        }

        if event.pubkey() == self.identity() {
            return self.is_visible(event);
        }

        self.policy
            .gossip
            .admits(self.policy.graph.standing(event.pubkey()))
    }

    /// Which authors this peer may be served, before visibility narrows the
    /// user's own events by category.
    ///
    /// The user is always in the set: their own events are governed by
    /// visibility, and a Gossip scope that excludes them would hide the user
    /// from every peer.
    #[must_use]
    pub fn gossip_authors(&self) -> Authors {
        match self.policy.gossip.authors(&self.policy.graph) {
            Authors::Any => Authors::Any,
            Authors::Only(mut authors) => {
                authors.insert(self.policy.identity);
                Authors::Only(authors)
            }
            Authors::Except(mut authors) => {
                authors.remove(&self.policy.identity);
                Authors::Except(authors)
            }
        }
    }

    /// Which authors an event may be accepted from, for the filter this device
    /// reconciles against the peer with.
    #[must_use]
    pub fn accept_authors(&self) -> Authors {
        self.policy.accept.authors(&self.policy.graph)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use coracle_lib::tags::Tags;

    use crate::fixtures::{author, event};
    use crate::model::graph::tests::graph;
    use crate::model::{KIND_MUTE, KIND_PROFILE};

    /// The device whose policy is under test.
    fn us() -> PublicKey {
        author(1)
    }

    fn policy() -> Policy {
        Policy {
            graph: graph(),
            ..Policy::new(us())
        }
    }

    #[test]
    fn muting_is_read_through_to_the_graph() {
        let policy = policy();

        assert!(policy.is_muted(&author(5)));
        assert!(!policy.is_muted(&author(2)));
    }

    #[test]
    fn discoverability_is_the_union_of_the_windows_set() {
        let mut policy = Policy::new(us());

        // Empty by default, which is nowhere rather than everywhere.
        assert!(!policy.is_discoverable_at(600));

        policy.discoverable_times.push(Window {
            start: 480,
            end: 1080,
        });
        policy.discoverable_times.push(Window {
            start: 1_380,
            end: 360,
        });

        assert!(policy.is_discoverable_at(600));
        assert!(policy.is_discoverable_at(10));
        assert!(!policy.is_discoverable_at(1_200));
    }

    #[test]
    fn the_user_is_always_in_their_own_gossip_set() {
        let mut policy = policy();

        policy.gossip = Scope::Nothing;
        assert_eq!(
            policy.clone().for_peer(author(2)).gossip_authors(),
            Authors::Only([us()].into())
        );

        policy.gossip = Scope::Network;
        assert_eq!(
            policy.clone().for_peer(author(2)).gossip_authors(),
            Authors::Only([us(), author(2), author(3)].into())
        );

        // And is never in the set a lenient scope excludes, which would
        // otherwise hide the user from every peer.
        policy.gossip = Scope::Lenient;
        policy.graph.blocked.insert(us());
        assert_eq!(
            policy.for_peer(author(2)).gossip_authors(),
            Authors::Except([author(4)].into())
        );
    }

    #[test]
    fn visibility_governs_the_users_own_events() {
        let policy = policy();
        let stranger = policy.clone().for_peer(author(9));
        let trusted = policy.clone().for_peer(author(2));

        let profile = event(us(), KIND_PROFILE, 1, "", Tags::new());
        let note = event(us(), 1, 1, "", Tags::new());
        let mutes = event(us(), KIND_MUTE, 1, "", Tags::new());

        // The defaults: the mute list to trusted peers, everything else public.
        assert!(stranger.is_visible(&profile));
        assert!(stranger.is_visible(&note));
        assert!(!stranger.is_visible(&mutes));
        assert!(trusted.is_visible(&mutes));

        assert!(stranger.should_gossip(&event(us(), KIND_PROFILE, 1, "", Tags::new())));
        assert!(stranger.should_gossip(&event(us(), 1, 1, "", Tags::new())));
        assert!(!stranger.should_gossip(&event(us(), KIND_MUTE, 1, "", Tags::new())));
        assert!(trusted.should_gossip(&event(us(), KIND_MUTE, 1, "", Tags::new())));
    }

    #[test]
    fn gossip_measures_other_peoples_events_against_their_author() {
        let policy = policy();
        let stranger = policy.clone().for_peer(author(9));

        // Default gossip is network, so a stranger's note goes no further
        // however trusted the peer asking for it is.
        assert!(stranger.should_gossip(&event(author(2), 1, 1, "", Tags::new())));
        assert!(stranger.should_gossip(&event(author(3), 1, 1, "", Tags::new())));
        assert!(!stranger.should_gossip(&event(author(9), 1, 1, "", Tags::new())));
        assert!(!stranger.should_gossip(&event(author(4), 1, 1, "", Tags::new())));
    }

    #[test]
    fn accept_measures_an_inbound_event_against_its_author() {
        let policy = policy();
        let peer = policy.clone().for_peer(author(9));

        // Default accept is lenient: anyone but a blocked author.
        assert!(peer.should_accept(&event(author(9), 1, 1, "", Tags::new())));
        assert!(peer.should_accept(&event(author(2), 1, 1, "", Tags::new())));
        assert!(!peer.should_accept(&event(author(4), 1, 1, "", Tags::new())));

        // The user's own event, coming home from a peer that carried it.
        let mut strict = policy;
        strict.accept = Scope::Nothing;
        assert!(
            strict
                .for_peer(author(9))
                .should_accept(&event(us(), 1, 1, "", Tags::new()))
        );
    }

    #[test]
    fn a_blocked_peer_is_served_nothing_and_offers_nothing() {
        let blocked = policy().for_peer(author(4));

        assert!(blocked.is_blocked());
        assert!(!blocked.should_gossip(&event(us(), 1, 1, "", Tags::new())));
        assert!(!blocked.should_accept(&event(author(2), 1, 1, "", Tags::new())));

        // Blocked outranks every rule, including one this peer would otherwise
        // fall inside.
        assert!(!blocked.is_visible(&event(us(), KIND_PROFILE, 1, "", Tags::new())));
    }
}
