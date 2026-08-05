# Storage

Where events live, who can answer a query, and what happens while the app is asleep.

## The constraint

The view is suspended in the background. If the event store lives only in the view, nothing can be served while backgrounded — which means two phones in two pockets never sync. Passive gossip between backgrounded devices is the core use case, not an enhancement on top of a foreground app.

So the core must hold events and answer queries autonomously.

## SQLite is the source of truth, and it is also a relay

The core's SQLite holds events durably. The view reaches it **through the same relay protocol used for peers**, behind a dedicated URL:

```ts
export const SQLITE_STORAGE_URL = 'sqlite://serendipity.storage/'
```

resolved by a `getAdapter` override that hands the request to the Capacitor plugin:

```ts
getAdapter: url =>
  url === LOCAL_RELAY_URL    ? new LocalAdapter(repository) :
  url === SQLITE_STORAGE_URL ? new SqliteAdapter()          : undefined
```

`SqliteAdapter` marshals `REQ` / `CLOSE` / `EVENT` across the bridge and emits inbound messages via `AdapterEvent.Receive`. The core streams `EVENT` frames as plugin listener callbacks and terminates with `EOSE`, so a large result set arrives incrementally rather than as one enormous bridge payload.

Three URL families speak this protocol, but the view only resolves two of them:

| URL | Backed by | Resolved in | Trust |
| --- | --- | --- | --- |
| `LOCAL_RELAY_URL` | In-memory `Repository` (working set) | View | Ours |
| `SQLITE_STORAGE_URL` | SQLite in the core, via the plugin bridge | View | Ours — already verified |
| `ble://…` | Remote peers | Rust core only | Untrusted; verify, authenticate, apply policy |

The third row is unreachable from TypeScript. Peer sessions start during background wakes and are driven entirely by the core ([`sync.md`](./sync.md#peers-speak-the-relay-wire-protocol)), so there is no `BleAdapter` to construct and no way for view code to address a peer.

## The working set stays

**welshman's reactive layer is synchronous over a `Repository` instance.** `deriveEventsById`, `deriveItemsByKey`, and `makeDeriveEvent` all take `{repository, filters}` and derive synchronously; `getter()` exists specifically to give callbacks and event handlers a synchronous lookup. Removing the in-memory store does not make reads slower — it makes every derived store asynchronous, which means rewriting the reactive layer rather than tuning it.

The working set is the layer the UI is built on. Bridge cost decides how much lives there, not whether it exists: Capacitor marshals plugin payloads as JSON, so the price of residency scales with serialized bytes.

### Hydration is welshman's existing pattern

The canonical chain — `deriveItemsByKey` → `deriveItems` → `getter` → `makeLoadItem` → `makeDeriveItem` — is exactly the mechanism needed, with `SQLITE_STORAGE_URL` in place of a network relay. `makeDeriveItem` calls the loader on each unique key access, `makeLoadItem` collapses concurrent calls for the same key and applies staleness and backoff, and the sync derive falls through to the Repository once the load lands.

One adjustment: `makeLoadItem`'s default staleness window is 3600 s, tuned for network relays. The local store is authoritative and cheap, so it wants a much shorter window — or `makeForceLoadItem` where correctness beats latency.

### Residency policy

Two tiers rather than one decision:

- **Resident always** — replaceable metadata: profiles, follow lists, mute lists, and the user's own events. Small, bounded by the social graph rather than by history, and needed synchronously for mute filtering and rendering any author's name. These are loaded at startup and kept.
- **On demand** — content events, hydrated per view through the loader chain above and evicted under memory pressure.

A full mirror works up to some store size and stops working past it.

## Writes

The same URL list works in the other direction:

```ts
publish({event, relays: [LOCAL_RELAY_URL, SQLITE_STORAGE_URL]})
```

Working set and durable store in one call, and that is the whole list. **Composing does not publish to peers**, because there is no peer to name — propagation happens when an encounter happens, from the store, under policy. Writing to `SQLITE_STORAGE_URL` is what makes an event eligible; the core decides the rest, possibly hours later and possibly while the app is closed.

Events arriving from peers while the app is foregrounded are ingested by the core, so the Repository would otherwise go stale. Keep a live subscription open against the store — a `REQ` with `since: now` over `SqliteAdapter` — and new events flow into the working set the same way network events do in an ordinary nostr client.

## Crossing the boundary

Two pieces of per-event state are not in the event JSON and must be carried alongside it:

- **`verifiedSymbol`.** The core checked the delivery grant on ingest, so the adapter sets `event[verifiedSymbol] = true` before handing events to the Repository. Schnorr verification in JS costs roughly a millisecond per event, so re-checking a 10,000-event hydration would cost ten seconds for no benefit. Omitting it is silent — hydration stays correct and just gets slower as the store grows. This is welshman's flag for "signature already checked", and our events have no signature; it is set because grant verification has already established authenticity.
- **`seen_at`.** Not part of the event and not expressible in an `EVENT` frame. The core sends it in a parallel structure keyed by event id; the adapter attaches it as a symbol property, which keeps it out of JSON serialization and out of anything ever sent to a peer.
- **The delivery grant**, for chunks this device can forward — ones whose author handed them to us directly. One grant per chunk, stored with the sorted id list it commits to, so inclusion proofs can still be produced after retention has evicted some of the events. Never transmitted: forwarding sends a [grant proof](./sync.md#grant-proofs) derived from it instead. Events received at the second hop arrive with no grant and can never be forwarded.

### What `SqliteAdapter` must not do

- **No `AUTH`.** This is local IPC, not a peer. There is no identity to prove and no challenge to issue.
- **No `NEG-*`.** The working set is a deliberate subset of the store; running negentropy between them would converge two things that are meant to differ. Reconciliation targets peers only.
- **No policy enforcement.** Egress policy exists to protect against peers. The view and the store are the same trust domain.

## What the core implements

The store is `rusqlite` inside the core, so this is one implementation rather than one per platform:

- Event storage with indexes on kind, pubkey, `created_at`, `seen_at`, and tags.
- NIP-01 filter matching.
- Id recomputation and **delivery grant verification** on ingest (see [`sync.md`](./sync.md#delivery-grants)): a grant naming us when the sender authored the event, a valid grant proof from the sender otherwise, checked by set membership against the pubkeys the peer authenticated as. Events satisfying neither are never stored.
- Quota counters, `seen_at` assignment, and retention.

The ingest rule is stated once and enforced once, at the only place events enter the system. Nothing untrusted reaches the view, so there is no second copy to drift.

`rusqlite` is built with the bundled amalgamation, so both platforms run one pinned SQLite rather than whatever the OS shipped, and neither `libsqlite3.dylib` nor `android.database.sqlite` is on the path. **There is no storage API in the shell.**

What the shell contributes is small: the database directory, the Keychain or Keystore entry holding the identity key, and the blob directory. The directory is passed in at startup rather than fetched through a callback, since it does not change during a run; the key stays a callback so it can be read on demand and zeroized. See [the call direction](./overview.md#architecture) for why these are traits the core declares rather than platform imports.

On iOS the shell also sets the database's data-protection class, and SQLite's `-wal` and `-shm` sidecars have to carry the same class. A stricter class on any of the three breaks a write during a background wake on a locked phone, which is the failure mode [`AfterFirstUnlock`](./identity.md#the-key-is-readable-while-the-device-is-locked) exists to avoid. The default for app-container files is already the class we want, so the thing to avoid is hardening it later.

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

Store which peer each event arrived from — a column in the core's schema, written at ingest, where the peer identity is known. `Tracker` in `@welshman/net` maps event → source relay URL in the view, but everything it sees over the bridge arrived from `SQLITE_STORAGE_URL`, so it records the store rather than the peer. Provenance is the core's, and it is not part of what crosses.

Provenance drives quota accounting and debugging. It is not surfaced in the UI — see [`privacy.md`](./privacy.md).

## Blobs

Blob bytes are stored outside the event store, keyed by SHA-256 hash. Partial transfers persist with their byte offset so a transfer interrupted on BLE resumes later. See [`media.md`](./media.md).

Media is written to disk unsealed, protected by the platform's data-protection class rather than app-layer encryption. bitchat does the same, and the privacy policy states it plainly.

## Retention

A device that gossips for a year in a busy neighborhood will fill its disk.

Eviction is driven by **`seen_at`, not `created_at`.** Evicting the oldest-authored events would delete exactly the archival material the app exists to surface, moments after discovering it.

Events and blobs have separate budgets. Blobs dominate by volume and evict independently, keeping the event that references them so the timeline entry survives with its tier-0 placeholder.
