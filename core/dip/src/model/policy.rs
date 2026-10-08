//! Everything the user has said about who gets what.
//!
//! [`Policy`] is `docs/policy.md` in full — every preference the document
//! defines, plus the contact graph its tiers are measured against — and it is
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

use crate::model::{Graph, Scope, Sharing, Standing, Visibility};
use crate::util::Window;

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
    /// Times of day when no stranger is told who the user is without asking.
    pub quiet_times: Vec<Window>,
    /// Whether a stranger is told who the user is while the app is not in
    /// front, as often as the [`DisclosureBucket`](crate::model::DisclosureBucket)
    /// allows. Off, every such stranger waits for the user.
    pub discover_in_background: bool,
    /// Who is handed the user's own activity, and who may carry it further.
    pub sharing: Sharing,
    /// Whose events the device stores from a peer.
    pub accept: Scope,
    /// How long a carried event stays after it first reaches this device.
    pub retention_days: u32,
    /// The contact graph the scopes above are measured against.
    #[serde(skip)]
    pub graph: Graph,
}

impl Policy {
    /// Create a policy with an identity and default values.
    #[must_use]
    pub fn new(identity: PublicKey) -> Self {
        Self {
            identity,
            quiet_times: Vec::new(),
            discover_in_background: true,
            sharing: Sharing::default(),
            accept: Scope::Lenient,
            retention_days: 90,
            graph: Graph::default(),
        }
    }

    /// Whether `minute` of the local day falls in one of the quiet times.
    #[must_use]
    pub fn is_quiet_at(&self, minute: u16) -> bool {
        self.quiet_times
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
        self.changes_accept(before) || self.sharing != before.sharing
    }

    /// Which of the user's own events go to whom: fixed rules for their lists,
    /// and [`sharing`](Self::sharing) for everything else.
    #[must_use]
    pub fn visibility(&self) -> Visibility {
        Visibility::for_sharing(self.sharing)
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

    /// Whether this pubkey is somebody the user paired with, which is who may
    /// be handed what this device carries for others, and who may be handed
    /// the user's signature.
    #[must_use]
    pub fn is_contact(&self) -> bool {
        self.standing == Standing::Contact
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
                .visibility()
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

    /// Whether an event may be handed to this peer: the user's own as
    /// [`sharing`](Policy::sharing) allows, and anybody else's only to a
    /// contact, which keeps a second hop among people somebody paired with.
    #[must_use]
    pub fn may_share<E>(&self, event: &E) -> bool
    where
        E: HasId + HasKind + HasPubkey + HasTags + HasCreatedAt,
    {
        if event.pubkey() == &self.policy.identity {
            return self.is_visible(event);
        }

        !self.is_blocked() && self.is_contact()
    }

    /// Whether this peer may be handed the author's signature over an own
    /// event, which is the capability to carry it one more hop.
    #[must_use]
    pub fn signs(&self) -> bool {
        !self.is_blocked() && self.policy.sharing.signs() && self.is_contact()
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

        // Every preference key, so that a screen reading this needs no defaults of its own.
        assert_eq!(
            named,
            [
                "accept",
                "discover_in_background",
                "quiet_times",
                "retention_days",
                "sharing",
            ]
            .into()
        );

        assert_eq!(crossed["sharing"], "anyone");
        assert_eq!(crossed["accept"], "lenient");
        assert_eq!(crossed["retention_days"], 90);
    }

    #[test]
    fn quiet_is_the_union_of_the_windows_set() {
        let mut policy = Policy::new(us());

        // Empty by default, which is never rather than always.
        assert!(!policy.is_quiet_at(600));

        policy.quiet_times.push(Window {
            start: 480,
            end: 1080,
        });
        policy.quiet_times.push(Window {
            start: 1_380,
            end: 360,
        });

        assert!(policy.is_quiet_at(600));
        assert!(policy.is_quiet_at(10));
        assert!(!policy.is_quiet_at(1_200));
    }

    #[test]
    fn visibility_governs_the_users_own_events() {
        let policy = policy();
        let stranger = policy.clone().for_pubkey(author(9));
        let contact = policy.clone().for_pubkey(author(2));

        let profile = event(us(), profile::KIND, 1, "", Tags::new());
        let note = event(us(), 1, 1, "", Tags::new());
        let mutes = event(us(), MUTE, 1, "", Tags::new());

        // The defaults: the mute list to contacts, everything else to anyone.
        assert!(stranger.may_share(&profile));
        assert!(stranger.may_share(&note));
        assert!(!stranger.may_share(&mutes));
        assert!(contact.may_share(&mutes));
    }

    #[test]
    fn somebody_elses_event_goes_only_to_a_contact() {
        let policy = policy();
        let theirs = event(author(3), 1, 1, "", Tags::new());

        assert!(policy.clone().for_pubkey(author(2)).may_share(&theirs));
        assert!(!policy.clone().for_pubkey(author(3)).may_share(&theirs));
        assert!(!policy.for_pubkey(author(9)).may_share(&theirs));
    }

    #[test]
    fn sharing_sets_who_sees_the_users_events_and_who_can_carry_them() {
        let mut policy = policy();
        let note = event(us(), 1, 1, "", Tags::new());
        let at = |policy: &Policy, pubkey| policy.clone().for_pubkey(pubkey);

        // Anyone: a stranger sees it first-hand, and only contacts get a signature.
        assert!(at(&policy, author(9)).may_share(&note));
        assert!(!at(&policy, author(9)).signs());
        assert!(at(&policy, author(2)).signs());

        policy.sharing = Sharing::Network;
        assert!(!at(&policy, author(9)).may_share(&note));
        assert!(at(&policy, author(2)).may_share(&note));
        assert!(at(&policy, author(2)).signs());

        policy.sharing = Sharing::Contacts;
        assert!(at(&policy, author(2)).may_share(&note));
        assert!(!at(&policy, author(2)).signs());
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
        assert!(!blocked.may_share(&event(us(), 1, 1, "", Tags::new())));
        assert!(!blocked.signs());
        assert!(!blocked.should_accept(&event(author(2), 1, 1, "", Tags::new())));

        // Blocked outranks every rule, including one this peer would otherwise fall inside.
        assert!(!blocked.is_visible(&event(us(), profile::KIND, 1, "", Tags::new())));
    }
}
