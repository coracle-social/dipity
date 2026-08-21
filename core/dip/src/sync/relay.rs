//! This device answering a peer: the relay half.
//!
//! Everything it has to apply is expressible as a [`Query`]. A peer's `REQ`
//! becomes a filter, the user's Gossip scope and visibility become a
//! [`PubkeyPolicy`](crate::model::PubkeyPolicy), and the two authorship registers
//! that may travel become [`Registers::offerable`]. `db::query::list_events`
//! takes all three, so there is no second place where what a peer may see is
//! decided.

use std::collections::BTreeMap;

use anyhow::{Context, Result, bail};
use coracle_lib::events::HashedEvent;
use coracle_lib::filters::Filter;
use coracle_lib::keys::SecretKey;
use coracle_lib::sync::{FrameBudget, Item, SyncSet};

use crate::db::Db;
use crate::db::query as db_query;
use crate::model::{AuthorshipProof, Query, RecipientSignature, Registers};
use crate::session::Peer;
use crate::spending::SessionSpending;
use crate::sync::client::{self, Rejected};
use crate::sync::{Message, Quota, SubscriptionId};

/// A page of events served on one subscription, pending the client's next
/// `REQ` carrying a narrowed since/until window.
const PAGE_SIZE: usize = 512;

/// The cap on one negentropy reply, mirroring the client's.
const NEG_FRAME_BYTES: usize = 4 * 1024;

/// The relay half's state, held by the session for the life of the link: the
/// subscriptions the peer has open, which the live-push path reads.
#[derive(Default)]
pub struct Relay {
    /// Open subscriptions, with every filter each one holds.
    subscriptions: BTreeMap<SubscriptionId, Vec<Filter>>,
}

impl Relay {
    /// The open subscriptions whose filters match `event`, for the live push.
    #[must_use]
    pub fn matching(&self, event: &HashedEvent) -> Vec<SubscriptionId> {
        self.subscriptions
            .iter()
            .filter(|(_, filters)| filters.iter().any(|filter| filter.matches(event)))
            .map(|(subscription, _)| subscription.clone())
            .collect()
    }

    /// Handle a message this device's relay half received.
    pub fn handle(
        &mut self,
        db: &Db,
        peer: &Peer,
        identity: &SecretKey,
        message: Message,
        quota: Quota,
        spending: &mut SessionSpending,
    ) -> Result<Vec<Message>> {
        match message {
            Message::Publish(event) => {
                let (events, bytes) = spending.total(peer);

                match client::admits(peer, &event, None, quota, events, bytes) {
                    Ok(()) => {
                        client::ingest(db, peer, &event, None)?;
                        spending.record(peer, &event);

                        Ok(vec![Message::Ok(event.id.to_hex(), true, "stored".into())])
                    }
                    Err(rejected) => Ok(vec![Message::Ok(
                        event.id.to_hex(),
                        false,
                        reason(rejected).into(),
                    )]),
                }
            }
            Message::Req(subscription, filters) => {
                // Registration is what the live-push path reads: NIP-01
                // replaces the filters when the same id is REQ'd again, and
                // CLOSE drops it.
                self.subscriptions
                    .insert(subscription.clone(), filters.clone());

                serve(db, peer, identity, &subscription, &filters)
            }
            Message::Close(subscription) => {
                self.subscriptions.remove(&subscription);

                Ok(Vec::new())
            }
            Message::NegOpen(subscription, filter, frame) => {
                self.subscriptions
                    .insert(subscription.clone(), vec![filter.clone()]);

                negotiate(db, peer, &subscription, &filter, &frame)
            }
            Message::NegMsg(subscription, frame) => {
                let Some(filters) = self.subscriptions.get(&subscription) else {
                    bail!("a NEG-MSG arrived for a subscription that is not open");
                };

                // A NEG-OPEN registers one filter, which is all NIP-77 uses.
                let filter = filters.first().cloned().unwrap_or_default();

                negotiate(db, peer, &subscription, &filter, &frame)
            }
            _ => Ok(Vec::new()),
        }
    }
}

/// The word a rejection travels as, in the `OK` message's reason slot.
fn reason(rejected: Rejected) -> &'static str {
    match rejected {
        Rejected::Unauthorized => "unauthorized",
        Rejected::OutOfScope => "outside scope",
        Rejected::OverQuota => "over quota",
        Rejected::TooLarge => "too large",
    }
}

/// The query answering `filter` for this peer.
///
/// The peer's filter is the only part of this that came off the wire. The
/// registers and the policy are the user's, and a peer cannot widen either.
#[must_use]
pub fn query_for(peer: &Peer, filter: Filter) -> Query {
    let query = Query::new()
        .with_filter(filter)
        .with_registers(Registers::offerable(peer.identity));

    // Blocked wins over every identity: any blocked pubkey the peer proved sinks
    // the whole session. Otherwise any unblocked one may be served, and the
    // first is as good as any other — the bindings differ only in standing,
    // and none of the unblocked ones is blocked.
    let binding = peer
        .policies
        .iter()
        .find(|policy| policy.is_blocked())
        .or_else(|| peer.policies.first());

    match binding {
        Some(policy) => query.with_policy(policy.clone()),
        None => query,
    }
}

/// The set the NIP-77 negentropy pass diffs, bounded by the same query as
/// everything else the relay half serves.
pub fn reconcilable(db: &Db, peer: &Peer, filter: Filter) -> Result<SyncSet> {
    let items = db_query::list_events(db, &query_for(peer, filter))?
        .into_iter()
        .map(|event| Item {
            timestamp: event.created_at,
            id: event.id,
        });

    Ok(SyncSet::from_items(items))
}

/// One round of negotiation from the responder's side.
///
/// The responder is stateless: every reply follows from the peer's frame and
/// this device's store under the subscription's filter. `reconcile_responder`
/// answers once per round; the initiator drives until it has enough.
fn negotiate(
    db: &Db,
    peer: &Peer,
    subscription: &SubscriptionId,
    filter: &Filter,
    frame: &[u8],
) -> Result<Vec<Message>> {
    let incoming = coracle_lib::sync::Message::decode(frame)
        .context("the peer's negentropy frame is malformed")?;
    let local = reconcilable(db, peer, filter.clone())?;
    let reply = coracle_lib::sync::reconcile_responder(
        &local,
        &incoming,
        FrameBudget::bytes(NEG_FRAME_BYTES),
    );

    Ok(vec![Message::NegMsg(subscription.clone(), reply.encode())])
}

/// Serve one subscription, paginated in reverse chronological order with
/// dynamic since/until windows.
///
/// Each served event travels with what proves it one hop further: an own event
/// with this device's signature naming the peer, a forwardable one with a
/// designated-verifier proof instead of the signature it was built from.
pub fn serve(
    db: &Db,
    peer: &Peer,
    identity: &SecretKey,
    subscription: &SubscriptionId,
    filters: &[Filter],
) -> Result<Vec<Message>> {
    let mut events = Vec::new();

    for filter in filters {
        // A filter with a limit serves that many; without it, one page.
        let filter = filter.clone();
        let mut serves = db_query::list_events(db, &query_for(peer, filter.clone()))?;
        events.append(&mut serves);
    }

    events.dedup_by(|a, b| a.id == b.id);
    events.sort_by(|a, b| b.created_at.cmp(&a.created_at));

    let mut messages: Vec<Message> = Vec::new();

    for event in events.into_iter().take(PAGE_SIZE) {
        messages.push(Message::Event(
            subscription.clone(),
            Box::new(event.clone()),
        ));
        attach(db, peer, identity, subscription, &event, &mut messages)?;
    }

    messages.push(Message::Eose(subscription.clone()));

    Ok(messages)
}

/// Attach what lets an event travel its next hop, following the `EVENT`.
///
/// An own event is signed for the peer; a forwardable one is proved to it. A
/// peer that proved no pubkey — which cannot happen once identified — simply
/// gets the bare event. The live-push path
/// ([`Session::offer_event`](crate::session::Session::offer_event)) attaches
/// through here too, so what accompanies an event cannot drift between the
/// serve and the offer.
pub fn attach(
    db: &Db,
    peer: &Peer,
    identity: &SecretKey,
    subscription: &SubscriptionId,
    event: &coracle_lib::events::HashedEvent,
    messages: &mut Vec<Message>,
) -> Result<()> {
    // The peer's identity is the pubkey it authenticates with, which in the
    // one-key custody model is the one it proved.
    let Some(peer_key) = peer.pubkeys().next().copied() else {
        return Ok(());
    };

    if event.pubkey == identity.public_key() {
        let signature = RecipientSignature::sign(identity, event.id, peer_key);

        messages.push(Message::RecipientSignature(
            subscription.clone(),
            event.id,
            Box::new(signature.sig),
        ));
    } else if let Some(signature) = db_query::get_signature(db, &event.id, &identity.public_key())?
    {
        let proof = AuthorshipProof::prove(&signature, peer_key)?;

        messages.push(Message::AuthorshipProof(
            subscription.clone(),
            event.id,
            Box::new(proof.to_bytes()),
        ));
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use coracle_lib::events::HashedEvent;
    use coracle_lib::filters::Filter;
    use coracle_lib::keys::PublicKey;
    use coracle_lib::tags::Tags;

    use std::sync::Arc;

    use crate::db::command;
    use crate::db::query as db_query;
    use crate::fixtures::{author, note, secret};
    use crate::link::LinkId;
    use crate::model::{Policy, RecipientSignature, Scope};
    use crate::spending::SpendingLedger;

    fn us() -> PublicKey {
        author(1)
    }

    fn peer() -> Peer {
        Peer::bind(LinkId(1), [author(2)], &Policy::new(us()))
    }

    fn spending() -> SessionSpending {
        SessionSpending::new(Arc::new(SpendingLedger::default()))
    }

    fn given(db: &Db, author: PublicKey, at: i64, content: &str) -> HashedEvent {
        let event = note(author, at, content, Tags::new());

        command::publish_event(db, &event, &author, at).unwrap();
        event
    }

    #[test]
    fn a_publish_is_answered_with_ok() {
        let db = Db::open_in_memory().unwrap();
        let event = note(author(2), 100, "published", Tags::new());

        let replies = Relay::default()
            .handle(
                &db,
                &peer(),
                &secret(1),
                Message::Publish(Box::new(event.clone())),
                Quota::STRANGER,
                &mut spending(),
            )
            .unwrap();

        assert_eq!(
            replies,
            vec![Message::Ok(event.id.to_hex(), true, "stored".into())]
        );
        assert_eq!(
            db_query::list_events(&db, &Query::new()).unwrap(),
            vec![event]
        );
    }

    #[test]
    fn an_unauthorized_publish_is_rejected() {
        let db = Db::open_in_memory().unwrap();
        // Authored by someone the peer did not prove.
        let event = note(author(4), 100, "sneaky", Tags::new());

        let replies = Relay::default()
            .handle(
                &db,
                &peer(),
                &secret(1),
                Message::Publish(Box::new(event.clone())),
                Quota::STRANGER,
                &mut spending(),
            )
            .unwrap();

        assert_eq!(
            replies,
            vec![Message::Ok(event.id.to_hex(), false, "unauthorized".into())]
        );
        assert!(
            db_query::list_events(&db, &Query::new())
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn a_blocked_peer_is_not_stored_even_when_it_authored() {
        let db = Db::open_in_memory().unwrap();
        let mut policy = Policy::new(us());
        policy.graph.blocked.insert(author(2));
        let peer = Peer::bind(LinkId(1), [author(2)], &policy);
        let event = note(author(2), 100, "blocked", Tags::new());

        let replies = Relay::default()
            .handle(
                &db,
                &peer,
                &secret(1),
                Message::Publish(Box::new(event)),
                Quota::STRANGER,
                &mut spending(),
            )
            .unwrap();

        assert!(matches!(replies.first(), Some(Message::Ok(_, false, _))));
        assert!(
            db_query::list_events(&db, &Query::new())
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn a_req_is_answered_with_events_and_eose() {
        let db = Db::open_in_memory().unwrap();
        // An event the device itself authored is in the Own register and so
        // offerable; a stranger's is not.
        let stored = given(&db, author(1), 100, "one");

        let replies = Relay::default()
            .handle(
                &db,
                &peer(),
                &secret(1),
                Message::Req(
                    SubscriptionId("sub".into()),
                    vec![Filter::new().add_kinds([1])],
                ),
                Quota::STRANGER,
                &mut spending(),
            )
            .unwrap();

        assert!(replies.contains(&Message::Event(
            SubscriptionId("sub".into()),
            Box::new(stored.clone()),
        )));
        assert!(replies.contains(&Message::Eose(SubscriptionId("sub".into()))));
    }

    #[test]
    fn an_own_event_is_served_with_its_signature() {
        let db = Db::open_in_memory().unwrap();
        let stored = given(&db, author(1), 100, "mine");

        let replies = serve(
            &db,
            &peer(),
            &secret(1),
            &SubscriptionId("sub".into()),
            &[Filter::new().add_kinds([1])],
        )
        .unwrap();

        assert!(matches!(&replies[0], Message::Event(_, _)));
        assert!(matches!(
            &replies[1],
            Message::RecipientSignature(_, id, _) if *id == stored.id
        ));
        assert!(matches!(&replies[2], Message::Eose(_)));
    }

    #[test]
    fn a_forwardable_event_is_served_with_a_proof() {
        let db = Db::open_in_memory().unwrap();
        // The event is someone else's, and this device holds the signature
        // naming itself — which is what puts it in the forwardable register.
        let event = note(author(3), 100, "forward", Tags::new());
        command::publish_event(&db, &event, &author(3), 100).unwrap();
        let signature = RecipientSignature::sign(&secret(3), event.id, author(1));
        command::receive_signature(&db, &event.id, &signature.sig, &author(1)).unwrap();

        // Gossip admits the author, or the register check alone would filter it.
        let mut policy = Policy::new(us());
        policy.gossip = Scope::Lenient;
        let peer = Peer::bind(LinkId(1), [author(2)], &policy);

        let replies = serve(
            &db,
            &peer,
            &secret(1),
            &SubscriptionId("sub".into()),
            &[Filter::new().add_kinds([1])],
        )
        .unwrap();

        assert!(matches!(&replies[0], Message::Event(_, _)));
        assert!(matches!(
            &replies[1],
            Message::AuthorshipProof(_, id, _) if *id == event.id
        ));
        assert!(matches!(&replies[2], Message::Eose(_)));
    }

    #[test]
    fn a_req_registers_the_subscription_for_live_offers() {
        let db = Db::open_in_memory().unwrap();
        let mut relay = Relay::default();

        relay
            .handle(
                &db,
                &peer(),
                &secret(1),
                Message::Req(
                    SubscriptionId("sub".into()),
                    vec![Filter::new().add_kinds([1])],
                ),
                Quota::STRANGER,
                &mut spending(),
            )
            .unwrap();

        assert_eq!(
            relay.subscriptions.get(&SubscriptionId("sub".into())),
            Some(&vec![Filter::new().add_kinds([1])])
        );

        // A REQ with the same id replaces the filters, per NIP-01.
        relay
            .handle(
                &db,
                &peer(),
                &secret(1),
                Message::Req(
                    SubscriptionId("sub".into()),
                    vec![Filter::new().add_kinds([1, 7])],
                ),
                Quota::STRANGER,
                &mut spending(),
            )
            .unwrap();

        assert_eq!(relay.subscriptions.len(), 1);
        assert_eq!(
            relay.subscriptions.get(&SubscriptionId("sub".into())),
            Some(&vec![Filter::new().add_kinds([1, 7])])
        );
    }

    #[test]
    fn a_close_drops_the_subscription() {
        let db = Db::open_in_memory().unwrap();
        let mut relay = Relay::default();
        relay
            .subscriptions
            .insert(SubscriptionId("sub".into()), vec![Filter::new()]);

        relay
            .handle(
                &db,
                &peer(),
                &secret(1),
                Message::Close(SubscriptionId("sub".into())),
                Quota::STRANGER,
                &mut spending(),
            )
            .unwrap();

        assert!(relay.subscriptions.is_empty());
    }

    #[test]
    fn matching_names_the_subscriptions_an_event_falls_under() {
        let mut relay = Relay::default();
        relay.subscriptions.insert(
            SubscriptionId("notes".into()),
            vec![Filter::new().add_kinds([1])],
        );
        relay.subscriptions.insert(
            SubscriptionId("reactions".into()),
            vec![Filter::new().add_kinds([7])],
        );

        let event = note(author(1), 100, "a note", Tags::new());

        assert_eq!(relay.matching(&event), vec![SubscriptionId("notes".into())]);
    }

    #[test]
    fn a_negotiation_responder_tells_the_client_what_it_lacks() {
        // Bob (relay) has an event Alice (client) does not. Alice opens a
        // negotiation against Bob's relay half; the IDs Bob has and Alice
        // lacks surface in the terminating REQ.
        let server = Db::open_in_memory().unwrap();
        let bobs_note = given(&server, author(2), 200, "bob's");
        let alice = Db::open_in_memory().unwrap();

        // Each device sees the other as the peer. Alice is identity 1 facing
        // Bob's device (author 2), holding her own store (empty); Bob is
        // identity 2 facing Alice (author 1), holding his own note.
        let alice_peer = Peer::bind(LinkId(1), [author(2)], &Policy::new(author(1)));
        let server_peer = Peer::bind(LinkId(2), [author(1)], &Policy::new(author(2)));

        // Alice's side, as the client half builds it.
        let (mut negotiation, opening) =
            crate::sync::client::Negotiation::begin(&alice, &alice_peer, Filter::new()).unwrap();
        let Message::NegOpen(subscription, filter, frame) = opening else {
            panic!("expected NEG-OPEN");
        };

        // Bob's relay half answers the opening.
        let replies = Relay::default()
            .handle(
                &server,
                &server_peer,
                &secret(2),
                Message::NegOpen(subscription.clone(), filter.clone(), frame),
                Quota::STRANGER,
                &mut spending(),
            )
            .unwrap();

        // Feed Bob's reply round back through Alice's step.
        let Message::NegMsg(_, reply) = replies.last().unwrap() else {
            panic!("expected the relay's NEG-MSG");
        };
        let out = negotiation.step(reply).unwrap();

        match out.last() {
            Some(Message::Req(_, filters)) => {
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
}
