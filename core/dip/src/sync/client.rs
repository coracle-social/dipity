//! This device asking a peer: the client half.
//!
//! Where authorization happens. Every inbound event is authorized by the
//! session or by a proof designated to this device, or it is dropped, and no
//! policy flag relaxes it. `docs/proofs.md#authorship-proofs`.

use std::collections::{BTreeMap, BTreeSet};

use anyhow::{Context, Result, bail};
use coracle_lib::events::{EventExtensionId, EventId, HashedEvent};
use coracle_lib::filters::Filter;
use coracle_lib::sync::{FrameBudget, SyncSet};

use crate::clock;
use crate::db::Db;
use crate::db::command;
use crate::db::query as db_query;
use crate::model::{AuthorshipClaim, AuthorshipProof, Identity, RecipientSignature, Standing};
use crate::session::Peer;
use crate::sync::spending::{SessionSpending, Spent, event_size};
use crate::sync::{Message, Quota, SubscriptionId};

/// The largest event the store accepts, whatever the peer's standing.
/// `docs/sync.md#quotas`.
pub const MAX_EVENT_BYTES: usize = 64 * 1024;

/// The cap on one negentropy frame, so a reply stays a few fragments rather
/// than a torrent; reconciliation resumes on the next round.
const NEG_FRAME_BYTES: usize = 4 * 1024;

/// How many forwarded events may wait on their proofs at once.
///
/// An event arriving without its proof is unauthorized until one lands, so
/// holding it is a write a peer gets for free. The cap bounds that write:
/// past it the oldest waiting event is dropped, and if the proof ever arrives
/// the event comes back on the next encounter.
const MAX_PENDING: usize = 64;

/// Why an event this device was offered was not stored.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Rejected {
    /// The id is not the hash of the event, so nothing that commits to the id
    /// says anything about these bytes.
    Forged,
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
    pub subscription: SubscriptionId,
    /// The filter the exchange is bounded by.
    filter: Filter,
    /// This device's side of the diff, under the filter and the peer's
    /// registers.
    local: SyncSet,
    /// Ids this device has that the peer lacks. Not yet on the wire.
    have: Vec<EventId>,
    /// Ids the peer has that this device lacks, accumulated every round.
    /// What the terminating `REQ` asks for, once reconciliation completes.
    pub need: Vec<EventId>,
    /// The reply budget for the next message.
    budget: FrameBudget,
}

impl Negotiation {
    /// Begin diffing against `filter`, returning the opening frame to send.
    ///
    /// The local set is everything this device **holds** under the filter, not
    /// what it would serve this peer. The question a reconciliation answers is
    /// what this device is missing, so an event it already holds has to be in
    /// the set whether or not this peer could be offered it — otherwise every
    /// second-hop event, which sits in the `Held` register and is offerable to
    /// nobody, is reported missing on every encounter, re-fetched, and
    /// re-dropped as a duplicate. Reconciliation would never converge, and the
    /// peer would spend its quota redelivering what this device already has.
    ///
    /// Ids this device refused join the set for the same reason: a deleted
    /// event, a superseded version or an author outside Accept is never stored,
    /// so without them it too would be fetched and dropped on every encounter.
    ///
    /// The relay half bounds its own answers separately, in
    /// [`reconcilable`](crate::sync::relay::reconcilable), so nothing here
    /// widens what this device will serve.
    pub fn begin(db: &Db, filter: Filter) -> Result<(Self, Message)> {
        let held = crate::model::Query::new().with_filter(filter.clone());
        let local = db_query::initiator_set(db, &held)?;

        let subscription = fresh_subscription()?;
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

/// A fresh subscription id, distinct across sessions and peers.
fn fresh_subscription() -> Result<SubscriptionId> {
    let mut id = [0u8; 8];
    getrandom::getrandom(&mut id).context("generating a subscription id")?;

    Ok(SubscriptionId(hex::encode(id)))
}

/// The client half's state, held by the session for the life of the link.
#[derive(Default)]
pub struct Client {
    /// Open negotiations against the peer's relay half.
    pub negotiations: BTreeMap<SubscriptionId, Negotiation>,
    /// Fetches in the `REQ` phase, pulling the ids a finished negotiation
    /// found missing.
    fetches: BTreeMap<SubscriptionId, Fetch>,
    /// The standing subscription for what the peer writes from now on, once
    /// reconciliation has settled what both sides already held.
    live: Option<SubscriptionId>,
    /// Events the peer forwarded that are waiting on their authorship proof,
    /// with the subscription each arrived on. A proof follows its event on the
    /// same subscription, so one still waiting at that subscription's `EOSE`
    /// is not coming.
    pending_events: BTreeMap<EventId, (SubscriptionId, HashedEvent)>,
    /// Authorship proofs waiting on the event they prove.
    pending_proofs: BTreeMap<EventId, AuthorshipProof>,
}

impl Client {
    /// Register a negotiation this device has opened, keyed by its own
    /// subscription id.
    pub fn open_negotiation(&mut self, negotiation: Negotiation) {
        self.negotiations
            .insert(negotiation.subscription.clone(), negotiation);
    }

    /// Whether this device is still waiting on the peer for something it asked
    /// for: a reconciliation round, or a page of a fetch, which includes the
    /// proofs for the events that page has already delivered.
    ///
    /// An event held on the standing subscription has no `EOSE` to end the
    /// wait, so it does not hold a drain open; it is bounded by count instead.
    #[must_use]
    pub fn is_awaiting(&self) -> bool {
        !self.negotiations.is_empty() || !self.fetches.is_empty()
    }

    /// Ask the peer for what it writes from here on, once per session.
    ///
    /// Reconciliation settles what the two already hold; without a standing
    /// subscription a note written while both are still in range would wait
    /// for the next encounter, which is the one thing being in range is for.
    /// `docs/storage.md` — a write is gossiped as it happens.
    fn open_live(&mut self) -> Result<Option<Message>> {
        if self.live.is_some() {
            return Ok(None);
        }

        let subscription = fresh_subscription()?;
        self.live = Some(subscription.clone());

        Ok(Some(Message::Req(
            subscription,
            vec![Filter::new().add_since(clock::now())],
        )))
    }

    /// Handle a message this device's client half received.
    pub fn handle(
        &mut self,
        db: &Db,
        peer: &Peer,
        local: &Identity,
        message: Message,
        quota: Quota,
        spending: &mut SessionSpending,
    ) -> Result<Vec<Message>> {
        // The quota is tested against the rolling window, so a reconnect refills nothing.
        let spent = spending.spent(peer);

        // Store what passes, and remember what Accept refused so reconciliation stops offering it.
        let mut take = |event: &HashedEvent, proof: Option<&AuthorshipProof>| -> Result<()> {
            match admits(peer, event, proof, quota, spent) {
                // Only what was stored is charged: a duplicate or a proof that failed cost nothing.
                Ok(()) => {
                    if ingest(db, peer, local, event, proof)? {
                        spending.record(peer, event);
                    }
                }
                Err(Rejected::OutOfScope) => command::refuse_by_policy(db, event, clock::now())?,
                Err(_) => {}
            }

            Ok(())
        };

        match message {
            Message::Event(subscription, event) => {
                let id = event.id;
                let proof = self.pending_proofs.remove(&id);

                if let Some(proof) = &proof {
                    // Second hop: the proof authorizes the forwarded event.
                    take(&event, Some(proof))?;
                } else if peer.pubkeys.contains(&event.pubkey) {
                    // First hop: the authenticated session is the proof.
                    take(&event, None)?;
                } else if holdable(peer, &event, quota, spent) {
                    // Forwarded without its proof yet; hold it until one arrives.
                    self.pending_events
                        .insert(id, (subscription.clone(), *event));
                    bound(&mut self.pending_events);
                }

                // Whether stored, held or rejected, it is no longer wanted.
                if let Some(fetch) = self.fetches.get_mut(&subscription) {
                    fetch.pending.remove(&id);
                }

                Ok(Vec::new())
            }
            Message::RecipientSignature(_, event_id, sig) => {
                store_signature(db, peer, local, event_id, &sig);

                Ok(Vec::new())
            }
            Message::AuthorshipProof(_, event_id, proof) => {
                let proof = AuthorshipProof::from_bytes(&proof);

                match self.pending_events.remove(&event_id) {
                    Some((_, event)) => {
                        take(&event, Some(&proof))?;
                    }
                    None => {
                        self.pending_proofs.insert(event_id, proof);
                        bound(&mut self.pending_proofs);
                    }
                }

                Ok(Vec::new())
            }
            // The peer could not run the diff, which ends it and nothing else.
            Message::NegErr(subscription, _) => {
                self.negotiations.remove(&subscription);

                Ok(Vec::new())
            }
            Message::NegMsg(subscription, frame) => {
                let Some(negotiation) = self.negotiations.get_mut(&subscription) else {
                    bail!("a NEG-MSG arrived for a subscription that is not open");
                };

                let mut replies = negotiation.step(&frame)?;

                // The terminating reply is a REQ on the same subscription: the exchange is over.
                let terminated =
                    matches!(replies.last(), Some(Message::Req(sub, _)) if *sub == subscription);
                let need: BTreeSet<EventId> = if terminated {
                    negotiation.need.iter().copied().collect()
                } else {
                    BTreeSet::new()
                };

                if terminated {
                    self.negotiations.remove(&subscription);

                    if need.is_empty() {
                        // An empty id set matches nothing and is not worth a round trip.
                        replies.pop();
                    } else {
                        self.fetches.insert(subscription.clone(), Fetch::new(need));
                    }

                    replies.extend(self.open_live()?);
                }

                Ok(replies)
            }
            Message::Eose(subscription) => {
                // Every proof this subscription was going to carry has arrived by now.
                self.pending_events
                    .retain(|_, (held_on, _)| *held_on != subscription);

                let Some(fetch) = self.fetches.get_mut(&subscription) else {
                    return Ok(Vec::new());
                };

                // Done: everything this page asked for arrived.
                if fetch.pending.is_empty() {
                    self.fetches.remove(&subscription);
                    return Ok(Vec::new());
                }

                // A page that served nothing means the peer does not hold the rest.
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

/// Whether a forwarded event is worth holding while its proof is outstanding.
///
/// [`admits`] cannot run yet — the proof is the missing half — but everything
/// it tests that a proof could not change runs now, so a peer cannot make this
/// device buffer what it could never store. Only the two-register
/// authorization is deferred.
fn holdable(peer: &Peer, event: &HashedEvent, quota: Quota, spent: Spent) -> bool {
    admissible(peer, event, quota, spent).is_ok()
}

/// Drop the oldest entries until a pending map is back under [`MAX_PENDING`].
///
/// Oldest by event id rather than by arrival, which is arbitrary but bounded:
/// what matters is that a peer cannot grow the map without limit, and anything
/// evicted returns on the next encounter.
fn bound<T>(pending: &mut BTreeMap<EventId, T>) {
    while pending.len() > MAX_PENDING {
        let Some(oldest) = pending.keys().next().copied() else {
            return;
        };

        pending.remove(&oldest);
    }
}

/// Store the author's signature over an event, once it verifies against the
/// stored event and names this device.
fn store_signature(db: &Db, peer: &Peer, local: &Identity, event_id: EventId, sig: &[u8; 64]) {
    let Ok(Some(event)) = db_query::get_event(db, &event_id) else {
        return;
    };

    // The signature is the author's, so only the author sends it.
    if !peer.pubkeys.contains(&event.pubkey) {
        return;
    }

    // Nothing on the wire says which identity the author named, so verification answers.
    let named = local.iter().find_map(|identity| {
        let signature = RecipientSignature {
            event_id,
            author_pubkey: event.pubkey,
            recipient_pubkey: *identity,
            sig: *sig,
        };

        signature.verifies().then_some(*identity)
    });

    if let Some(identity) = named {
        let _ = command::receive_signature(db, &event_id, sig, &identity);
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
    spent: Spent,
) -> Result<(), Rejected> {
    admissible(peer, event, quota, spent)?;

    // The two registers, and nothing else widens them.
    if !peer.pubkeys.contains(&event.pubkey) && proof.is_none() {
        return Err(Rejected::Unauthorized);
    }

    Ok(())
}

/// Everything that disqualifies an event whatever authorizes it.
///
/// Split out from [`admits`] so a forwarded event waiting on its proof can be
/// tested against all of it before this device agrees to hold the event at
/// all. Authorization is the one thing a later proof can change; integrity,
/// standing, size, quota and scope are not.
fn admissible(
    peer: &Peer,
    event: &HashedEvent,
    quota: Quota,
    spent: Spent,
) -> Result<(), Rejected> {
    // Both registers authorize an id, not a body, so an id that is not its own hash carries none.
    if !event.verify_id() {
        return Err(Rejected::Forged);
    }

    if peer.policy.is_blocked() {
        return Err(Rejected::Unauthorized);
    }

    if event_size(event) > MAX_EVENT_BYTES {
        return Err(Rejected::TooLarge);
    }

    let size = event_size(event) as u64;

    if spent.events >= quota.events || spent.bytes + size > quota.bytes {
        return Err(Rejected::OverQuota);
    }

    // A stranger is bounded twice; the pool is what a fresh keypair cannot reset.
    if peer.policy.standing != Standing::Trusted
        && (spent.pooled_events >= Quota::STRANGER_POOL.events
            || spent.pooled_bytes + size > Quota::STRANGER_POOL.bytes)
    {
        return Err(Rejected::OverQuota);
    }

    if !peer.policy.should_accept(event) {
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
    local: &Identity,
    event: &HashedEvent,
    proof: Option<&AuthorshipProof>,
) -> Result<bool> {
    // A proof must be designated to this device; an author's own session needs none.
    if let Some(proof) = proof {
        // Neither holder nor verifier is named on the wire, so the pair it verifies under is it.
        let verified = peer.pubkeys.iter().any(|holder| {
            local.iter().any(|verifier| {
                proof.verifies(&AuthorshipClaim {
                    event_id: event.id,
                    author: event.pubkey,
                    holder: *holder,
                    verifier: *verifier,
                })
            })
        });

        if !verified {
            return Ok(false);
        }
    }

    let seen_from: Vec<coracle_lib::keys::PublicKey> = peer.pubkeys.iter().copied().collect();

    command::receive_event(db, event, &seen_from, clock::now())
}

#[cfg(test)]
mod tests {
    use super::*;
    use coracle_lib::keys::PublicKey;
    use coracle_lib::sync::Item;
    use coracle_lib::tags::Tags;

    use std::sync::Arc;

    use crate::db::command;
    use crate::db::query as db_query;
    use crate::fixtures::{author, note, secret};
    use crate::link::LinkId;
    use crate::model::{Policy, Query, RecipientSignature, Scope};
    use crate::sync::spending::SpendingLedger;

    /// This device, identifying as seed 1.
    fn us() -> PublicKey {
        author(1)
    }

    fn peer(pubkeys: impl IntoIterator<Item = PublicKey>) -> Peer {
        Peer::bind(LinkId(1), pubkeys, &Policy::new(us()))
    }

    /// This device's identity in these tests.
    fn local() -> Identity {
        Identity::from([us()])
    }

    fn note_from(n: u8, at: i64) -> HashedEvent {
        note(author(n), at, "content", Tags::new())
    }

    fn open(db: &Db, filter: Filter) -> Result<(Negotiation, Message)> {
        Negotiation::begin(db, filter)
    }

    fn spending() -> SessionSpending {
        SessionSpending::new(Arc::new(SpendingLedger::default()))
    }

    #[test]
    fn an_authored_event_is_admitted_without_a_proof() {
        let event = note_from(2, 100);
        let peer = peer([author(2)]);

        assert_eq!(
            admits(&peer, &event, None, Quota::STRANGER, Spent::default()),
            Ok(())
        );
    }

    #[test]
    fn a_forwarded_event_needs_a_proof() {
        let event = note_from(3, 100);
        let peer = peer([author(2)]);

        assert_eq!(
            admits(&peer, &event, None, Quota::STRANGER, Spent::default()),
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
            admits(&peer, &event, None, Quota::STRANGER, Spent::default()),
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
            admits(&peer, &event, None, Quota::STRANGER, Spent::default()),
            Err(Rejected::OutOfScope)
        );
    }

    #[test]
    fn an_event_outside_accept_counts_as_held_until_the_policy_changes() {
        let db = Db::open_in_memory().unwrap();
        let mut policy = Policy::new(us());
        policy.accept = Scope::Trusted;

        let peer = Peer::bind(LinkId(1), [author(2)], &policy);
        let event = note_from(2, 100);

        client(BTreeMap::new())
            .handle(
                &db,
                &peer,
                &local(),
                Message::Event(SubscriptionId("sub".into()), Box::new(event.clone())),
                Quota::STRANGER,
                &mut spending(),
            )
            .unwrap();

        let opened = |db: &Db| {
            let set = db_query::initiator_set(db, &Query::new()).unwrap();
            set.iter().map(|item| item.id).collect::<Vec<_>>()
        };

        assert!(
            db_query::list_events(&db, &Query::new())
                .unwrap()
                .is_empty()
        );
        assert_eq!(opened(&db), vec![event.id]);

        command::forget_policy_refusals(&db).unwrap();
        assert!(opened(&db).is_empty());
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
                Spent {
                    events: Quota::STRANGER.events,
                    ..Spent::default()
                },
            ),
            Err(Rejected::OverQuota)
        );
    }

    #[test]
    fn the_rolling_window_counts_across_sessions() {
        let ledger = Arc::new(SpendingLedger::default());

        // One event admitted now, one just inside the window.
        clock::at(1_000, || {
            ledger.record(&author(2), false, 10);
        });
        clock::at(2_000, || {
            ledger.record(&author(2), false, 20);

            // Both are inside the window at one instant past the first.
            let spending = SessionSpending::new(Arc::clone(&ledger));
            assert_eq!(spending.spent(&peer([author(2)])).events, 2);
        });

        // …and the first has fallen out just past the window.
        clock::at(1_000 + crate::sync::spending::WINDOW_SECONDS + 1, || {
            let spending = SessionSpending::new(Arc::clone(&ledger));
            let spent = spending.spent(&peer([author(2)]));
            assert_eq!(spent.events, 1, "the stale event fell out of the window");
            assert!(spent.bytes > 0);
        });
    }

    #[test]
    fn a_reconnect_cannot_refill_a_spent_budget() {
        let ledger = Arc::new(SpendingLedger::default());

        clock::at(1_000, || {
            // A past session already spent the stranger budget in the window.
            for _ in 0..Quota::STRANGER.events {
                ledger.record(&author(2), false, 1);
            }

            // A fresh session sees the same spent budget and is over quota.
            let spending = SessionSpending::new(Arc::clone(&ledger));
            let spent = spending.spent(&peer([author(2)]));

            assert_eq!(
                admits(
                    &peer([author(2)]),
                    &note_from(2, 200),
                    None,
                    Quota::STRANGER,
                    spent,
                ),
                Err(Rejected::OverQuota)
            );
        });
    }

    #[test]
    fn an_event_delivered_twice_is_charged_once() {
        let db = Db::open_in_memory().unwrap();
        let ledger = Arc::new(SpendingLedger::default());
        let mut spending = SessionSpending::new(Arc::clone(&ledger));
        let event = note_from(2, 100);

        for _ in 0..2 {
            client(BTreeMap::new())
                .handle(
                    &db,
                    &peer([author(2)]),
                    &local(),
                    Message::Event(SubscriptionId("sub".into()), Box::new(event.clone())),
                    Quota::STRANGER,
                    &mut spending,
                )
                .unwrap();
        }

        assert_eq!(spending.spent(&peer([author(2)])).events, 1);
    }

    #[test]
    fn the_stranger_pool_refuses_a_peer_whose_own_budget_is_untouched() {
        // A burner arrives with a clean per-peer meter and is still refused.
        let event = note_from(2, 100);
        let peer = peer([author(2)]);

        let spent = Spent {
            events: 0,
            bytes: 0,
            pooled_events: Quota::STRANGER_POOL.events,
            pooled_bytes: 0,
        };

        assert_eq!(
            admits(&peer, &event, None, Quota::STRANGER, spent),
            Err(Rejected::OverQuota)
        );
    }

    #[test]
    fn a_trusted_peer_passes_a_spent_stranger_pool() {
        // "A hard ceiling that cannot crowd out known peers": a full pool is theirs, not ours.
        let mut policy = Policy::new(us());
        policy.graph.trusted.insert(author(2));
        let peer = Peer::bind(LinkId(1), [author(2)], &policy);

        let spent = Spent {
            pooled_events: Quota::STRANGER_POOL.events,
            pooled_bytes: Quota::STRANGER_POOL.bytes,
            ..Spent::default()
        };

        assert_eq!(
            admits(&peer, &note_from(2, 100), None, Quota::TRUSTED, spent),
            Ok(())
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
            admits(&peer, &event, None, Quota::STRANGER, Spent::default()),
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

        assert!(ingest(&db, &peer, &local(), &forwarded, Some(&proof)).unwrap());

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

        assert!(!ingest(&db, &peer, &local(), &forwarded, Some(&proof)).unwrap());
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

        assert!(!ingest(&db, &peer, &local(), &forwarded, Some(&proof)).unwrap());
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
                assert_eq!(subscription, &negotiation.subscription);
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
                &local(),
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
                &local(),
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
                &local(),
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
                &local(),
                Message::Event(subscription.clone(), Box::new(event.clone())),
                Quota::STRANGER,
                &mut spending(),
            )
            .unwrap();

        let replies = client
            .handle(
                &db,
                &peer([author(2)]),
                &local(),
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
                &local(),
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
                &local(),
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
        let subscription = SubscriptionId("sub".into());

        let mut client = Client::default();

        for message in [
            Message::Event(subscription.clone(), Box::new(forwarded.clone())),
            Message::Eose(subscription.clone()),
        ] {
            client
                .handle(
                    &db,
                    &peer([author(2)]),
                    &local(),
                    message,
                    Quota::STRANGER,
                    &mut spending(),
                )
                .unwrap();
        }

        // No proof came before the EOSE, so none is coming and nothing waits on one.
        assert!(!client.pending_events.contains_key(&forwarded.id));
        assert!(!client.is_awaiting());
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
                &local(),
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
