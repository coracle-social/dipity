//! Stored preferences, and the policy the core reads back out of them.
//!
//! [`Pref`] is one stored key. [`Policy`] is `docs/policy.md` in full — every
//! preference the document defines, plus the trust graph its tiers are measured
//! against — and it is what the rest of the core asks rather than reading keys
//! one at a time.
//!
//! Bind it to the peer on the other end of a session with [`Policy::for_peer`]
//! and the questions a session asks have answers:
//!
//! ```
//! use coracle_lib::keys::SecretKey;
//! use dip::domain::pref::model::Policy;
//!
//! let identity = SecretKey::generate().public_key();
//! let peer = SecretKey::generate().public_key();
//! let policy = Policy::new(identity).for_peer(peer);
//!
//! assert!(!policy.is_blocked());
//! ```

use std::collections::BTreeSet;

use coracle_lib::events::{HasKind, HasPubkey};
use coracle_lib::keys::PublicKey;
use serde::{Deserialize, Serialize};

use crate::domain::event::model::{KIND_MUTE, KIND_PROFILE};
use crate::util::Window;

/// One stored preference.
///
/// Values are JSON documents, so a preference can grow from a scalar into a
/// structure without a migration, and so the view and the core agree on what a
/// value means without a per-key encoding.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Pref {
    /// The key, dotted by area.
    pub key: String,
    /// The value, as a JSON document.
    pub value: String,
    /// When it was last written.
    pub updated_at: i64,
}

/// The keys defined for user preferences.
pub mod keys {
    /// How long the device keeps accepting unknown peers after the app is backgrounded.
    pub const COOL_OFF_MINUTES: &str = "policy.cool_off_minutes";

    /// Times of day, in the device's local timezone, when the user is passively discoverable.
    pub const DISCOVERABLE_TIMES: &str = "policy.discoverable_times";

    /// How many new pubkeys the device will disclose to per discoverable window.
    pub const DISCLOSURE_BUDGET: &str = "policy.disclosure_budget";

    /// Who can see the user's profile. Default `public`.
    pub const PROFILE_VISIBILITY: &str = "policy.visibility.profile";

    /// Who can see the user's content. Default `public`.
    pub const CONTENT_VISIBILITY: &str = "policy.visibility.content";

    /// Who can see the user's trust, block and mute lists. Default `trusted`.
    pub const METADATA_VISIBILITY: &str = "policy.visibility.metadata";

    /// Whose events the device stores from a peer. Default `lenient`.
    pub const ACCEPT: &str = "policy.accept";

    /// Whose events the device relays onward. Default `network`.
    pub const GOSSIP: &str = "policy.gossip";
}

/// Where a pubkey stands in the user's graph.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Standing {
    /// Blocked. Dropped on ingest, never served, sessions refused.
    Blocked,
    /// Explicitly trusted.
    Trusted,
    /// Transitively trusted, two hops out.
    Network,
    /// Everyone else.
    Stranger,
}

/// The user's trust graph: the tiers, as sets of pubkeys.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Graph {
    /// People the user explicitly trusts.
    pub trusted: BTreeSet<PublicKey>,
    /// People a trusted person trusts, which is as far as the graph reaches.
    pub network: BTreeSet<PublicKey>,
    /// People the user has blocked.
    pub blocked: BTreeSet<PublicKey>,
    /// People the user has muted.
    pub muted: BTreeSet<PublicKey>,
}

impl Graph {
    /// Where `pubkey` stands.
    #[must_use]
    pub fn standing(&self, pubkey: &PublicKey) -> Standing {
        if self.blocked.contains(pubkey) {
            Standing::Blocked
        } else if self.trusted.contains(pubkey) {
            Standing::Trusted
        } else if self.network.contains(pubkey) {
            Standing::Network
        } else {
            Standing::Stranger
        }
    }

    /// Whether the user has muted `pubkey`.
    #[must_use]
    pub fn is_muted(&self, pubkey: &PublicKey) -> bool {
        self.muted.contains(pubkey)
    }
}

/// The authors admitted for a resolved standing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Authors {
    /// Every author.
    Any,
    /// Only these. An empty set admits nobody.
    Only(BTreeSet<PublicKey>),
    /// Everyone but these. An empty set excludes nobody.
    Except(BTreeSet<PublicKey>),
}

/// The tiers every policy setting is expressed on, narrowest first.
///
/// Ordered, so a wider scope admits everyone a narrower one does and
/// [`admits`](Self::admits) is a comparison rather than a table.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Scope {
    /// Nobody. Gossip only, where it means the device shares the user's own
    /// content and relays nothing.
    Nothing,
    /// People the user explicitly trusts.
    Trusted,
    /// People the user transitively trusts, two hops out.
    Network,
    /// Anyone who connects, except blocked pubkeys.
    Lenient,
    /// Anyone. Visibility settings only.
    Public,
}

impl Scope {
    /// Whether a pubkey standing here falls inside this scope.
    #[must_use]
    pub fn admits(self, standing: Standing) -> bool {
        match standing {
            Standing::Blocked => false,
            Standing::Trusted => self >= Self::Trusted,
            Standing::Network => self >= Self::Network,
            Standing::Stranger => self >= Self::Lenient,
        }
    }

    /// Which authors this scope admits, resolved against `graph`.
    #[must_use]
    pub fn authors(self, graph: &Graph) -> Authors {
        match self {
            Self::Nothing => Authors::Only(BTreeSet::new()),
            Self::Trusted => Authors::Only(graph.trusted.clone()),
            Self::Network => Authors::Only(
                graph
                    .trusted
                    .union(&graph.network)
                    .copied()
                    .collect::<BTreeSet<_>>(),
            ),
            Self::Lenient => Authors::Except(graph.blocked.clone()),
            Self::Public => Authors::Any,
        }
    }
}

/// Which visibility setting governs an event the user wrote.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EventCategory {
    /// The user's profile.
    Profile,
    /// The user's trust, block and mute lists.
    Metadata,
    /// Everything else the user writes.
    Content,
}

impl EventCategory {
    /// Every category, so a caller can ask about each in turn.
    pub const ALL: [Self; 3] = [Self::Profile, Self::Metadata, Self::Content];

    /// The kinds a category other than [`Content`](Self::Content) names.
    const NAMED: [(Self, u16); 2] = [(Self::Profile, KIND_PROFILE), (Self::Metadata, KIND_MUTE)];

    /// Which category governs an event of this kind.
    #[must_use]
    pub fn of(kind: u16) -> Self {
        Self::NAMED
            .iter()
            .find(|(_, named)| *named == kind)
            .map_or(Self::Content, |(category, _)| *category)
    }

    /// Every kind some category other than [`Content`](Self::Content) names.
    #[must_use]
    pub fn named_kinds() -> Vec<u16> {
        Self::NAMED.iter().map(|(_, kind)| *kind).collect()
    }

    /// The kinds this category names.
    #[must_use]
    pub fn kinds(self) -> Vec<u16> {
        Self::NAMED
            .iter()
            .filter(|(category, _)| *category == self)
            .map(|(_, kind)| *kind)
            .collect()
    }
}

/// How long the device keeps accepting unknown peers after backgrounding.
pub const DEFAULT_COOL_OFF_MINUTES: i64 = 10;

/// How many new pubkeys the device discloses to per discoverable window.
pub const DEFAULT_DISCLOSURE_BUDGET: u32 = 10;

/// Everything the user has said about who gets what, ready to apply.
///
/// The preference keys of `docs/policy.md`, read back with their defaults, plus
/// the [`Graph`] the tiers are measured against. Assembled once per session by
/// [`query::policy`](super::query::policy) rather than re-read per event, since
/// a session answers the same questions of the same peer many times over.
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
    /// Who can see the user's profile.
    pub profile_visibility: Scope,
    /// Who can see the user's content.
    pub content_visibility: Scope,
    /// Who can see the user's trust, block and mute lists.
    pub metadata_visibility: Scope,
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
            profile_visibility: Scope::Public,
            content_visibility: Scope::Public,
            metadata_visibility: Scope::Trusted,
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

    /// Whether the peer may see the user's own events of this category.
    #[must_use]
    pub fn sees(&self, category: EventCategory) -> bool {
        !self.is_blocked() && self.visibility(category).admits(self.standing)
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
    pub fn should_gossip<E: HasPubkey + HasKind>(&self, event: &E) -> bool {
        if self.is_blocked() {
            return false;
        }

        if event.pubkey() == self.identity() {
            return self.sees(EventCategory::of(event.kind()));
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

    /// The visibility setting governing a category of the user's own events.
    fn visibility(&self, category: EventCategory) -> Scope {
        match category {
            EventCategory::Profile => self.policy.profile_visibility,
            EventCategory::Metadata => self.policy.metadata_visibility,
            EventCategory::Content => self.policy.content_visibility,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use coracle_lib::tags::Tags;

    use crate::domain::event::fixtures::{author, event};

    /// The device whose policy is under test.
    fn us() -> PublicKey {
        author(1)
    }

    fn policy() -> Policy {
        let mut policy = Policy::new(us());

        policy.graph.trusted.insert(author(2));
        policy.graph.network.insert(author(3));
        policy.graph.blocked.insert(author(4));
        policy.graph.muted.insert(author(5));

        policy
    }

    #[test]
    fn block_wins_over_trust() {
        let mut graph = Graph::default();

        graph.trusted.insert(author(2));
        graph.blocked.insert(author(2));

        assert_eq!(graph.standing(&author(2)), Standing::Blocked);
    }

    #[test]
    fn standing_reads_the_graph_in_tiers() {
        let policy = policy();

        assert_eq!(policy.graph.standing(&author(2)), Standing::Trusted);
        assert_eq!(policy.graph.standing(&author(3)), Standing::Network);
        assert_eq!(policy.graph.standing(&author(4)), Standing::Blocked);
        assert_eq!(policy.graph.standing(&author(9)), Standing::Stranger);

        // Muting is not a tier: a muted author still gossips normally.
        assert_eq!(policy.graph.standing(&author(5)), Standing::Stranger);
        assert!(policy.is_muted(&author(5)));
        assert!(!policy.is_muted(&author(2)));
    }

    #[test]
    fn a_wider_scope_admits_everyone_a_narrower_one_does() {
        for standing in [Standing::Trusted, Standing::Network, Standing::Stranger] {
            let admitted: Vec<bool> = [
                Scope::Nothing,
                Scope::Trusted,
                Scope::Network,
                Scope::Lenient,
                Scope::Public,
            ]
            .into_iter()
            .map(|scope| scope.admits(standing))
            .collect();

            assert!(
                admitted.windows(2).all(|pair| pair[0] <= pair[1]),
                "{standing:?} fell out of a scope it was in: {admitted:?}"
            );
        }

        // Block is not a tier, so it is outside even the widest scope.
        assert!(!Scope::Public.admits(Standing::Blocked));
        assert!(Scope::Trusted.admits(Standing::Trusted));
        assert!(!Scope::Trusted.admits(Standing::Network));
        assert!(Scope::Network.admits(Standing::Network));
        assert!(!Scope::Network.admits(Standing::Stranger));
        assert!(Scope::Lenient.admits(Standing::Stranger));
        assert!(!Scope::Nothing.admits(Standing::Trusted));
    }

    #[test]
    fn a_scope_is_stored_as_the_word_the_document_uses() {
        assert_eq!(
            serde_json::to_string(&Scope::Lenient).unwrap(),
            r#""lenient""#
        );
        assert_eq!(
            serde_json::from_str::<Scope>(r#""network""#).unwrap(),
            Scope::Network
        );
        assert!(serde_json::from_str::<Scope>(r#""whatever""#).is_err());
    }

    #[test]
    fn a_scope_resolves_to_the_authors_it_admits() {
        let policy = policy();

        assert_eq!(
            Scope::Nothing.authors(&policy.graph),
            Authors::Only(BTreeSet::new())
        );
        assert_eq!(
            Scope::Trusted.authors(&policy.graph),
            Authors::Only([author(2)].into())
        );
        assert_eq!(
            Scope::Network.authors(&policy.graph),
            Authors::Only([author(2), author(3)].into())
        );
        assert_eq!(
            Scope::Lenient.authors(&policy.graph),
            Authors::Except([author(4)].into())
        );
        assert_eq!(Scope::Public.authors(&policy.graph), Authors::Any);
    }

    #[test]
    fn a_category_is_the_kinds_it_names() {
        assert_eq!(EventCategory::of(KIND_PROFILE), EventCategory::Profile);
        assert_eq!(EventCategory::of(KIND_MUTE), EventCategory::Metadata);
        assert_eq!(EventCategory::of(1), EventCategory::Content);

        assert_eq!(EventCategory::Profile.kinds(), [KIND_PROFILE]);
        assert_eq!(EventCategory::Metadata.kinds(), [KIND_MUTE]);
        assert!(EventCategory::Content.kinds().is_empty());
        assert_eq!(EventCategory::named_kinds(), [KIND_PROFILE, KIND_MUTE]);
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
    fn visibility_governs_the_users_own_events_by_category() {
        let policy = policy();
        let stranger = policy.clone().for_peer(author(9));
        let trusted = policy.clone().for_peer(author(2));

        // The defaults: profile and content public, metadata trusted.
        assert!(stranger.sees(EventCategory::Profile));
        assert!(stranger.sees(EventCategory::Content));
        assert!(!stranger.sees(EventCategory::Metadata));
        assert!(trusted.sees(EventCategory::Metadata));

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
        assert!(!blocked.sees(EventCategory::Profile));
    }
}
