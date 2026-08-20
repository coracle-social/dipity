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
use coracle_lib::filters::Filter;
use coracle_lib::sync::{FrameBudget, Item, SyncSet};

use crate::db::Db;
use crate::db::query as db_query;
use crate::model::{Query, Registers};
use crate::session::Peer;
use crate::sync::client::{self, Rejected, Used};
use crate::sync::{Message, Quota, SubscriptionId};

/// A page of events served on one subscription, pending the client's next
/// `REQ` carrying a narrowed since/until window.
const PAGE_SIZE: usize = 512;

/// The cap on one negentropy reply, mirroring the client's.
const NEG_FRAME_BYTES: usize = 4 * 1024;

/// Handle a message this device's relay half received.
pub fn handle(
    db: &Db,
    peer: &Peer,
    message: Message,
    quota: Quota,
    used: &mut Used,
    subscriptions: &mut BTreeMap<SubscriptionId, Filter>,
) -> Result<Vec<Message>> {
    match message {
        Message::Publish(event) => match client::admits(peer, &event, None, quota, *used) {
            Ok(()) => {
                client::ingest(db, peer, &event, None)?;
                used.record(&event);

                Ok(vec![Message::Ok(event.id.to_hex(), true, "stored".into())])
            }
            Err(rejected) => Ok(vec![Message::Ok(
                event.id.to_hex(),
                false,
                reason(rejected).into(),
            )]),
        },
        Message::Req(subscription, filters) => serve(db, peer, &subscription, &filters),
        Message::Close(subscription) => {
            subscriptions.remove(&subscription);

            Ok(Vec::new())
        }
        Message::NegOpen(subscription, filter, frame) => {
            subscriptions.insert(subscription.clone(), filter.clone());

            negotiate(db, peer, &subscription, &filter, &frame)
        }
        Message::NegMsg(subscription, frame) => {
            let Some(filter) = subscriptions.get(&subscription) else {
                bail!("a NEG-MSG arrived for a subscription that is not open");
            };

            negotiate(db, peer, &subscription, filter, &frame)
        }
        _ => Ok(Vec::new()),
    }
}

/// The word a rejection travels as, in the `OK` message's reason slot.
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

    // Blocked wins over every identity, so any session reaching here may be served.
    match peer.policies.first() {
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
pub fn serve(
    db: &Db,
    peer: &Peer,
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

    let mut messages: Vec<Message> = events
        .into_iter()
        .take(PAGE_SIZE)
        .map(|event| Message::Event(subscription.clone(), Box::new(event)))
        .collect();

    messages.push(Message::Eose(subscription.clone()));

    Ok(messages)
}

#[cfg(test)]
mod tests {
    use super::*;
    use coracle_lib::events::HashedEvent;
    use coracle_lib::filters::Filter;
    use coracle_lib::keys::PublicKey;
    use coracle_lib::tags::Tags;

    use crate::db::command;
    use crate::db::query as db_query;
    use crate::fixtures::{author, note};
    use crate::link::LinkId;
    use crate::model::Policy;

    fn us() -> PublicKey {
        author(1)
    }

    fn peer() -> Peer {
        Peer::bind(LinkId(1), [author(2)], &Policy::new(us()))
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

        let replies = handle(
            &db,
            &peer(),
            Message::Publish(Box::new(event.clone())),
            Quota::STRANGER,
            &mut Used::default(),
            &mut BTreeMap::new(),
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

        let replies = handle(
            &db,
            &peer(),
            Message::Publish(Box::new(event.clone())),
            Quota::STRANGER,
            &mut Used::default(),
            &mut BTreeMap::new(),
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

        let replies = handle(
            &db,
            &peer,
            Message::Publish(Box::new(event)),
            Quota::STRANGER,
            &mut Used::default(),
            &mut BTreeMap::new(),
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

        let replies = handle(
            &db,
            &peer(),
            Message::Req(
                SubscriptionId("sub".into()),
                vec![Filter::new().add_kinds([1])],
            ),
            Quota::STRANGER,
            &mut Used::default(),
            &mut BTreeMap::new(),
        )
        .unwrap();

        assert!(replies.contains(&Message::Event(
            SubscriptionId("sub".into()),
            Box::new(stored.clone()),
        )));
        assert!(replies.contains(&Message::Eose(SubscriptionId("sub".into()))));
    }

    #[test]
    fn a_close_drops_the_subscription() {
        let db = Db::open_in_memory().unwrap();
        let mut subscriptions = BTreeMap::from([(SubscriptionId("sub".into()), Filter::new())]);

        handle(
            &db,
            &peer(),
            Message::Close(SubscriptionId("sub".into())),
            Quota::STRANGER,
            &mut Used::default(),
            &mut subscriptions,
        )
        .unwrap();

        assert!(subscriptions.is_empty());
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
        let mut subscriptions = BTreeMap::new();
        let replies = handle(
            &server,
            &server_peer,
            Message::NegOpen(subscription.clone(), filter.clone(), frame),
            Quota::STRANGER,
            &mut Used::default(),
            &mut subscriptions,
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
