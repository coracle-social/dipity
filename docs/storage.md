# Storage

Where events live, who can answer a query, and what happens while the app is asleep.

## SQLite is the source of truth, and it is also a relay

The core's SQLite holds every event. The view reaches it **through the same relay protocol used for peers** — `REQ` / `EVENT` / `EOSE` / `CLOSE` — marshalled across the Capacitor bridge. The core streams `EVENT` frames as plugin listener callbacks and terminates with `EOSE`, so a large result set arrives incrementally rather than as one payload.

There is one store, and the view does not hold a second one:

| Store | Backed by | Reached from | Trust |
| --- | --- | --- | --- |
| The local store | SQLite in the core | View, over the bridge | Ours — already verified |
| `ble://…` | Remote peers | Core only | Untrusted; verify, authenticate, apply policy |

The second row is unreachable from TypeScript. Peer sessions start during background wakes and are driven entirely by the core ([`sync.md`](./sync.md#peers-speak-the-relay-wire-protocol)), so there is no way for view code to address a peer.

## The controller layer

Between components and the bridge sits a **controller layer**, in `src/lib/data/`. It owns every query against the core and the caches over them, and it is the only thing in the view that speaks the protocol.

`deriveEvents(filter)` returns a store backed by an open `REQ`. The core matches the filter against events as they are ingested and pushes them, so nothing in the view has to work out which open queries a new event affects. Closing the store sends `CLOSE`.

Caches are per use case. Profiles, follow lists and mute lists are small, bounded by the social graph rather than by history, and read synchronously while rendering, so the controller keeps them in maps hydrated at startup and current through their own subscriptions. Content events are not cached: a view opens a query and drops it when it goes away. Aggregations are computed in the controller, cached, and debounced.

A few thousand events per query is the working size. Capacitor marshals plugin payloads as JSON, so cost scales with serialized bytes.

## Writes

Composing sends the event to the core, which stores it. **Composing does not publish to peers**, because there is no peer to name — propagation happens when an encounter happens, from the store, under policy. Writing to the store is what makes an event eligible; the core decides the rest, possibly hours later and possibly while the app is closed.

Events arriving from peers reach the view the same way anything else does: an open `REQ` matches them on ingest and the controller's stores update. There is no second copy to keep current.

## Crossing the boundary

`seen_at` is not part of the event and not expressible in an `EVENT` frame. The core sends it in a parallel structure keyed by event id, and the controller carries it alongside. It never becomes a property of an event object, which is what keeps it out of anything serialized towards a peer.

Events arrive verified. The core checked the delivery grant on ingest, so the view has no verification step.

### Filtering by `seen_at`

The main view is ordered by `seen_at`, which no NIP-01 filter can express. The controller sends **`since_seen` and `until_seen` alongside the filter, never inside it.**

The filter type is the one the gossip protocol uses, and it stays exactly NIP-01. A seen-window is a separate field on the local request, and the peer-facing message has no field to put one in — so a `seen_at` bound cannot be serialized towards a peer even by mistake. See [`privacy.md`](./privacy.md).

### What the local protocol must not do

- **No `AUTH`.** This is local IPC, not a peer. There is no identity to prove and no challenge to issue.
- **No `NEG-*`.** Reconciliation targets peers. There is one store, so there is nothing to reconcile against.
- **No policy enforcement.** Egress policy exists to protect against peers. The view and the store are the same trust domain.

## What the core implements

The store is `rusqlite` inside the core, so this is one implementation rather than one per platform:

- Event storage with indexes on kind, pubkey, `created_at`, `seen_at`, and tags.
- NIP-01 filter matching.
- Id recomputation and **delivery grant verification** on ingest (see [`sync.md`](./sync.md#delivery-grants)): a grant naming us when the sender authored the event, a valid grant proof from the sender otherwise, checked by set membership against the pubkeys the peer authenticated as. Events satisfying neither are never stored.
- Replaceable and addressable event semantics, and kind 5 deletions. The core serves peers, so a superseded profile must not reach one either.
- The delivery grant for each chunk this device can forward, stored with the sorted id list it commits to, so inclusion proofs survive retention evicting some of the events ([`sync.md`](./sync.md#grant-proofs)).
- Quota counters, `seen_at` assignment, and retention.

The ingest rule is stated once and enforced once, at the only place events enter the system. Nothing untrusted reaches the view, so there is no second copy to drift.

`rusqlite` is built with the bundled amalgamation, so both platforms run one pinned SQLite rather than whatever the OS shipped, and neither `libsqlite3.dylib` nor `android.database.sqlite` is on the path. **There is no storage API in the shell.**

What the shell contributes is small: the database directory, the Keychain or Keystore entry holding the identity key, and the blob directory. The directory is passed in at startup rather than fetched through a callback, since it does not change during a run; the key stays a callback so it can be read on demand and zeroized. See [the call direction](./overview.md#architecture) for why these are traits the core declares rather than platform imports.

On iOS the shell also sets the database's data-protection class, and SQLite's `-wal` and `-shm` sidecars have to carry the same class. A stricter class on any of the three breaks a write during a background wake on a locked phone, which is the failure mode [`AfterFirstUnlock`](./keys.md#signing-happens-at-encounter-time-in-the-background) exists to avoid. The default for app-container files is already the class we want, so the thing to avoid is hardening it later.

## Policy lives in preferences

Sync scope, mute, consent and the per-peer limits are user preferences, stored durably on the device alongside the events. The core reads them when a peer appears and resolves them itself: "within social distance 2" is a traversal over the follows and mutes already in the store, not an author set computed elsewhere and handed down.

The view edits those preferences and computes nothing the core depends on. A device that has not been foregrounded in weeks therefore serves peers under exactly the policy the user last set, evaluated against whatever the store currently holds.

## Background execution

CoreBluetooth background wakes are short bursts, not continuous execution. That fits the design: small `REQ` responses are served within a burst, and bulk transfers are deferred to the foreground or to a later encounter. It is consistent with the media tiering in [`media.md`](./media.md).

## `seen_at`

Every stored event carries `seen_at`: when **this device** first received it. It drives the "recently discovered" view, which is the app's primary surface.

Rules:

- **Set once, on first insert. Never updated.** Receiving the same event again from a second peer must not move it — otherwise the recently-discovered view churns as duplicates arrive.
- **Never transmitted.** It is metadata about the user's movements and encounters. It is not part of the event and must never appear in anything served to a peer. See [`privacy.md`](./privacy.md).
- **Indexed.** It is the primary sort key for the main view.
- **Distinct from `created_at`.** An event authored three years ago and discovered five minutes ago sorts to the top — the point of the app, not an anomaly to correct.

`seen_at` also changes reconciliation, which cannot use it — see [`sync.md`](./sync.md#seen_at-changes-what-recent-means).

## Provenance

Store which peer each event arrived from — a column in the core's schema, written at ingest, where the peer identity is known. It stays there. Provenance is not part of what crosses the bridge, and the view has no way to ask for it.

Provenance drives quota accounting and debugging. It is not surfaced in the UI — see [`privacy.md`](./privacy.md).

## Blobs

Blob bytes are stored outside the event store, keyed by SHA-256 hash. Partial transfers persist with their byte offset so a transfer interrupted on BLE resumes later. See [`media.md`](./media.md).

Media is written to disk unsealed, protected by the platform's data-protection class rather than app-layer encryption. bitchat does the same, and the privacy policy states it plainly.

## Retention

A device that gossips for a year in a busy neighborhood will fill its disk.

Eviction is driven by **`seen_at`, not `created_at`.** Evicting the oldest-authored events would delete exactly the archival material the app exists to surface, moments after discovering it.

Events and blobs have separate budgets. Blobs dominate by volume and evict independently, keeping the event that references them so the timeline entry survives with its tier-0 placeholder.
