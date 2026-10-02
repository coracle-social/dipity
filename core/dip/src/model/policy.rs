//! Everything the user has said about who gets what.
//!
//! [`Policy`] is `docs/policy.md` in full — every preference the document
//! defines, plus the trust graph its tiers are measured against — and it is
//! what the rest of the core asks rather than reading preference keys one at a
//! time.
//!
//! Bind it to the pubkey on the other end of a session with
//! [`Policy::for_pubkey`] and the questions a session asks have answers:
//!
//! ```
//! use coracle_lib::keys::SecretKey;
//! use dip::model::Policy;
//!
//! let identity = SecretKey::generate().public_key();
//! let peer = SecretKey::generate().public_key();
//! let policy = Policy::new(identity).for_pubkey(peer);
//!
//! assert!(!policy.is_blocked());
//! ```

use coracle_lib::events::{HasCreatedAt, HasId, HasKind, HasPubkey, HasTags};
use coracle_lib::keys::PublicKey;
use serde::Serialize;

use crate::model::{Authors, Graph, Scope, Standing, Visibility};
use crate::util::Window;

/// The window the disclosure budget counts `AUTH` responses over, in seconds.
///
/// A rolling day, conservative against the doc's "per discoverable window":
/// resetting at each window boundary would admit more strangers, not fewer.
pub const DISCLOSURE_WINDOW_SECONDS: i64 = 24 * 60 * 60;

/// Everything the user has said about who gets what, ready to apply.
///
/// Its serde form is the half a preference carries, which is what the view
/// draws: [`identity`](Self::identity) and [`graph`](Self::graph) are skipped.
/// So a setting added here reaches the view with nothing else naming it, and
/// the defaults in [`Policy::new`] are the only ones anywhere.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Policy {
    /// This device's pubkey.
    #[serde(skip)]
    pub identity: PublicKey,
    /// How long the device keeps accepting unknown peers after backgrounding.
    pub cool_off_minutes: i64,
    /// When the user is willing to be passively discoverable.
    pub discoverable_times: Vec<Window>,
    /// How many `AUTH` responses the device will send unrecognized peers per
    /// window. Peers the user approved by hand do not spend it.
    pub disclosure_budget: u32,
    /// Who can see what the user publishes.
    pub visibility: Visibility,
    /// Whose events the device stores from a peer.
    pub accept: Scope,
    /// Whose events the device relays onward.
    pub gossip: Scope,
    /// Which peers may be handed the author's signature over an own event.
    pub forward: Scope,
    /// How long a carried event outlives the last peer to hand it over.
    pub retention_days: u32,
    /// The trust graph the scopes above are measured against.
    #[serde(skip)]
    pub graph: Graph,
}

impl Policy {
    /// Create a policy with an identity and default values.
    #[must_use]
    pub fn new(identity: PublicKey) -> Self {
        Self {
            identity,
            cool_off_minutes: 10,
            discoverable_times: Vec::new(),
            disclosure_budget: 10,
            visibility: Visibility::default(),
            accept: Scope::Lenient,
            gossip: Scope::Network,
            forward: Scope::Trusted,
            retention_days: 30,
            graph: Graph::default(),
        }
    }

    /// Whether the device is passively discoverable at `minute` of the local
    /// day, ignoring the cool-off window and the disclosure budget.
    #[must_use]
    pub fn is_discoverable_at(&self, minute: u16) -> bool {
        self.discoverable_times
            .iter()
            .any(|window| window.contains(minute))
    }

    /// Whether moving from `before` to this changes what this device stores.
    #[must_use]
    pub fn changes_accept(&self, before: &Self) -> bool {
        self.accept != before.accept || self.graph != before.graph
    }

    /// Whether moving from `before` to this changes what moves between peers.
    #[must_use]
    pub fn moves_sync(&self, before: &Self) -> bool {
        self.changes_accept(before)
            || self.gossip != before.gossip
            || self.forward != before.forward
            || self.visibility != before.visibility
    }

    /// Bind this policy to a pubkey the peer proved.
    #[must_use]
    pub fn for_pubkey(self, pubkey: PublicKey) -> PeerPolicy {
        let standing = self.graph.standing(&pubkey);

        self.for_standing(standing)
    }

    /// Resolve against a standing already worked out.
    #[must_use]
    pub fn for_standing(self, standing: Standing) -> PeerPolicy {
        PeerPolicy {
            policy: self,
            standing,
        }
    }
}

/// A [`Policy`] bound to one pubkey a peer proved.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PeerPolicy {
    /// The policy this was bound from.
    pub policy: Policy,
    /// Where the pubkey stands in the user's graph.
    pub standing: Standing,
}

impl PeerPolicy {
    /// Whether the pubkey is blocked, in which case nothing passes in either
    /// direction and the session should not have been accepted at all.
    #[must_use]
    pub fn is_blocked(&self) -> bool {
        self.standing == Standing::Blocked
    }

    /// Whether this pubkey may see an event of the user's own.
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

        event.pubkey() == &self.policy.identity
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

        if event.pubkey() == &self.policy.identity {
            return self.is_visible(event);
        }

        self.policy
            .gossip
            .admits(self.policy.graph.standing(event.pubkey()))
    }

    /// Which authors this peer may be served, before visibility narrows the
    /// user's own events by category.
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

    /// Whether this peer may be handed the author's signature over an own
    /// event, which is the capability to forward it one more hop.
    #[must_use]
    pub fn may_forward(&self) -> bool {
        !self.is_blocked() && self.policy.forward.admits(self.standing)
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::*;
    use coracle_lib::tags::Tags;

    use crate::fixtures::{author, event};
    use crate::model::graph::tests::graph;
    use coracle_kinds::profile;

    use crate::model::MUTE;

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
    fn the_serde_form_is_every_setting_and_nothing_derived() {
        let policy = policy();
        let crossed: serde_json::Value = serde_json::to_value(&policy).unwrap();
        let named: BTreeSet<&str> = crossed
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();

        // Every preference key, so a screen reading this needs no defaults of its own.
        assert_eq!(
            named,
            [
                "accept",
                "cool_off_minutes",
                "disclosure_budget",
                "discoverable_times",
                "forward",
                "gossip",
                "retention_days",
                "visibility",
            ]
            .into()
        );

        // Including the visibility rules, which an edit writes back whole.
        assert_eq!(crossed["visibility"]["rules"].as_array().unwrap().len(), 2);
        assert_eq!(crossed["visibility"]["default"], "public");
        assert_eq!(crossed["accept"], "lenient");
        assert_eq!(crossed["retention_days"], 30);
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
            policy.clone().for_pubkey(author(2)).gossip_authors(),
            Authors::Only([us()].into())
        );

        policy.gossip = Scope::Network;
        assert_eq!(
            policy.clone().for_pubkey(author(2)).gossip_authors(),
            Authors::Only([us(), author(2), author(3)].into())
        );

        // And never in the set a lenient scope excludes, which would hide the user entirely.
        policy.gossip = Scope::Lenient;
        policy.graph.blocked.insert(us());
        assert_eq!(
            policy.for_pubkey(author(2)).gossip_authors(),
            Authors::Except([author(4)].into())
        );
    }

    #[test]
    fn visibility_governs_the_users_own_events() {
        let policy = policy();
        let stranger = policy.clone().for_pubkey(author(9));
        let trusted = policy.clone().for_pubkey(author(2));

        let profile = event(us(), profile::KIND, 1, "", Tags::new());
        let note = event(us(), 1, 1, "", Tags::new());
        let mutes = event(us(), MUTE, 1, "", Tags::new());

        // The defaults: the mute list to trusted peers, everything else public.
        assert!(stranger.is_visible(&profile));
        assert!(stranger.is_visible(&note));
        assert!(!stranger.is_visible(&mutes));
        assert!(trusted.is_visible(&mutes));

        assert!(stranger.should_gossip(&event(us(), profile::KIND, 1, "", Tags::new())));
        assert!(stranger.should_gossip(&event(us(), 1, 1, "", Tags::new())));
        assert!(!stranger.should_gossip(&event(us(), MUTE, 1, "", Tags::new())));
        assert!(trusted.should_gossip(&event(us(), MUTE, 1, "", Tags::new())));
    }

    #[test]
    fn gossip_measures_other_peoples_events_against_their_author() {
        let policy = policy();
        let stranger = policy.clone().for_pubkey(author(9));

        // Gossip defaults to network, so a stranger's note goes no further whoever asks.
        assert!(stranger.should_gossip(&event(author(2), 1, 1, "", Tags::new())));
        assert!(stranger.should_gossip(&event(author(3), 1, 1, "", Tags::new())));
        assert!(!stranger.should_gossip(&event(author(9), 1, 1, "", Tags::new())));
        assert!(!stranger.should_gossip(&event(author(4), 1, 1, "", Tags::new())));
    }

    #[test]
    fn accept_measures_an_inbound_event_against_its_author() {
        let policy = policy();
        let peer = policy.clone().for_pubkey(author(9));

        // Default accept is lenient: anyone but a blocked author.
        assert!(peer.should_accept(&event(author(9), 1, 1, "", Tags::new())));
        assert!(peer.should_accept(&event(author(2), 1, 1, "", Tags::new())));
        assert!(!peer.should_accept(&event(author(4), 1, 1, "", Tags::new())));

        // The user's own event, coming home from a peer that carried it.
        let mut strict = policy;
        strict.accept = Scope::Nothing;
        assert!(
            strict
                .for_pubkey(author(9))
                .should_accept(&event(us(), 1, 1, "", Tags::new()))
        );
    }

    #[test]
    fn a_blocked_peer_is_served_nothing_and_offers_nothing() {
        let blocked = policy().for_pubkey(author(4));

        assert!(blocked.is_blocked());
        assert!(!blocked.should_gossip(&event(us(), 1, 1, "", Tags::new())));
        assert!(!blocked.should_accept(&event(author(2), 1, 1, "", Tags::new())));

        // Blocked outranks every rule, including one this peer would otherwise fall inside.
        assert!(!blocked.is_visible(&event(us(), profile::KIND, 1, "", Tags::new())));
    }
}
