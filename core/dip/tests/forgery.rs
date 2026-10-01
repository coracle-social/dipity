//! An event's id must be the hash of the event.
//!
//! Both authorship registers commit to an id rather than to a body: the
//! session says the peer authored the event with this id, and an authorship
//! proof is a signature over `event_id ‖ recipient_pubkey`. If an id could
//! name content it is not the hash of, one genuine proof would authorize
//! anything a forwarder cared to attach to it. `docs/proofs.md`.

use std::sync::Arc;

use coracle_lib::events::{EventContent, EventExtensionId, HashedEvent};
use coracle_lib::keys::SecretKey;
use coracle_lib::tags::Tags;

use dip::LinkId;
use dip::db::{Db, query};
use dip::model::{AuthorshipProof, Identity, Policy, Query, RecipientSignature};
use dip::session::Peer;
use dip::sync::client::Client;
use dip::sync::spending::{SessionSpending, SpendingLedger};
use dip::sync::{Message, Quota, SubscriptionId};

/// An event whose id names different content than it carries.
fn forged_under(author: &SecretKey, id: &HashedEvent, content: &str) -> HashedEvent {
    let json = serde_json::json!({
        "id": id.id.to_hex(),
        "pubkey": author.public_key().to_hex(),
        "created_at": id.created_at,
        "kind": id.kind,
        "tags": [],
        "content": content,
    });

    serde_json::from_value(json).expect("a well-formed event with a borrowed id")
}

fn authored(author: &SecretKey, content: &str) -> HashedEvent {
    EventContent::new()
        .with_content(content)
        .with_tags(Tags::new())
        .with_kind(1)
        .with_created_at(1_000)
        .with_pubkey(author.public_key())
        .with_id()
}

#[test]
fn a_forwarder_cannot_reuse_a_proof_for_content_the_author_never_wrote() {
    let alice = SecretKey::generate(); // the author
    let bob = SecretKey::generate(); // the first-hop recipient
    let carol = SecretKey::generate(); // this device, the second hop

    // Alice hands Bob the signature naming Bob, which is what lets Bob forward it one hop.
    let real = authored(&alice, "the real note");
    let signature = RecipientSignature::sign(&alice, real.id, bob.public_key());
    let proof = AuthorshipProof::prove(&signature, carol.public_key()).unwrap();

    // Bob keeps the id and swaps the body.
    let forged = forged_under(&alice, &real, "FORGED — Alice never wrote this");
    assert_eq!(forged.id, real.id);
    assert_ne!(forged.content, real.content);

    let db = Db::open_in_memory().unwrap();
    let mut client = Client::default();
    let peer = Peer::bind(
        LinkId(1),
        [bob.public_key()],
        &Policy::new(carol.public_key()),
    );
    let mut spending = SessionSpending::new(Arc::new(SpendingLedger::default()));
    let subscription = SubscriptionId("sub".into());

    client
        .handle(
            &db,
            &peer,
            &Identity::from([carol.public_key()]),
            Message::Event(subscription.clone(), Box::new(forged)),
            Quota::STRANGER,
            &mut spending,
        )
        .unwrap();

    client
        .handle(
            &db,
            &peer,
            &Identity::from([carol.public_key()]),
            Message::AuthorshipProof(subscription, real.id, Box::new(proof.to_bytes())),
            Quota::STRANGER,
            &mut spending,
        )
        .unwrap();

    assert!(
        query::list_events(&db, &Query::new()).unwrap().is_empty(),
        "an event whose id is not its hash was stored"
    );
}

#[test]
fn a_forged_event_decodes_so_that_admission_refuses_one_message_rather_than_the_link() {
    let alice = SecretKey::generate();
    let real = authored(&alice, "the real note");
    let forged = forged_under(&alice, &real, "FORGED");
    let carrier = Message::Event(SubscriptionId("sub".into()), Box::new(forged));

    match Message::decode(&carrier.encode()).unwrap() {
        Message::Event(_, decoded) => assert!(!decoded.verify_id()),
        other => panic!("expected an EVENT, got {other:?}"),
    }
}

#[test]
fn an_authentic_event_still_decodes() {
    let alice = SecretKey::generate();
    let real = authored(&alice, "the real note");
    let carrier = Message::Event(SubscriptionId("sub".into()), Box::new(real.clone()));

    match Message::decode(&carrier.encode()).unwrap() {
        Message::Event(_, decoded) => assert_eq!(*decoded, real),
        other => panic!("expected an EVENT, got {other:?}"),
    }
}
