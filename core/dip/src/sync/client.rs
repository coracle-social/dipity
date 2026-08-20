//! This device asking a peer: the client half.
//!
//! Where authorization happens. Every inbound event is authorized by the
//! session or by a proof designated to this device, or it is dropped, and no
//! policy flag relaxes it. `docs/proofs.md#authorship-proofs`.

use std::collections::{BTreeMap, BTreeSet};

use anyhow::{Context, Result, bail};
use coracle_lib::events::{EventId, HashedEvent};
use coracle_lib::filters::Filter;
use coracle_lib::sync::{FrameBudget, Item, SyncSet};

use crate::clock;
use crate::db::Db;
use crate::db::command;
use crate::db::query as db_query;
use crate::model::{AuthorshipClaim, AuthorshipProof};
use crate::session::Peer;
use crate::sync::relay;
use crate::sync::{Message, Quota, SubscriptionId};

/// The largest event the store accepts, whatever the peer's standing.
/// `docs/sync.md#quotas` names the cap; this is its value.
pub const MAX_EVENT_BYTES: usize = 64 * 1024;

/// The cap on one negentropy frame, so a reply stays a few fragments rather
/// than a torrent; reconciliation resumes on the next round.
const NEG_FRAME_BYTES: usize = 4 * 1024;

/// Why an event this device was offered was not stored.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Rejected {
    /// Neither authored by the peer nor carrying a proof designated to this
    /// device.
    Unauthorized,
    /// Outside the user's Accept scope.
    OutOfScope,
    /// The peer has written as much as it may this session.
    OverQuota,
    /// Larger than the per-event cap.
    TooLarge,
}

/// What a session has already accepted from one peer, against its [`Quota`].
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Used {
    /// Events accepted this session.
    pub events: u32,
    /// Bytes of them.
    pub bytes: u64,
}

impl Used {
    /// Count an event that was just accepted.
    pub(crate) fn record(&mut self, event: &HashedEvent) {
        self.events = self.events.saturating_add(1);
        self.bytes = self.bytes.saturating_add(event_size(event) as u64);
    }
}

/// An event's count against the byte budget.
fn event_size(event: &HashedEvent) -> usize {
    serde_json::to_vec(event).map_or(0, |encoded| encoded.len())
}

/// The negentropy side of the client half: one open subscription's diff.
pub struct Negotiation {
    /// The subscription this exchange runs on.
    subscription: SubscriptionId,
    /// The filter the exchange is bounded by.
    filter: Filter,
    /// This device's side of the diff, under the filter and the peer's
    /// registers.
    local: SyncSet,
    /// Ids this device has that the peer lacks. Not yet on the wire.
    have: Vec<EventId>,
    /// Ids the peer has that this device lacks, accumulated every round.
    need: Vec<EventId>,
    /// The reply budget for the next message.
    budget: FrameBudget,
}

impl Negotiation {
    /// Begin diffing against `filter`, returning the opening frame to send.
    ///
    /// The local set is whatever this device would offer this peer under the
    /// filter — the same query the relay half serves, since both halves of the
    /// exchange are over the same store.
    pub fn begin(db: &Db, peer: &Peer, filter: Filter) -> Result<(Self, Message)> {
        let local = SyncSet::from_items(
            db_query::list_events(db, &relay::query_for(peer, filter.clone()))?
                .into_iter()
                .map(|event| Item {
                    timestamp: event.created_at,
                    id: event.id,
                }),
        );

        let subscription = {
            let mut id = [0u8; 8];
            let _ = getrandom::getrandom(&mut id);

            SubscriptionId(hex::encode(id))
        };
        let negotiation = Self {
            subscription,
            filter,
            local,
            have: Vec::new(),
            need: Vec::new(),
            budget: FrameBudget::bytes(NEG_FRAME_BYTES),
        };
        let opening = coracle_lib::sync::initiate(&negotiation.local);
        let message = Message::NegOpen(
            negotiation.subscription.clone(),
            negotiation.filter.clone(),
            opening.encode(),
        );

        Ok((negotiation, message))
    }

    /// This exchange's subscription id.
    #[must_use]
    pub fn subscription_id(&self) -> &SubscriptionId {
        &self.subscription
    }

    /// The ids the peer has that this device lacks, once reconciliation has
    /// run to completion. What the terminating `REQ` asks for.
    #[must_use]
    pub fn need(&self) -> &[EventId] {
        &self.need
    }

    /// Consume one `NEG-MSG` from the peer's relay, returning what comes next.
    ///
    /// The reply is the next round's frame, or the `REQ` for the accumulated
    /// `need` once the exchange terminates.
    pub fn step(&mut self, frame: &[u8]) -> Result<Vec<Message>> {
        let incoming = coracle_lib::sync::Message::decode(frame)
            .context("the peer's negentropy frame is malformed")?;
        let out = coracle_lib::sync::reconcile_initiator(&self.local, &incoming, self.budget);

        self.have.extend(out.have);
        self.need.extend(out.need);

        match out.next {
            Some(reply) => Ok(vec![Message::NegMsg(
                self.subscription.clone(),
                reply.encode(),
            )]),
            None => {
                let need = self.need.iter().copied().collect::<BTreeSet<_>>();
                let filter = self.filter.clone().add_ids(need);

                Ok(vec![Message::Req(self.subscription.clone(), vec![filter])])
            }
        }
    }
}

/// The `REQ` phase after a negotiation terminates: which of the negotiated
/// ids are still missing, fetched page by page.
pub struct Fetch {
    /// Ids this device still wants.
    pending: BTreeSet<EventId>,
    /// The ids in the most recent `REQ`, to tell a page that served nothing —
    /// the peer does not hold the rest — from one still in flight.
    last_requested: BTreeSet<EventId>,
}

impl Fetch {
    /// A fetch for every id the negotiation concluded with.
    fn new(need: BTreeSet<EventId>) -> Self {
        Self {
            pending: need.clone(),
            last_requested: need,
        }
    }
}

/// Handle a message this device's client half received.
pub fn handle(
    db: &Db,
    peer: &Peer,
    message: Message,
    quota: Quota,
    used: &mut Used,
    negotiations: &mut BTreeMap<SubscriptionId, Negotiation>,
    fetches: &mut BTreeMap<SubscriptionId, Fetch>,
) -> Result<Vec<Message>> {
    match message {
        Message::Event(subscription, event) => {
            if admits(peer, &event, None, quota, *used).is_ok() {
                ingest(db, peer, &event, None)?;
                used.record(&event);
            }
            // Whether stored or rejected, it is no longer wanted.
            if let Some(fetch) = fetches.get_mut(&subscription) {
                fetch.pending.remove(&event.id);
            }

            Ok(Vec::new())
        }
        Message::NegMsg(subscription, frame) => {
            let Some(negotiation) = negotiations.get_mut(&subscription) else {
                bail!("a NEG-MSG arrived for a subscription that is not open");
            };

            let replies = negotiation.step(&frame)?;

            // The terminating reply is a REQ on the same subscription: the
            // exchange is over and the fetch for what it wants begins.
            if matches!(replies.last(), Some(Message::Req(sub, _)) if *sub == subscription) {
                let need = negotiation.need().iter().copied().collect::<BTreeSet<_>>();

                negotiations.remove(&subscription);

                if !need.is_empty() {
                    fetches.insert(subscription.clone(), Fetch::new(need));
                }
            }

            Ok(replies)
        }
        Message::Eose(subscription) => {
            let Some(fetch) = fetches.get_mut(&subscription) else {
                return Ok(Vec::new());
            };

            // Done: everything this page asked for arrived.
            if fetch.pending.is_empty() {
                fetches.remove(&subscription);
                return Ok(Vec::new());
            }

            // A page that served nothing means the peer does not hold the
            // rest; asking again would not change that.
            if fetch.pending == fetch.last_requested {
                fetches.remove(&subscription);
                return Ok(Vec::new());
            }

            let ids = fetch.pending.clone();
            fetch.last_requested = ids.clone();

            Ok(vec![Message::Req(
                subscription.clone(),
                vec![Filter::new().add_ids(ids)],
            )])
        }
        Message::Ok(..) => Ok(Vec::new()),
        _ => Ok(Vec::new()),
    }
}

/// Whether an event a peer offered may be stored, and why not.
///
/// Two registers and nothing else. The peer authored it, in which case the
/// authenticated session is proof of authorship to this device and to nobody
/// else, or it holds a proof naming this device, in which case it is a
/// forwarder and the event goes no further. Scope and quota narrow what
/// survives that; neither widens it.
pub fn admits(
    peer: &Peer,
    event: &HashedEvent,
    proof: Option<&AuthorshipProof>,
    quota: Quota,
    used: Used,
) -> Result<(), Rejected> {
    if peer.is_blocked() {
        return Err(Rejected::Unauthorized);
    }

    if !peer.authored(event) && proof.is_none() {
        return Err(Rejected::Unauthorized);
    }

    if event_size(event) > MAX_EVENT_BYTES {
        return Err(Rejected::TooLarge);
    }

    if used.events >= quota.events || used.bytes + event_size(event) as u64 > quota.bytes {
        return Err(Rejected::OverQuota);
    }

    if !peer.may_store(event) {
        return Err(Rejected::OutOfScope);
    }

    Ok(())
}

/// Take in an event a peer offered, once [`admits`] has passed it.
///
/// The author's signature is stored only when this device is the recipient it
/// names, so the event travels one more hop and no further.
pub fn ingest(
    db: &Db,
    peer: &Peer,
    event: &HashedEvent,
    proof: Option<&AuthorshipProof>,
) -> Result<bool> {
    // A proof, when present, must be designated to this device and hold over
    // the event with one of the peer's own pubkeys as holder. When the peer
    // authored the event, the session is itself the proof and none is needed.
    if let Some(proof) = proof {
        let verified = peer.pubkeys().any(|holder| {
            proof.verifies(&AuthorshipClaim {
                event_id: event.id,
                author: event.pubkey,
                holder: *holder,
                verifier: peer.identity,
            })
        });

        if !verified {
            return Ok(false);
        }
    }

    let seen_from: Vec<coracle_lib::keys::PublicKey> = peer.pubkeys().copied().collect();

    command::receive_event(db, event, &seen_from, None, &peer.identity, clock::now())
}

#[cfg(test)]
mod tests {
    use super::*;
    use coracle_lib::keys::PublicKey;
    use coracle_lib::tags::Tags;

    use crate::db::command;
    use crate::db::query as db_query;
    use crate::fixtures::{author, note, secret};
    use crate::link::LinkId;
    use crate::model::{Policy, Query, RecipientSignature, Scope};

    /// This device, identifying as seed 1.
    fn us() -> PublicKey {
        author(1)
    }

    fn peer(pubkeys: impl IntoIterator<Item = PublicKey>) -> Peer {
        Peer::bind(LinkId(1), pubkeys, &Policy::new(us()))
    }

    fn note_from(n: u8, at: i64) -> HashedEvent {
        note(author(n), at, "content", Tags::new())
    }

    fn open(db: &Db, filter: Filter) -> Result<(Negotiation, Message)> {
        let peer = peer([author(2)]);

        Negotiation::begin(db, &peer, filter)
    }

    #[test]
    fn an_authored_event_is_admitted_without_a_proof() {
        let event = note_from(2, 100);
        let peer = peer([author(2)]);

        assert_eq!(
            admits(&peer, &event, None, Quota::STRANGER, Used::default()),
            Ok(())
        );
    }

    #[test]
    fn a_forwarded_event_needs_a_proof() {
        let event = note_from(3, 100);
        let peer = peer([author(2)]);

        assert_eq!(
            admits(&peer, &event, None, Quota::STRANGER, Used::default()),
            Err(Rejected::Unauthorized)
        );
    }

    #[test]
    fn a_blocked_peer_is_admitted_nothing() {
        let mut policy = Policy::new(us());
        policy.graph.blocked.insert(author(2));

        let peer = Peer::bind(LinkId(1), [author(2)], &policy);
        let event = note_from(2, 100);

        assert_eq!(
            admits(&peer, &event, None, Quota::STRANGER, Used::default()),
            Err(Rejected::Unauthorized)
        );
    }

    #[test]
    fn an_event_outside_accept_is_rejected() {
        let mut policy = Policy::new(us());
        policy.accept = Scope::Trusted;

        let peer = Peer::bind(LinkId(1), [author(2)], &policy);
        let event = note_from(2, 100);

        assert_eq!(
            admits(&peer, &event, None, Quota::STRANGER, Used::default()),
            Err(Rejected::OutOfScope)
        );
    }

    #[test]
    fn quota_is_metered_per_session() {
        let event = note_from(2, 100);
        let peer = peer([author(2)]);
        let full = Used {
            events: Quota::STRANGER.events,
            bytes: 0,
        };

        assert_eq!(
            admits(&peer, &event, None, Quota::STRANGER, full),
            Err(Rejected::OverQuota)
        );
    }

    #[test]
    fn an_oversized_event_is_rejected() {
        let event = note(
            author(2),
            100,
            &"x".repeat(MAX_EVENT_BYTES + 1),
            Tags::new(),
        );
        let peer = peer([author(2)]);

        assert_eq!(
            admits(&peer, &event, None, Quota::STRANGER, Used::default()),
            Err(Rejected::TooLarge)
        );
    }

    #[test]
    fn a_forwarded_event_with_a_valid_proof_is_stored() {
        let db = Db::open_in_memory().unwrap();
        let author_key = secret(3);
        let holder = author(2);
        let forwarded = note_from(3, 100);

        let signature = RecipientSignature::sign(&author_key, forwarded.id, holder);
        let proof = AuthorshipProof::prove(&signature, us()).unwrap();
        let peer = peer([holder]);

        assert!(ingest(&db, &peer, &forwarded, Some(&proof)).unwrap());

        let stored = db_query::list_events(&db, &Query::new()).unwrap();
        assert_eq!(stored, vec![forwarded]);
    }

    #[test]
    fn a_proof_for_the_wrong_holder_is_not_ingested() {
        let db = Db::open_in_memory().unwrap();
        let author_key = secret(3);
        let forwarded = note_from(3, 100);

        let signature = RecipientSignature::sign(&author_key, forwarded.id, author(2));
        let proof = AuthorshipProof::prove(&signature, us()).unwrap();
        // The peer proved a different pubkey than the signature names.
        let peer = peer([author(4)]);

        assert!(!ingest(&db, &peer, &forwarded, Some(&proof)).unwrap());
        assert!(
            db_query::list_events(&db, &Query::new())
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn a_proof_for_a_different_verifier_is_not_ingested() {
        let db = Db::open_in_memory().unwrap();
        let author_key = secret(3);
        let forwarded = note_from(3, 100);

        let signature = RecipientSignature::sign(&author_key, forwarded.id, author(2));
        // Designated to someone else, not this device.
        let proof = AuthorshipProof::prove(&signature, author(5)).unwrap();
        let peer = peer([author(2)]);

        assert!(!ingest(&db, &peer, &forwarded, Some(&proof)).unwrap());
    }

    #[test]
    fn a_negotiation_spells_out_what_is_missing() {
        // Bob has a note Alice does not; Alice has none herself.
        let alice = Db::open_in_memory().unwrap();
        let bob = Db::open_in_memory().unwrap();

        let bobs_note = note_from(3, 200);
        command::publish_event(&bob, &bobs_note, &author(3), 20).unwrap();

        // Alice begins diffing against Bob over everything.
        let (mut negotiation, opening) = open(&alice, Filter::new()).unwrap();
        let Message::NegOpen(_, _, frame) = opening else {
            panic!("expected a NEG-OPEN");
        };

        // Bob's relay half answers with its own set.
        let incoming = coracle_lib::sync::Message::decode(&frame).unwrap();
        let bob_items = SyncSet::from_items(vec![Item {
            timestamp: bobs_note.created_at,
            id: bobs_note.id,
        }]);
        let reply =
            coracle_lib::sync::reconcile_responder(&bob_items, &incoming, FrameBudget::unlimited());

        let out = negotiation.step(&reply.encode()).unwrap();

        match out.last() {
            Some(Message::Req(subscription, filters)) => {
                assert_eq!(subscription, negotiation.subscription_id());
                let ids = filters
                    .first()
                    .and_then(|filter| filter.ids.as_ref())
                    .map(|ids| ids.iter().copied().collect::<Vec<_>>())
                    .unwrap();
                assert_eq!(ids, vec![bobs_note.id]);
            }
            _ => panic!("expected the terminating REQ"),
        }
    }

    // ---------------------------------------------------------- pagination

    fn fetched(id: &HashedEvent) -> BTreeSet<EventId> {
        [id.id].into_iter().collect()
    }

    #[test]
    fn an_eose_with_ids_still_missing_requests_the_next_page() {
        let db = Db::open_in_memory().unwrap();
        let first = note_from(2, 100);
        let second = note_from(2, 200);

        let subscription = SubscriptionId("sub".into());
        let mut fetches = BTreeMap::from([(
            subscription.clone(),
            Fetch::new([first.id, second.id].into()),
        )]);

        // One of the two arrives.
        let replies = handle(
            &db,
            &peer([author(2)]),
            Message::Event(subscription.clone(), Box::new(first.clone())),
            Quota::STRANGER,
            &mut Used::default(),
            &mut BTreeMap::new(),
            &mut fetches,
        )
        .unwrap();
        assert!(replies.is_empty());

        // EOSE with one still missing asks again for just that one.
        let replies = handle(
            &db,
            &peer([author(2)]),
            Message::Eose(subscription.clone()),
            Quota::STRANGER,
            &mut Used::default(),
            &mut BTreeMap::new(),
            &mut fetches,
        )
        .unwrap();

        match replies.as_slice() {
            [Message::Req(sub, filters)] => {
                assert_eq!(sub, &subscription);
                let ids = filters
                    .first()
                    .and_then(|filter| filter.ids.as_ref())
                    .unwrap();
                assert_eq!(ids, &fetched(&second));
            }
            other => panic!("expected a REQ for the missing page, got {other:?}"),
        }
    }

    #[test]
    fn a_page_that_served_nothing_ends_the_fetch() {
        let db = Db::open_in_memory().unwrap();
        let missing = note_from(3, 300);

        let subscription = SubscriptionId("sub".into());
        let mut fetches = BTreeMap::from([(subscription.clone(), Fetch::new(fetched(&missing)))]);

        // EOSE arrives with nothing served: the peer does not hold the rest.
        let replies = handle(
            &db,
            &peer([author(2)]),
            Message::Eose(subscription.clone()),
            Quota::STRANGER,
            &mut Used::default(),
            &mut BTreeMap::new(),
            &mut fetches,
        )
        .unwrap();

        assert!(replies.is_empty());
        assert!(fetches.is_empty(), "the dead fetch should be dropped");
    }

    #[test]
    fn a_fetch_drains_when_every_requested_id_arrives() {
        let db = Db::open_in_memory().unwrap();
        let event = note_from(2, 100);

        let subscription = SubscriptionId("sub".into());
        let mut fetches = BTreeMap::from([(subscription.clone(), Fetch::new(fetched(&event)))]);

        handle(
            &db,
            &peer([author(2)]),
            Message::Event(subscription.clone(), Box::new(event.clone())),
            Quota::STRANGER,
            &mut Used::default(),
            &mut BTreeMap::new(),
            &mut fetches,
        )
        .unwrap();

        let replies = handle(
            &db,
            &peer([author(2)]),
            Message::Eose(subscription.clone()),
            Quota::STRANGER,
            &mut Used::default(),
            &mut BTreeMap::new(),
            &mut fetches,
        )
        .unwrap();

        assert!(replies.is_empty());
        assert!(fetches.is_empty(), "a completed fetch should be dropped");
    }
}
