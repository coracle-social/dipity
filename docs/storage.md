# Storage

Where events live, who can answer a query, and what happens while the app is asleep. See [`overview.md`](./overview.md).

## The constraint

The webview is suspended in the background. If the event store lives only in the webview, nothing can be served while backgrounded — which means two phones in two pockets never sync. Passive gossip between backgrounded devices is the core use case, not an enhancement on top of a foreground app.

So the native side must hold events and answer queries autonomously.

## SQLite is the source of truth, and it is also a relay

Native SQLite holds events durably. The webview reaches it **through the same relay protocol used for peers**, behind a dedicated URL:

```ts
export const SQLITE_STORAGE_URL = 'sqlite://serendipity.storage/'
```

resolved by a `getAdapter` override that hands the request to the Capacitor plugin:

```ts
getAdapter: url =>
  url === SQLITE_STORAGE_URL     ? new SqliteAdapter()    :
  url.startsWith('ble://')       ? new BleAdapter(url)    :
  url.startsWith('iroh://')      ? new IrohAdapter(url)   : undefined
```

`SqliteAdapter` marshals `REQ` / `CLOSE` / `EVENT` across the bridge and emits inbound messages via `AdapterEvent.Receive`. Native streams `EVENT` frames as plugin listener callbacks and terminates with `EOSE`, so a large result set arrives incrementally rather than as one enormous bridge payload.

That gives three URL families, all speaking one protocol:

| URL | Backed by | Trust |
| --- | --- | --- |
| `LOCAL_RELAY_URL` | In-memory `Repository` (working set) | Ours |
| `SQLITE_STORAGE_URL` | Native SQLite, via the plugin bridge | Ours — already verified |
| `ble://…`, `iroh://…` | Remote peers | Untrusted; verify, authenticate, apply policy |

## The working set stays

It is worth being explicit about why, because "just query SQLite for everything" looks simpler.

**welshman's reactive layer is synchronous over a `Repository` instance.** `deriveEventsById`, `deriveItemsByKey`, and `makeDeriveEvent` all take `{repository, filters}` and derive synchronously; `getter()` exists specifically to give callbacks and event handlers a synchronous lookup. Removing the in-memory store does not make reads slower — it makes every derived store asynchronous, which means rewriting the reactive layer rather than tuning it.

So the working set is not a cache we could drop if the bridge turned out cheap. It is the layer the UI is built on. What the bridge cost actually decides is *how much lives there*, not *whether it exists*.

### Hydration is welshman's existing pattern

The canonical chain — `deriveItemsByKey` → `deriveItems` → `getter` → `makeLoadItem` → `makeDeriveItem` — is exactly the mechanism needed, with `SQLITE_STORAGE_URL` in place of a network relay. `makeDeriveItem` calls the loader on each unique key access, `makeLoadItem` collapses concurrent calls for the same key and applies staleness and backoff, and the sync derive falls through to the Repository once the load lands.

One adjustment: `makeLoadItem`'s default staleness window is 3600 s, tuned for network relays. The local store is authoritative and cheap, so it wants a much shorter window — or `makeForceLoadItem` where correctness beats latency.

### Residency policy

Two tiers rather than one decision:

- **Resident always** — replaceable metadata: profiles, follow lists, mute lists, and the user's own events. Small, bounded by the social graph rather than by history, and needed synchronously for WoT computation, mute filtering, and rendering any author's name. These are loaded at startup and kept.
- **On demand** — content events, hydrated per view through the loader chain above and evicted under memory pressure.

A full mirror is fine early and stops being fine at some store size. Starting with the split costs little and avoids a migration later.

## Writes

The same URL list works in the other direction:

```ts
publish({event, relays: [LOCAL_RELAY_URL, SQLITE_STORAGE_URL]})
```

Working set and durable store in one call, with peers appended when the event should also propagate.

Events arriving from peers while the app is foregrounded are ingested by native, so the Repository would otherwise go stale. Keep a live subscription open against the store — a `REQ` with `since: now` over `SqliteAdapter` — and new events flow into the working set the same way network events do in an ordinary nostr client.

## Crossing the boundary

Two pieces of per-event state are not in the event JSON and must be carried alongside it:

- **`verifiedSymbol`.** Native checked the delivery grant on ingest. The adapter sets `event[verifiedSymbol] = true` before handing events to the Repository. Schnorr verification in JS costs roughly a millisecond per event, so re-checking a 10,000-event hydration would cost ten seconds for no benefit. Omitting it is silent — hydration stays correct and just gets slower as the store grows. Note this is welshman's flag for "signature already checked", and our events have no signature; it is set because grant verification has already established authenticity.
- **`seen_at`.** Not part of the event and not expressible in an `EVENT` frame. Native sends it in a parallel structure keyed by event id; the adapter attaches it as a symbol property, which keeps it out of JSON serialization and out of anything ever sent to a peer.
- **The delivery grant**, for chunks this device can forward — ones whose author handed them to us directly. One grant per chunk, stored with the sorted id list it commits to, so inclusion proofs can still be produced after retention has evicted some of the events. Never transmitted: forwarding sends a [grant proof](./sync.md#grant-proofs) derived from it instead. Events received at the second hop arrive with no grant and can never be forwarded.

### What `SqliteAdapter` must not do

- **No `AUTH`.** This is local IPC, not a peer. There is no identity to prove and no challenge to issue.
- **No `NEG-*`.** The working set is a deliberate subset of the store; running negentropy between them would converge two things that are meant to differ. Reconciliation targets peers only.
- **No policy enforcement.** Egress policy exists to protect against peers. The webview and the store are the same trust domain.

## Measuring the bridge

The residency split above is a starting guess. The number that settles it is bridge throughput for a realistic hydration — Capacitor marshals plugin payloads as JSON, so cost scales with serialized bytes, and Android's `JavascriptInterface` has historically been the worse of the two.

Benchmark it during build step 3, once native SQLite exists and before the UI depends on a residency assumption: stream 1k, 10k, and 100k events across the bridge and record wall-clock and peak memory on a mid-range Android device, not just a simulator. Streaming via listener callbacks is what makes the large cases survivable; the benchmark should use the real path.

### What native implements

Bounded, and deliberately mechanical:

- Event storage with indexes on kind, pubkey, `created_at`, `seen_at`, and tags.
- NIP-01 filter matching. Simple semantics, roughly 500 lines per platform.
- Id recomputation and **delivery grant verification** on ingest (see [`sync.md`](./sync.md#delivery-grants)) — libsecp256k1 is available on both platforms. Native applies the same rule as the webview: a grant naming us when the sender authored the event, a valid grant proof from the sender otherwise, checked against the transport-authenticated peer. Events satisfying neither are never stored.
- Quota counters.

### Native gets a compiled policy, not policy logic

Native does not compute the web of trust. The webview does that while foregrounded and hands down a **materialized author set** plus limits. Native's job is to apply it.

The snapshot goes stale while backgrounded, so it carries a max age. After some days offline the store falls back to explicit follows only, rather than serving under a graph that may have changed.

### Background execution

CoreBluetooth background wakes are short bursts, not continuous execution. That fits the design: small `REQ` responses are served within a burst, and bulk transfers are deferred to the foreground or to an iroh upgrade. It is consistent with the media tiering in [`media.md`](./media.md).

## `seen_at`

Every stored event carries `seen_at`: when **this device** first received it. It drives the "recently discovered" view, which is the app's primary surface.

Rules:

- **Set once, on first insert. Never updated.** Receiving the same event again from a second peer must not move it — otherwise the recently-discovered view churns as duplicates arrive.
- **Never transmitted.** It is metadata about the user's movements and encounters. It is not part of the event and must never appear in anything served to a peer. See [`privacy.md`](./privacy.md).
- **Indexed.** It is the primary sort key for the main view.
- **Distinct from `created_at`.** An event authored three years ago and discovered five minutes ago sorts to the top. This is the point of the app, not an anomaly to correct.

`seen_at` also changes reconciliation, which cannot use it — see [`sync.md`](./sync.md#seen_at-changes-what-recent-means).

## Provenance

Store which peer each event arrived from. `Tracker` in `@welshman/net` already maps event → source relay URL, and peer URLs (`ble://…`, `iroh://…`) slot in unchanged.

Provenance drives quota accounting and debugging. It is not surfaced in the UI: "discovered near X" carries obvious privacy implications, and exposing it is a deliberate product decision rather than a side effect of having the data.

## Blobs

Blob bytes are stored outside the event store, keyed by SHA-256 hash. Partial transfers persist with their byte offset so a transfer interrupted on BLE resumes later, possibly over a different transport. See [`media.md`](./media.md).

Media is written to disk unsealed, protected by the platform's data-protection class rather than app-layer encryption. bitchat does the same, and the privacy policy states it plainly.

## Retention

A device that gossips for a year in a dense city will fill its disk.

Eviction is driven by **`seen_at`, not `created_at`.** Evicting the oldest-authored events would delete exactly the archival material the app exists to surface, moments after discovering it.

Events and blobs have separate budgets. Blobs dominate by volume and evict independently, keeping the event that references them so the timeline entry survives with its tier-0 placeholder.
