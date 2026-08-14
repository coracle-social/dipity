//! The store through its public surface: the use cases in `db::query` and
//! `db::command`, against a real file, reached the way the shell reaches it.
//!
//! The unit tests each build their own in-memory store, so nothing there
//! touches [`dip::db::Db::open`], the migrations against a file, WAL, or the
//! transaction each use case opens.

use std::fs;
use std::path::PathBuf;

use coracle_lib::events::{EventContent, EventId, HashedEvent};
use coracle_lib::filters::Filter;
use coracle_lib::keys::{PublicKey, SecretKey};
use coracle_lib::sync::{Item, SyncSet};
use coracle_lib::tags::Tags;

use dip::db::event::channel::{self, EventChange};
use dip::db::{Db, command, query};
use dip::model::{BlobRole, Order, Query, Registers, Scope, keys};

/// The blob the event below references.
///
/// Shaped like a real sha256 rather than being the word "hash": the store keys
/// blobs on one and checks the shape on the way in, so a placeholder that could
/// never name a row would be testing the wrong thing.
const BLOB: &str = "b10bb10bb10bb10bb10bb10bb10bb10bb10bb10bb10bb10bb10bb10bb10bb10b";

fn database_directory(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!("dip-store-test-{}-{name}", std::process::id()))
}

/// A key from a seed, so a fixture is reproducible. `PublicKey` holds nothing
/// that is not a point on the curve, so these are real keys.
fn pubkey(seed: u8) -> PublicKey {
    SecretKey::from_hex(&hex::encode([seed; 32]))
        .unwrap()
        .public_key()
}

fn author() -> PublicKey {
    pubkey(1)
}

fn peer() -> PublicKey {
    pubkey(2)
}

fn us() -> PublicKey {
    pubkey(3)
}

fn note(author: PublicKey, created_at: i64, content: &str, tags: Tags) -> HashedEvent {
    EventContent::new()
        .with_content(content)
        .with_tags(tags)
        .with_kind(1)
        .with_created_at(created_at)
        .with_pubkey(author)
        .with_id()
}

fn id(event: &HashedEvent) -> EventId {
    event.id
}

/// Everything this device could put on the wire, whoever were asking.
fn offerable() -> Query {
    Query::new().with_registers(Registers::offerable(us()))
}

#[test]
fn the_store_serves_its_use_cases() {
    let directory = database_directory("use-cases");
    let _ = fs::remove_dir_all(&directory);

    let db = Db::open(&directory).unwrap();

    // Idempotent, because the shell may open the same directory on a
    // background relaunch as well as at startup.
    drop(Db::open(&directory).unwrap());

    let mut changes = channel::subscribe(&db);

    // An event, the author's signature naming this device, and the media it
    // references, in one call and one transaction.
    let with_media = note(
        author(),
        1_000,
        "hello neighbor",
        Tags::new().add(
            "imeta",
            [
                format!("x {BLOB}"),
                "m image/jpeg".into(),
                "size 2048".into(),
                "blake3 root".into(),
            ],
        ),
    );
    let signature = [7u8; 64];

    assert!(
        command::receive_event(&db, &with_media, &[peer()], Some(&signature), &us(), 100).unwrap()
    );

    match changes.try_recv() {
        Ok(EventChange::Stored(event)) => assert_eq!(event.id, with_media.id),
        other => panic!("expected the stored event, got {other:?}"),
    }

    // The feed reads the event and its media back together.
    let feed = query::with_details(
        &db,
        query::list_events(
            &db,
            &Query::new()
                .with_filter(Filter::new().add_kinds([1]).add_limit(50))
                .with_order(Order::SeenAt),
        )
        .unwrap(),
    )
    .unwrap();
    assert_eq!(feed.len(), 1);
    assert_eq!(feed[0].event, with_media);
    assert_eq!(feed[0].blobs.len(), 1);
    assert_eq!(feed[0].blobs[0].sha256, BLOB);
    assert_eq!(feed[0].blobs[0].size, Some(2048));
    assert_eq!(feed[0].blobs[0].blake3.as_deref(), Some("root"));

    let search = |text: &str| {
        query::list_events(
            &db,
            &Query::new().with_filter(Filter::new().add_search(text).add_limit(50)),
        )
        .unwrap()
    };

    assert_eq!(search("neighbor").len(), 1);
    assert!(search("elsewhere").is_empty());

    // Provenance is a detail of the event, and stays a separate read: it is
    // never part of what a peer is served.
    let detail = query::with_details(
        &db,
        query::list_events(
            &db,
            &Query::new().with_filter(Filter::new().add_id(with_media.id)),
        )
        .unwrap(),
    )
    .unwrap();
    assert_eq!(detail.len(), 1);
    assert_eq!(detail[0].sightings.len(), 1);
    assert_eq!(detail[0].sightings[0].pubkey, peer());
    assert_eq!(detail[0].sightings[0].seen_at, 100);

    // Holding the author's signature is what makes an event forwardable; an
    // event received without one is held for the user and goes no further.
    let unsigned = note(author(), 2_000, "no signature came with this", Tags::new());
    assert!(command::receive_event(&db, &unsigned, &[peer()], None, &us(), 200).unwrap());

    assert_eq!(
        query::list_events(&db, &offerable()).unwrap(),
        vec![with_media.clone()]
    );

    // Until the signature arrives on a later encounter. The id is an `EventId`
    // rather than a string, so there is no spelling of it that reaches the
    // store as a row that is not there.
    assert!(command::receive_signature(&db, &id(&unsigned), &[9u8; 64], &us()).unwrap());
    assert_eq!(query::list_events(&db, &offerable()).unwrap().len(), 2);

    // Blob hashes are still strings — a sha256 of a file is not an event id —
    // so they keep their check: a key that could not name a row is an error
    // rather than a shrug.
    assert!(query::get_blob(&db, "not a hash").is_err());
    assert_eq!(
        query::get_blob(&db, &BLOB.to_uppercase())
            .unwrap()
            .unwrap()
            .sha256,
        BLOB
    );

    // Our own events need no signature: the authenticated session establishes
    // authorship at the first hop.
    let mine = note(us(), 3_000, "mine", Tags::new());
    assert!(command::publish_event(&db, &mine, &us(), 300).unwrap());
    assert_eq!(query::list_events(&db, &offerable()).unwrap().len(), 3);

    // What negentropy diffs is the same set under the same constraints, in the
    // order it compares.
    let set = SyncSet::from_items(query::list_events(&db, &offerable()).unwrap().iter().map(
        |event| Item {
            timestamp: event.created_at,
            id: event.id,
        },
    ));
    assert_eq!(set.len(), 3);
    assert_eq!(set.iter().next().unwrap().id, with_media.id);

    // Blob transfer: wanted, resumed, completed, and then evicted under the
    // cache ceiling.
    let wanted = query::wanted_blobs(&db, 10).unwrap();
    assert_eq!(wanted.len(), 1);
    assert_eq!(wanted[0].role, BlobRole::Original);

    assert!(command::record_blob_progress(&db, BLOB, 1_024, Some(&[0b0000_0011])).unwrap());
    assert_eq!(
        query::get_blob(&db, BLOB).unwrap().unwrap().stored_bytes,
        1_024
    );
    assert_eq!(query::wanted_blobs(&db, 10).unwrap().len(), 1);

    assert!(command::complete_blob(&db, BLOB, 2_048, 400).unwrap());
    assert!(query::wanted_blobs(&db, 10).unwrap().is_empty());
    assert_eq!(query::cached_bytes(&db).unwrap(), 2_048);

    assert!(command::evict_originals(&db, 4_096).unwrap().is_empty());
    assert_eq!(command::evict_originals(&db, 1_024).unwrap(), [BLOB]);
    assert_eq!(query::cached_bytes(&db).unwrap(), 0);

    // Preferences. A use case that fails leaves nothing behind: bare text is
    // not JSON, and a preference that reads fine but decodes into nothing would
    // silently fall back to a default.
    assert!(command::set_preference(&db, keys::ACCEPT, "lenient", 500).is_err());
    assert!(query::preference(&db, keys::ACCEPT).unwrap().is_none());

    command::set_preference(&db, keys::ACCEPT, r#""lenient""#, 500).unwrap();
    assert_eq!(
        query::preference(&db, keys::ACCEPT).unwrap().as_deref(),
        Some(r#""lenient""#)
    );
    assert_eq!(query::preferences(&db).unwrap().len(), 1);

    // Policy is those preferences read back together, with the document's
    // defaults standing in for the keys nobody has written.
    let policy = query::policy(&db, &us()).unwrap();
    assert_eq!(policy.accept, Scope::Lenient);
    assert_eq!(policy.gossip, Scope::Network);
    assert_eq!(policy.cool_off_minutes, 10);
    assert!(policy.discoverable_times.is_empty());
    assert!(!policy.for_pubkey(peer()).is_blocked());

    command::set_preference(&db, keys::GOSSIP, r#""nothing""#, 500).unwrap();
    assert_eq!(query::policy(&db, &us()).unwrap().gossip, Scope::Nothing);
    assert!(command::clear_preference(&db, keys::GOSSIP).unwrap());

    assert!(command::clear_preference(&db, keys::ACCEPT).unwrap());
    assert!(query::preferences(&db).unwrap().is_empty());

    // Forgetting is by arrival, and takes the event's rows with it.
    assert_eq!(command::forget_events_before(&db, 250).unwrap(), 2);
    assert_eq!(query::list_events(&db, &Query::new()).unwrap().len(), 1);
    assert!(
        query::list_events(
            &db,
            &Query::new().with_filter(Filter::new().add_id(with_media.id))
        )
        .unwrap()
        .is_empty()
    );

    // WAL, so the sidecars sit beside the database. On iOS all three carry the
    // same data-protection class — see docs/storage.md.
    assert!(directory.join("dip.sqlite").exists());
    assert!(directory.join("dip.sqlite-wal").exists());

    fs::remove_dir_all(&directory).unwrap();
}

/// Two stores in one process: two sets of channels, and neither able to see the
/// other's rows.
#[test]
fn two_stores_in_one_process_share_nothing() {
    let hers = database_directory("alice");
    let his = database_directory("bob");

    let _ = fs::remove_dir_all(&hers);
    let _ = fs::remove_dir_all(&his);

    let alice = Db::open(&hers).unwrap();
    let bob = Db::open(&his).unwrap();

    let mut alice_hears = channel::subscribe(&alice);
    let mut bob_hears = channel::subscribe(&bob);

    // A write to one store is announced on that store's channel and no other's.
    let note = note(author(), 1_000, "from alice", Tags::new());
    assert!(command::receive_event(&alice, &note, &[peer()], None, &us(), 100).unwrap());

    match alice_hears.try_recv() {
        Ok(EventChange::Stored(stored)) => assert_eq!(stored.id, note.id),
        other => panic!("expected the stored event, got {other:?}"),
    }
    assert!(
        bob_hears.try_recv().is_err(),
        "a write to one store was announced on the other's channel"
    );

    // And the row is one store's alone, so the same event is new to the other.
    assert_eq!(
        query::list_events(&alice, &Query::new()).unwrap(),
        vec![note.clone()]
    );
    assert!(query::list_events(&bob, &Query::new()).unwrap().is_empty());

    assert!(command::receive_event(&bob, &note, &[peer()], None, &us(), 200).unwrap());
    assert!(matches!(bob_hears.try_recv(), Ok(EventChange::Stored(_))));

    fs::remove_dir_all(&hers).unwrap();
    fs::remove_dir_all(&his).unwrap();
}
