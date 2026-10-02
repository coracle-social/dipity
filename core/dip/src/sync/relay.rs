//! This device answering a peer: the relay half.
//!
//! Everything it has to apply is expressible as a [`Query`]. A peer's `REQ`
//! becomes a filter, the user's Gossip scope and visibility become a
//! [`PeerPolicy`](crate::model::PeerPolicy), and the two authorship registers
//! that may travel become [`Registers::offerable`]. `db::query::list_events`
//! takes all three, so there is no second place where what a peer may see is
//! decided.

use std::collections::BTreeMap;

use anyhow::{Context, Result, bail};
use coracle_lib::events::{EventId, HashedEvent};
use coracle_lib::filters::Filter;
use coracle_lib::keys::PublicKey;

use crate::keys::Signer;
use coracle_lib::sync::{FrameBudget, SyncSet};

use crate::clock;
use crate::db::Db;
use crate::db::command as db_command;
use crate::db::query as db_query;
use crate::model::{AuthorshipProof, Identity, Query, RecipientSignature, Registers};
use crate::session::Peer;
use crate::sync::client::{self, Rejected};
use crate::sync::spending::SessionSpending;
use crate::sync::{Message, Quota, SubscriptionId};

/// A page of events served on one subscription, pending the client's next
/// `REQ` carrying a narrowed since/until window.
const PAGE_SIZE: usize = 512;

/// The cap on one negentropy reply, mirroring the client's.
const NEG_FRAME_BYTES: usize = 4 * 1024;

/// The relay half's state, held by the session for the life of the link.
///
/// Subscriptions and negotiations are kept apart because they grant different
/// things. A `REQ` asks to be served matching events now and as they arrive; a
/// `NEG-OPEN` asks to diff a set. Holding both in one map let a peer open a
/// live subscription over everything by sending a reconciliation and no `REQ`
/// at all.
#[derive(Default)]
pub struct Relay {
    /// Open subscriptions, with every filter each one holds. The live-push
    /// path reads these.
    subscriptions: BTreeMap<SubscriptionId, Vec<Filter>>,
    /// The filter each open reconciliation is bounded by, which its later
    /// `NEG-MSG` rounds are answered under.
    negotiations: BTreeMap<SubscriptionId, Filter>,
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
        identity: &dyn Signer,
        message: Message,
        quota: Quota,
        spending: &mut SessionSpending,
    ) -> Result<Vec<Message>> {
        let local = Identity::from([identity.pubkey()]);

        match message {
            Message::Publish(event) => {
                let spent = spending.spent(peer);

                match client::admits(peer, &event, None, quota, spent) {
                    Ok(()) => {
                        if client::ingest(db, peer, &local, &event, None)? {
                            spending.record(peer, &event);
                        }

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
                // NIP-01 replaces the filters when the same id is REQ'd again, and CLOSE drops it.
                self.subscriptions
                    .insert(subscription.clone(), filters.clone());

                serve(db, peer, identity, &subscription, &filters)
            }
            Message::Close(subscription) => {
                self.subscriptions.remove(&subscription);
                self.negotiations.remove(&subscription);

                Ok(Vec::new())
            }
            Message::NegOpen(subscription, filter, frame) => {
                self.negotiations
                    .insert(subscription.clone(), filter.clone());

                negotiate(db, peer, &local, &subscription, &filter, &frame)
            }
            Message::NegMsg(subscription, frame) => {
                let Some(filter) = self.negotiations.get(&subscription).cloned() else {
                    bail!("a NEG-MSG arrived for a negotiation that is not open");
                };

                negotiate(db, peer, &local, &subscription, &filter, &frame)
            }
            Message::NegClose(subscription) => {
                self.negotiations.remove(&subscription);

                Ok(Vec::new())
            }
            _ => Ok(Vec::new()),
        }
    }
}

/// The word a rejection travels as, in the `OK` message's reason slot.
fn reason(rejected: Rejected) -> &'static str {
    match rejected {
        Rejected::Forged => "invalid id",
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
///
/// There is no binding to choose between: [`Peer`] reduced whatever the far
/// side proved to one standing when it was bound, and the registers are
/// measured from whichever identities this device is acting as. So this is the
/// one place that decides what a peer may see, and it decides it once.
#[must_use]
pub fn query_for(peer: &Peer, local: &Identity, filter: Filter) -> Query {
    Query::new()
        .with_filter(filter)
        .with_registers(Registers::offerable(local))
        .with_policy(peer.policy.clone())
}

/// The set the NIP-77 negentropy pass diffs, under the query the relay half serves.
pub fn reconcilable(db: &Db, peer: &Peer, local: &Identity, filter: Filter) -> Result<SyncSet> {
    db_query::reconciliation_set(db, &query_for(peer, local, filter))
}

/// One round of negotiation from the responder's side.
///
/// The responder is stateless: every reply follows from the peer's frame and
/// this device's store under the subscription's filter. `reconcile_responder`
/// answers once per round; the initiator drives until it has enough.
fn negotiate(
    db: &Db,
    peer: &Peer,
    local: &Identity,
    subscription: &SubscriptionId,
    filter: &Filter,
    frame: &[u8],
) -> Result<Vec<Message>> {
    let incoming = coracle_lib::sync::Message::decode(frame)
        .context("the peer's negentropy frame is malformed")?;
    let ours = reconcilable(db, peer, local, filter.clone())?;
    let reply = coracle_lib::sync::reconcile_responder(
        &ours,
        &incoming,
        FrameBudget::bytes(NEG_FRAME_BYTES),
    );

    Ok(vec![Message::NegMsg(subscription.clone(), reply.encode())])
}

/// The filter as it is served: a page at most, whatever the peer asked for.
fn paged(filter: &Filter) -> Filter {
    let limit = filter.limit.map_or(PAGE_SIZE, |limit| limit.min(PAGE_SIZE));

    filter.clone().add_limit(limit)
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
    identity: &dyn Signer,
    subscription: &SubscriptionId,
    filters: &[Filter],
) -> Result<Vec<Message>> {
    let local = Identity::from([identity.pubkey()]);
    let mut events = Vec::new();

    for filter in filters {
        let mut serves = db_query::list_events(db, &query_for(peer, &local, paged(filter)))?;
        events.append(&mut serves);
    }

    // Sort before deduplicating: `dedup_by` only removes adjacent equals.
    events.sort_by(|a, b| b.created_at.cmp(&a.created_at).then(a.id.cmp(&b.id)));
    events.dedup_by(|a, b| a.id == b.id);

    let mut messages: Vec<Message> = Vec::new();
    let page: Vec<HashedEvent> = events.into_iter().take(PAGE_SIZE).collect();

    for event in &page {
        messages.push(Message::Event(
            subscription.clone(),
            Box::new(event.clone()),
        ));
        attach(db, peer, identity, subscription, event, &mut messages)?;
    }

    messages.push(Message::Eose(subscription.clone()));
    record_shares(db, peer, &page)?;

    Ok(messages)
}

/// Record that events went to a peer, which is provenance in the outbound
/// direction and never served to anyone.
///
/// Every path that hands a peer an event goes through here: [`serve`] for a
/// subscription's page and
/// [`Session::offer_event`](crate::session::Session::offer_event) for the live
/// push. Written when the frames are handed to the transport rather than when
/// the peer has them, since NIP-01 has a relay hear nothing back about an
/// `EVENT`.
pub fn record_shares(db: &Db, peer: &Peer, events: &[HashedEvent]) -> Result<()> {
    let to: Vec<PublicKey> = peer.pubkeys.iter().copied().collect();
    let forwards = peer.policy.may_forward();
    let shares: Vec<(EventId, bool)> = events
        .iter()
        .map(|event| {
            (
                event.id,
                forwards && event.pubkey == peer.policy.policy.identity,
            )
        })
        .collect();

    db_command::record_shares(db, &shares, &to, clock::now())
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
    identity: &dyn Signer,
    subscription: &SubscriptionId,
    event: &coracle_lib::events::HashedEvent,
    messages: &mut Vec<Message>,
) -> Result<()> {
    // One artifact per identity the peer proved, since nothing on the wire says which it uses.
    if event.pubkey == identity.pubkey() {
        // The signature is transferable evidence, so who gets one is a policy question.
        if !peer.policy.may_forward() {
            return Ok(());
        }

        for recipient in peer.pubkeys.iter() {
            let signature = RecipientSignature::sign(identity.key()?, event.id, *recipient);

            messages.push(Message::RecipientSignature(
                subscription.clone(),
                event.id,
                Box::new(signature.sig),
            ));
        }
    } else if let Some(signature) = db_query::get_signature(db, &event.id, &identity.pubkey())? {
        // A proof is designated to one peer and worthless to anyone else, so it is not gated.
        for verifier in peer.pubkeys.iter() {
            let proof = AuthorshipProof::prove(&signature, *verifier)?;

            messages.push(Message::AuthorshipProof(
                subscription.clone(),
                event.id,
                Box::new(proof.to_bytes()),
            ));
        }
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
    use crate::sync::spending::SpendingLedger;

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
        // An event the device authored is offerable through the Own register; a stranger's is not.
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
    fn serving_an_event_records_where_it_went() {
        let db = Db::open_in_memory().unwrap();
        let stored = given(&db, author(1), 100, "one");

        let req = |relay: &mut Relay| {
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
                .unwrap()
        };

        req(&mut Relay::default());

        let shares = |db: &Db| {
            db_query::with_details(db, vec![stored.clone()]).unwrap()[0]
                .shares
                .iter()
                .map(|share| share.pubkey)
                .collect::<Vec<_>>()
        };

        assert_eq!(shares(&db), vec![author(2)]);

        // The same peer asking again is the same handoff, however many times it asks.
        req(&mut Relay::default());

        assert_eq!(shares(&db), vec![author(2)]);
    }

    #[test]
    fn an_own_event_is_served_with_its_signature() {
        let db = Db::open_in_memory().unwrap();
        let stored = given(&db, author(1), 100, "mine");

        // Trusted, because the signature is transferable and Forward defaults to the trusted.
        let mut policy = Policy::new(us());
        policy.graph.trusted.insert(author(2));
        let trusted = Peer::bind(LinkId(1), [author(2)], &policy);

        let replies = serve(
            &db,
            &trusted,
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
    fn a_peer_that_proved_several_identities_gets_one_artifact_each() {
        // Naming one identity would leave the event dead on arrival whenever the guess was wrong.
        let db = Db::open_in_memory().unwrap();
        let stored = given(&db, author(1), 100, "mine");

        let mut policy = Policy::new(us());
        policy.graph.trusted.insert(author(2));
        let peer = Peer::bind(LinkId(1), [author(2), author(3)], &policy);

        let replies = serve(
            &db,
            &peer,
            &secret(1),
            &SubscriptionId("sub".into()),
            &[Filter::new().add_kinds([1])],
        )
        .unwrap();

        let named: Vec<_> = replies
            .iter()
            .filter_map(|reply| match reply {
                Message::RecipientSignature(_, id, sig) if *id == stored.id => Some(**sig),
                _ => None,
            })
            .collect();

        assert_eq!(named.len(), 2, "one signature per proved identity");

        // Each verifies under the identity it was minted for, and neither is the other's.
        for recipient in [author(2), author(3)] {
            assert!(
                named.iter().any(|sig| RecipientSignature {
                    event_id: stored.id,
                    author_pubkey: us(),
                    recipient_pubkey: recipient,
                    sig: *sig,
                }
                .verifies()),
                "nothing verifies for {recipient:?}"
            );
        }
    }

    #[test]
    fn a_stranger_is_served_the_event_without_the_signature() {
        // `docs/proofs.md`: only peers trusted not to leak a signature should receive one.
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

        assert!(
            matches!(&replies[0], Message::Event(_, event) if event.id == stored.id),
            "the event itself is still served"
        );
        assert!(
            !replies
                .iter()
                .any(|reply| matches!(reply, Message::RecipientSignature(..))),
            "a stranger was handed the author's signature"
        );
        assert!(matches!(&replies[1], Message::Eose(_)));
    }

    #[test]
    fn a_forwardable_event_is_served_with_a_proof() {
        let db = Db::open_in_memory().unwrap();
        // The event is someone else's and this device holds the signature naming itself.
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
    fn a_negotiation_does_not_open_a_live_subscription() {
        // A peer that sends NEG-OPEN and never a REQ has asked to be served nothing.
        let db = Db::open_in_memory().unwrap();
        let mut relay = Relay::default();
        let (_, opening) = crate::sync::client::Negotiation::begin(&db, Filter::new()).unwrap();
        let Message::NegOpen(subscription, filter, frame) = opening else {
            panic!("expected NEG-OPEN");
        };

        relay
            .handle(
                &db,
                &peer(),
                &secret(1),
                Message::NegOpen(subscription, filter, frame),
                Quota::STRANGER,
                &mut spending(),
            )
            .unwrap();

        let own = note(
            author(1),
            100,
            "written while they are still here",
            Tags::new(),
        );

        assert!(
            relay.matching(&own).is_empty(),
            "a reconciliation granted a live subscription"
        );
    }

    #[test]
    fn a_req_since_now_catches_what_is_written_next() {
        // What the live push rides on: a standing REQ carries the next write across.
        let db = Db::open_in_memory().unwrap();
        let mut relay = Relay::default();

        relay
            .handle(
                &db,
                &peer(),
                &secret(1),
                Message::Req(
                    SubscriptionId("live".into()),
                    vec![Filter::new().add_since(100)],
                ),
                Quota::STRANGER,
                &mut spending(),
            )
            .unwrap();

        let fresh = note(author(1), 200, "written after the sync", Tags::new());
        let stale = note(author(1), 50, "older than the subscription", Tags::new());

        assert_eq!(relay.matching(&fresh), vec![SubscriptionId("live".into())]);
        assert!(relay.matching(&stale).is_empty());
    }

    #[test]
    fn a_negotiation_responder_tells_the_client_what_it_lacks() {
        // Bob has an event Alice does not; the ids she lacks surface in the terminating REQ.
        let server = Db::open_in_memory().unwrap();
        let bobs_note = given(&server, author(2), 200, "bob's");
        let alice = Db::open_in_memory().unwrap();

        // Bob sees Alice as the peer: identity 2 facing author 1, holding his own note.
        let server_peer = Peer::bind(LinkId(2), [author(1)], &Policy::new(author(2)));

        // Alice's side, as the client half builds it.
        let (mut negotiation, opening) =
            crate::sync::client::Negotiation::begin(&alice, Filter::new()).unwrap();
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

    #[test]
    fn a_req_with_no_limit_asks_the_store_for_one_page() {
        assert_eq!(paged(&Filter::new()).limit, Some(PAGE_SIZE));
    }

    #[test]
    fn a_peers_limit_is_honoured_but_never_above_a_page() {
        assert_eq!(paged(&Filter::new().add_limit(3)).limit, Some(3));
        assert_eq!(
            paged(&Filter::new().add_limit(PAGE_SIZE * 2)).limit,
            Some(PAGE_SIZE)
        );
    }

    #[test]
    fn a_req_serves_no_more_than_the_limit_it_carries() {
        let db = Db::open_in_memory().unwrap();
        let newest = given(&db, author(1), 300, "three");
        given(&db, author(1), 200, "two");
        given(&db, author(1), 100, "one");

        let replies = Relay::default()
            .handle(
                &db,
                &peer(),
                &secret(1),
                Message::Req(
                    SubscriptionId("sub".into()),
                    vec![Filter::new().add_kinds([1]).add_limit(1)],
                ),
                Quota::STRANGER,
                &mut spending(),
            )
            .unwrap();

        let served: Vec<_> = replies
            .iter()
            .filter_map(|message| match message {
                Message::Event(_, event) => Some(event.id),
                _ => None,
            })
            .collect();

        assert_eq!(served, vec![newest.id]);
    }
}
