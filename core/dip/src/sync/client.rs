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
use crate::model::{AuthorshipClaim, AuthorshipProof, RecipientSignature};
use crate::session::Peer;
use crate::spending::{SessionSpending, event_size};
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
    /// The peer has written as much as it may within the rolling window.
    OverQuota,
    /// Larger than the per-event cap.
    TooLarge,
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

/// The client half's state, held by the session for the life of the link.
#[derive(Default)]
pub struct Client {
    /// Open negotiations against the peer's relay half.
    negotiations: BTreeMap<SubscriptionId, Negotiation>,
    /// Fetches in the `REQ` phase, pulling the ids a finished negotiation
    /// found missing.
    fetches: BTreeMap<SubscriptionId, Fetch>,
    /// Events the peer forwarded that are waiting on their authorship proof.
    pending_events: BTreeMap<EventId, HashedEvent>,
    /// Authorship proofs waiting on the event they prove.
    pending_proofs: BTreeMap<EventId, AuthorshipProof>,
}

impl Client {
    /// Register a negotiation this device has opened, keyed by its own
    /// subscription id.
    pub fn open_negotiation(&mut self, negotiation: Negotiation) {
        self.negotiations
            .insert(negotiation.subscription_id().clone(), negotiation);
    }

    /// Whether this device opened the named subscription, which is how a
    /// `NEG-MSG` is routed to the right half.
    #[must_use]
    pub fn owns_subscription(&self, subscription: &SubscriptionId) -> bool {
        self.negotiations.contains_key(subscription)
    }

    /// Handle a message this device's client half received.
    pub fn handle(
        &mut self,
        db: &Db,
        peer: &Peer,
        message: Message,
        quota: Quota,
        spending: &mut SessionSpending,
    ) -> Result<Vec<Message>> {
        // The quota is tested against the rolling window, which this
        // session's own accepts land in too, so a reconnect cannot refill
        // what a drive-by already spent.
        let (events, bytes) = spending.total(peer);

        match message {
            Message::Event(subscription, event) => {
                let id = event.id;
                let proof = self.pending_proofs.remove(&id);

                if let Some(proof) = &proof {
                    // Second hop: the proof authorizes the forwarded event.
                    if admits(peer, &event, Some(proof), quota, events, bytes).is_ok() {
                        ingest(db, peer, &event, Some(proof))?;
                        spending.record(peer, &event);
                    }
                } else if peer.authored(event.as_ref()) {
                    // First hop: the authenticated session is the proof.
                    if admits(peer, &event, None, quota, events, bytes).is_ok() {
                        ingest(db, peer, &event, None)?;
                        spending.record(peer, &event);
                    }
                } else {
                    // Forwarded without its proof yet; hold it until one
                    // arrives.
                    self.pending_events.insert(id, *event);
                }

                // Whether stored, held or rejected, it is no longer wanted.
                if let Some(fetch) = self.fetches.get_mut(&subscription) {
                    fetch.pending.remove(&id);
                }

                Ok(Vec::new())
            }
            Message::RecipientSignature(_, event_id, sig) => {
                store_signature(db, peer, event_id, &sig);

                Ok(Vec::new())
            }
            Message::AuthorshipProof(_, event_id, proof) => {
                let proof = AuthorshipProof::from_bytes(&proof);

                match self.pending_events.remove(&event_id) {
                    Some(event) => {
                        if admits(peer, &event, Some(&proof), quota, events, bytes).is_ok() {
                            ingest(db, peer, &event, Some(&proof))?;
                            spending.record(peer, &event);
                        }
                    }
                    None => {
                        self.pending_proofs.insert(event_id, proof);
                    }
                }

                Ok(Vec::new())
            }
            Message::NegMsg(subscription, frame) => {
                let Some(negotiation) = self.negotiations.get_mut(&subscription) else {
                    bail!("a NEG-MSG arrived for a subscription that is not open");
                };

                let replies = negotiation.step(&frame)?;

                // The terminating reply is a REQ on the same subscription: the
                // exchange is over and the fetch for what it wants begins.
                if matches!(replies.last(), Some(Message::Req(sub, _)) if *sub == subscription) {
                    let need = negotiation.need().iter().copied().collect::<BTreeSet<_>>();

                    self.negotiations.remove(&subscription);

                    if !need.is_empty() {
                        self.fetches.insert(subscription.clone(), Fetch::new(need));
                    }
                }

                Ok(replies)
            }
            Message::Eose(subscription) => {
                let Some(fetch) = self.fetches.get_mut(&subscription) else {
                    return Ok(Vec::new());
                };

                // Done: everything this page asked for arrived.
                if fetch.pending.is_empty() {
                    self.fetches.remove(&subscription);
                    return Ok(Vec::new());
                }

                // A page that served nothing means the peer does not hold the
                // rest; asking again would not change that.
                if fetch.pending == fetch.last_requested {
                    self.fetches.remove(&subscription);
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
}

/// Store the author's signature over an event, once it verifies against the
/// stored event and names this device.
fn store_signature(db: &Db, peer: &Peer, event_id: EventId, sig: &[u8; 64]) {
    let Ok(Some(event)) = db_query::get_event(db, &event_id) else {
        return;
    };

    // The signature is the author's, so only the author sends it.
    if !peer.authored(&event) {
        return;
    }

    let signature = RecipientSignature {
        event_id,
        author_pubkey: event.pubkey,
        recipient_pubkey: peer.identity,
        sig: *sig,
    };

    if signature.verifies() {
        let _ = command::receive_signature(db, &event_id, sig, &peer.identity);
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
    events: u32,
    bytes: u64,
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

    if events >= quota.events || bytes + event_size(event) as u64 > quota.bytes {
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

    use std::sync::Arc;

    use crate::db::command;
    use crate::db::query as db_query;
    use crate::fixtures::{author, note, secret};
    use crate::link::LinkId;
    use crate::model::{Policy, Query, RecipientSignature, Scope};
    use crate::spending::SpendingLedger;

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

    fn spending() -> SessionSpending {
        SessionSpending::new(Arc::new(SpendingLedger::default()))
    }

    #[test]
    fn an_authored_event_is_admitted_without_a_proof() {
        let event = note_from(2, 100);
        let peer = peer([author(2)]);

        assert_eq!(admits(&peer, &event, None, Quota::STRANGER, 0, 0), Ok(()));
    }

    #[test]
    fn a_forwarded_event_needs_a_proof() {
        let event = note_from(3, 100);
        let peer = peer([author(2)]);

        assert_eq!(
            admits(&peer, &event, None, Quota::STRANGER, 0, 0),
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
            admits(&peer, &event, None, Quota::STRANGER, 0, 0),
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
            admits(&peer, &event, None, Quota::STRANGER, 0, 0),
            Err(Rejected::OutOfScope)
        );
    }

    #[test]
    fn a_spent_event_budget_refuses_the_next_event() {
        let event = note_from(2, 100);
        let peer = peer([author(2)]);

        assert_eq!(
            admits(
                &peer,
                &event,
                None,
                Quota::STRANGER,
                Quota::STRANGER.events,
                0
            ),
            Err(Rejected::OverQuota)
        );
    }

    #[test]
    fn the_rolling_window_counts_across_sessions() {
        let ledger = Arc::new(SpendingLedger::default());

        // One event admitted now, one just inside the window.
        clock::at(1_000, || {
            ledger.record(&author(2), 10);
        });
        clock::at(2_000, || {
            ledger.record(&author(2), 20);

            // Both are inside the window at one instant past the first.
            let spending = SessionSpending::new(Arc::clone(&ledger));
            let (events, _) = spending.total(&peer([author(2)]));
            assert_eq!(events, 2);
        });

        // …and the first has fallen out just past the window.
        clock::at(1_000 + crate::spending::WINDOW_SECONDS + 1, || {
            let spending = SessionSpending::new(Arc::clone(&ledger));
            let (events, bytes) = spending.total(&peer([author(2)]));
            assert_eq!(events, 1, "the stale event fell out of the window");
            assert!(bytes > 0);
        });
    }

    #[test]
    fn a_reconnect_cannot_refill_a_spent_budget() {
        let ledger = Arc::new(SpendingLedger::default());

        clock::at(1_000, || {
            // A past session already spent the stranger budget in the window.
            for _ in 0..Quota::STRANGER.events {
                ledger.record(&author(2), 1);
            }

            // A fresh session sees the same spent budget and is over quota.
            let spending = SessionSpending::new(Arc::clone(&ledger));
            let (events, bytes) = spending.total(&peer([author(2)]));

            assert_eq!(
                admits(
                    &peer([author(2)]),
                    &note_from(2, 200),
                    None,
                    Quota::STRANGER,
                    events,
                    bytes,
                ),
                Err(Rejected::OverQuota)
            );
        });
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
            admits(&peer, &event, None, Quota::STRANGER, 0, 0),
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

    /// A client with only `fetches` populated, for the pagination tests.
    fn client(fetches: BTreeMap<SubscriptionId, Fetch>) -> Client {
        Client {
            fetches,
            ..Client::default()
        }
    }

    #[test]
    fn an_eose_with_ids_still_missing_requests_the_next_page() {
        let db = Db::open_in_memory().unwrap();
        let first = note_from(2, 100);
        let second = note_from(2, 200);

        let subscription = SubscriptionId("sub".into());
        let mut client = client(BTreeMap::from([(
            subscription.clone(),
            Fetch::new([first.id, second.id].into()),
        )]));

        // One of the two arrives.
        let replies = client
            .handle(
                &db,
                &peer([author(2)]),
                Message::Event(subscription.clone(), Box::new(first.clone())),
                Quota::STRANGER,
                &mut spending(),
            )
            .unwrap();
        assert!(replies.is_empty());

        // EOSE with one still missing asks again for just that one.
        let replies = client
            .handle(
                &db,
                &peer([author(2)]),
                Message::Eose(subscription.clone()),
                Quota::STRANGER,
                &mut spending(),
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
        let mut client = client(BTreeMap::from([(
            subscription.clone(),
            Fetch::new(fetched(&missing)),
        )]));

        // EOSE arrives with nothing served: the peer does not hold the rest.
        let replies = client
            .handle(
                &db,
                &peer([author(2)]),
                Message::Eose(subscription.clone()),
                Quota::STRANGER,
                &mut spending(),
            )
            .unwrap();

        assert!(replies.is_empty());
        assert!(
            client.fetches.is_empty(),
            "the dead fetch should be dropped"
        );
    }

    #[test]
    fn a_fetch_drains_when_every_requested_id_arrives() {
        let db = Db::open_in_memory().unwrap();
        let event = note_from(2, 100);

        let subscription = SubscriptionId("sub".into());
        let mut client = client(BTreeMap::from([(
            subscription.clone(),
            Fetch::new(fetched(&event)),
        )]));

        client
            .handle(
                &db,
                &peer([author(2)]),
                Message::Event(subscription.clone(), Box::new(event.clone())),
                Quota::STRANGER,
                &mut spending(),
            )
            .unwrap();

        let replies = client
            .handle(
                &db,
                &peer([author(2)]),
                Message::Eose(subscription.clone()),
                Quota::STRANGER,
                &mut spending(),
            )
            .unwrap();

        assert!(replies.is_empty());
        assert!(
            client.fetches.is_empty(),
            "a completed fetch should be dropped"
        );
    }

    // ------------------------------------------------------ the second hop

    #[test]
    fn a_forwarded_event_is_held_until_its_proof_arrives() {
        let db = Db::open_in_memory().unwrap();
        let forwarded = note_from(3, 100);
        let subscription = SubscriptionId("sub".into());

        let mut client = Client::default();

        // The event arrives without a proof and is held.
        let replies = client
            .handle(
                &db,
                &peer([author(2)]),
                Message::Event(subscription.clone(), Box::new(forwarded.clone())),
                Quota::STRANGER,
                &mut spending(),
            )
            .unwrap();

        assert!(replies.is_empty());
        assert!(client.pending_events.contains_key(&forwarded.id));
        assert!(
            db_query::list_events(&db, &Query::new())
                .unwrap()
                .is_empty(),
            "nothing stored before its proof"
        );

        // The proof authorizes it: B holds the author's signature naming B.
        let signature = RecipientSignature::sign(&secret(3), forwarded.id, author(2));
        let proof = AuthorshipProof::prove(&signature, us()).unwrap();

        client
            .handle(
                &db,
                &peer([author(2)]),
                Message::AuthorshipProof(subscription, forwarded.id, Box::new(proof.to_bytes())),
                Quota::STRANGER,
                &mut spending(),
            )
            .unwrap();

        assert_eq!(
            db_query::list_events(&db, &Query::new()).unwrap(),
            vec![forwarded]
        );
    }

    #[test]
    fn an_unproven_forwarded_event_is_dropped_at_eose() {
        let db = Db::open_in_memory().unwrap();
        let forwarded = note_from(3, 100);

        let mut client = Client::default();

        client
            .handle(
                &db,
                &peer([author(2)]),
                Message::Event(SubscriptionId("sub".into()), Box::new(forwarded)),
                Quota::STRANGER,
                &mut spending(),
            )
            .unwrap();

        // No proof ever comes, so the held event is simply not stored.
        assert!(
            db_query::list_events(&db, &Query::new())
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn a_recipient_signature_is_verified_and_stored() {
        let db = Db::open_in_memory().unwrap();
        let event = note_from(2, 100);
        command::publish_event(&db, &event, &author(2), 100).unwrap();

        let signature = RecipientSignature::sign(&secret(2), event.id, us());

        Client::default()
            .handle(
                &db,
                &peer([author(2)]),
                Message::RecipientSignature(
                    SubscriptionId("sub".into()),
                    event.id,
                    Box::new(signature.sig),
                ),
                Quota::STRANGER,
                &mut spending(),
            )
            .unwrap();

        assert!(
            db_query::get_signature(&db, &event.id, &us())
                .unwrap()
                .is_some()
        );
    }
}
