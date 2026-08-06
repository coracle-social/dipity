# Storage

Where events live, who can answer a query, and what happens while the app is asleep.

## The schema

Four tables in the core's SQLite:

| Table | Holds |
| --- | --- |
| `events` | The events themselves. |
| `event_tags` | One row per tag, so NIP-01 tag filters are an index lookup. |
| `event_fts` | Full-text index over content, for search. |
| `event_provenance` | `event_id`, `seen_at`, `peer_pubkey`. Unique on (`event_id`, `peer_pubkey`). |

**Provenance** is that last table: which peers an event has been seen from, and when each of them first handed it over. A row is never updated, so re-gossip of an event a peer has already given us is a no-op, and the table grows with distinct pairs rather than with gossip volume.

## The read boundary

The view does not speak the relay protocol. That protocol exists for peers, and can only ask for events.

Instead the bridge exposes a **query method built for this store**: it filters on the event fields a NIP-01 filter covers, and additionally on `seen_at` and `peer_pubkey`, and it returns each event **with** its provenance attached. Seen time and peer are first-class in both the query and the result.

A query is live. The controller registers it rather than asking once, and the core pushes every change that affects the result: events newly matching the filter, deletions, an addressable event superseded by a newer one, and a new peer appearing on an event already in the set.

Three of those four are things a relay cannot express. It only ever pushes events, and has no way to say that one is gone, has been replaced, or has arrived from someone new.

The gossip filter stays exactly NIP-01. It is a different type reached by a different call, so there is no shared filter with optional local-only fields for someone to populate by mistake, and nothing on the peer path has a field for `seen_at` at all.

## The controller layer

Between components and the bridge sits a **controller layer**, in `src/lib/data/`. It owns every query against the core and the caches over them, and it is the only thing in the view that talks to the bridge.

Caches are per use case. Profiles, follow lists and mute lists are small, bounded by the social graph rather than by history, and read synchronously while rendering, so the controller keeps them in maps hydrated at startup. Content events are not cached: a view issues a query and drops the result when it goes away. Aggregations are computed in the controller, cached, and debounced.

## Writes

Composing sends the event to the core, which stores it. **Composing does not publish to peers**, because there is no peer to name — propagation happens when an encounter happens, from the store, under policy. Writing to the store is what makes an event eligible; the core decides the rest, possibly hours later and possibly while the app is closed.

Events arriving from peers land in the same store the view reads, so there is no second copy to keep current. Events published to storage (either gossiped from peers or created by the user) are immediately picked up for replication to connected peers.

## What the core implements

The store is `rusqlite` inside the core, so this is one implementation rather than one per platform:

- The four tables above, with indexes on kind, pubkey and `created_at`, and on `seen_at` and `peer_pubkey`.
- NIP-01 filter matching for peers, and the local query method for the view.
- Full-text search over `event_fts`.
- Id recomputation and **delivery grant verification** on ingest (see [`sync.md`](./sync.md#delivery-grants)): a grant naming us when the sender authored the event, a valid grant proof from the sender otherwise, checked by set membership against the pubkeys the peer authenticated as. Events satisfying neither are never stored.
- Replaceable and addressable event semantics, and kind 5 deletions. The core serves peers, so a superseded profile must not reach one either.
- The delivery grant for each chunk this device can forward, stored with the sorted id list it commits to, since chunk membership cannot be recovered from the events themselves ([`sync.md`](./sync.md#grant-proofs)).
- Quota counters, and a provenance row the first time each peer hands over an event.

The ingest rule is stated once and enforced once, at the only place events enter the system. Nothing untrusted reaches the view.

`rusqlite` is built with the bundled amalgamation, so both platforms run one pinned SQLite rather than whatever the OS shipped, and neither `libsqlite3.dylib` nor `android.database.sqlite` is on the path. **There is no storage API in the shell.**

What the shell contributes is small: the database directory, the Keychain or Keystore entry holding the identity key, and the blob directory. The directory is passed in at startup rather than fetched through a callback, since it does not change during a run; the key stays a callback so it can be read on demand and zeroized. See [the call direction](./overview.md#architecture) for why these are traits the core declares rather than platform imports.

On iOS the shell also sets the database's data-protection class, and SQLite's `-wal` and `-shm` sidecars have to carry the same class. A stricter class on any of the three breaks a write during a background wake on a locked phone, which is the failure mode [`AfterFirstUnlock`](./keys.md#signing-happens-at-encounter-time-in-the-background) exists to avoid. The default for app-container files is already the class we want, so the thing to avoid is hardening it later.

## Policy lives in preferences

Sync scope, mute, consent and the per-peer limits are user preferences, stored durably on the device alongside the events. The core reads them when a peer appears and resolves them itself: "within social distance 2" is a traversal over the follows and mutes already in the store, not an author set computed elsewhere and handed down.

The view edits those preferences and computes nothing the core depends on. A device that has not been foregrounded in weeks therefore serves peers under exactly the policy the user last set, evaluated against whatever the store currently holds.

## Background execution

CoreBluetooth background wakes are short bursts, not continuous execution. That fits the design: small `REQ` responses are served within a burst, and bulk transfers are deferred to the foreground or to a later encounter.

## `seen_at`

When **this device** first saw an event: the earliest `seen_at` across its [provenance](#provenance) rows.

- **Derived, not stored.** Rows are written once and never updated, so the earliest one cannot move. Seeing the same event again — from the same peer or a new one — cannot change it.
- **Never transmitted.** It is metadata about the user's movements and encounters, not part of the event, and must never appear in anything served to a peer. See [`privacy.md`](./privacy.md).
- **Distinct from `created_at`.** An event authored long ago and seen five minutes ago is new to this device, which is the point of the app rather than an anomaly to correct.

`seen_at` also changes reconciliation, which cannot use it — see [`sync.md`](./sync.md#seen_at-changes-what-recent-means).

## Provenance

Every peer an event has been seen from, each with the first time they handed it over — the `event_provenance` rows for that event. Accumulated across the store, they are a connectivity graph.

A second copy from a peer already on the list is a no-op. A second copy from a new one is the entire signal.

Provenance is never transmitted. It is not part of an event, has no place in an `EVENT` frame, and like [`seen_at`](#seen_at) it records who the user was physically near and when. It also drives quota accounting.

## Blobs

Blob bytes are stored outside the event store, keyed by SHA-256 hash. Partial transfers persist with their byte offset so a transfer interrupted on BLE resumes later. See [`media.md`](./media.md).

Media is written to disk unsealed, protected by the platform's data-protection class rather than app-layer encryption. See [`privacy.md`](./privacy.md#what-we-do-not-defend-against).
