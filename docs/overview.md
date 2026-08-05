# Serendipity — Overview

Offline-first nostr social gossip over local transports. This is the overview; each subsystem has its own document.

## What this is

A nostr client whose network is the people physically around you. Events propagate device-to-device over Bluetooth, with no internet dependency in the core loop.

The setting it is designed around is a neighborhood or a town: local gossip propagating between people who might not otherwise talk, and conversations that start because two phones were in the same place.

It differs from its two closest relatives:

- **Manyverse** (SSB) gets offline gossip right but bridges freely over rooms and pubs, and its data model is SSB's append-only log rather than nostr's signed events.
- **bitchat** gets local mesh right but is a chat app, and its "global reach" path bridges distant peers over public relays — precisely what this design excludes.

Serendipity takes Manyverse's sync model, bitchat's transport engineering, nostr's data model, and neither project's bridging.

## Invariants

These are load-bearing.

**I1 — Proximity.** A peer connection is only ever established with a device that is currently physically nearby. Enforced structurally, not by runtime policy: the only way to learn how to reach a peer is to have been in Bluetooth range of them.

**I2 — Progressive enhancement.** Bluetooth is the floor and always works. Any transport above it is an optimization for bandwidth. See [`transport.md`](./transport.md#adding-a-transport-later) for details.

**I3 — Offline-first gossip.** No part of the gossip protocol requires internet: discovery, session establishment, sync, storage, signing and forwarding all work with no network. The app has direct access to the user's key ([`identity.md`](./identity.md#key-custody)), so no network is needed for signing.

**I4 — Data outlives connections.** Sync is store-and-forward. Tearing down a link does not discard what was synced over it, and events propagate transitively through people who move. This means I1 constrains *connections*, not *information* — see [`privacy.md`](./privacy.md).

**I5 — Bounded reach.** An event travels at most two hops from its author: to someone the author met, and one step beyond. Enforced cryptographically by delivery grants ([`sync.md`](./sync.md#delivery-grants)), not by policy.

## Non-goals

- No relay fallback, no hole punching, no global discovery, no DHT.
- No bridging of peers who have not been co-present.
- **No unbounded flooding.** Reach is capped at two hops by construction, not by a policy each device is trusted to apply. See [`sync.md`](./sync.md#delivery-grants).
- **No interoperability with the open nostr network.** Content events carry no signature, so relays reject them and no existing client can read them. See [`identity.md`](./identity.md#events-are-not-signed-grants-are).
- Not a chat app. Rich nostr event types, including media, are first-class.

## The stack

```
┌──────────────────────────────────────────────────────┐
│  Svelte UI · welshman app layer          (webview)   │
├──────────────────────────────────────────────────────┤
│  Capacitor bridge — streamed EVENT frames            │
├──────────────────────────────────────────────────────┤
│  Sync — relay wire protocol, grants      sync.md     │
│  Storage — SQLite, seen_at, blobs        storage.md  │
│  Discovery — connect, identify, beat     discovery.md│
│  Transport — framing, Noise XX           transport.md│
│                                        (Rust core)   │
├──────────────────────────────────────────────────────┤
│  BLE radio · Keychain / Keystore · lifecycle         │
│                                    (Swift / Kotlin)  │
└──────────────────────────────────────────────────────┘
```

| Document | Covers |
| --- | --- |
| [`discovery.md`](./discovery.md) | Advertisement, connection scheduling, identification, session lifecycle, heartbeat and teardown |
| [`transport.md`](./transport.md) | BLE link layer and framing, Noise XX, the bandwidth ceiling, the seam for a second transport |
| [`sync.md`](./sync.md) | Relay wire protocol as the peer protocol, delivery grants and the two-hop cap, reconciliation, sync policy, mute, quotas |
| [`storage.md`](./storage.md) | Native SQLite as source of truth and relay, `seen_at`, background serving, retention |
| [`media.md`](./media.md) | Blob tiers, transfer, fetch policy, quotas |
| [`identity.md`](./identity.md) | Keys, unsigned events and grants, key custody, login with device, backup |
| [`privacy.md`](./privacy.md) | Threat model, what leaks, what users will wrongly assume |
| [`ui.md`](./ui.md) | Component framework, design tokens, the conventions the linter enforces |
| [`nip-p2p-auth.md`](./nip-p2p-auth.md) | Peer authentication — the NIP-42 additions covering transports without URLs |

## Client stack

Four languages, split along [the plugin boundary](#the-plugin-boundary) below.

- **TypeScript / Svelte 5** — the webview: UI, reactivity, the social graph.
- **Tailwind 4 / shadcn-svelte** — design tokens and vendored components. See [`ui.md`](./ui.md).
- **welshman** — nostr app layer: events, `Repository`, feeds, web of trust. Not the peer protocol — see [the plugin boundary](#the-plugin-boundary).
- **nostr-tools** — NIP-19 encoding for display.
- **Vite 8** — bundles the web assets to `dist/`, which the native shells load.
- **Capacitor 8** — native shell, and the JSON bridge between webview and plugin.
- **Rust** — the core: protocol, crypto, store, session state. Everything below the UI that has to run while backgrounded.
- **uniffi** — Swift and Kotlin bindings to that core.
- **Swift / Kotlin** — the platform shell: radio, secure storage, lifecycle, bridge.
- **CoreBluetooth** — iOS dual-role GATT, background modes, state restoration.
- **`android.bluetooth`** — Android advertiser, scanner, GATT server and client.
- **BLE GATT** — the floor transport, with [our own framing](./transport.md#framing) over one characteristic.
- **`snow`** — Noise XX: Curve25519 / ChaCha20-Poly1305 / SHA-256.
- **`rusqlite`** — durable event store, indexes, NIP-01 filter matching.
- **`secp256k1`** — signing and verifying grants and auth events; **`k256`** for the [grant proof](./sync.md#grant-proofs), which needs explicit group arithmetic that the binding does not expose.

### The plugin boundary

The webview is suspended in the background. **Everything that must survive backgrounding lives in the native plugin.** This is the central constraint on the whole app.

What decides where a piece of work lives is whether it happens at *encounter time*, not which layer it belongs to conceptually. A peer appears while both phones are in pockets. Whatever has to run then runs with no webview, so it is native, whatever it looks like.

| Rust core (via uniffi) | Platform shell (Swift / Kotlin) | Webview (TypeScript) |
| --- | --- | --- |
| Relay protocol, both halves | BLE advertise, scan, GATT both roles | UI, all of it |
| Noise handshake, framing codec | Keychain / Keystore | Social graph, WoT, scope computation |
| Session state machine, heartbeat | Background lifecycle, state restoration | Feed construction, rendering, composition |
| Signing — auth events and grants | Paths and data-protection classes | Working-set `Repository` |
| Merkle trees, grant proofs | Capacitor bridge marshalling | Onboarding, backup and transfer UI |
| Reconciliation — GCS, negentropy | | |
| SQLite store, filter matching | | |
| Policy application, quotas | | |
| Blob transfer, assembly, verification | | |

Native holds the durable store and serves peers autonomously using a policy snapshot compiled by the webview. The webview reads its own store over the same relay protocol, through a `SQLITE_STORAGE_URL` adapter that crosses the Capacitor bridge. The in-memory `Repository` stays as the working set because welshman's reactive layer derives from it synchronously. See [`storage.md`](./storage.md).

#### The webview does not sign

Exactly two things need the identity key, and both happen at encounter time: **kind 22242 auth events**, one per direction per session, and **delivery grants**, one per chunk handed over. Everything else is key-free — content events carry no signature at all, [grant proofs](./sync.md#grant-proofs) demonstrate knowledge of a grant's signature rather than of our key, and verification is public.

So signing is native, and the key lives where native can read it, which is what forces custody down to one model ([`identity.md`](./identity.md#key-custody)). The webview composes unsigned events and hands them across the bridge; nothing on the gossip path passes through it. The one exception is deliberate and user-initiated: [promoting your own post](./identity.md#events-are-not-signed-grants-are) to the open network calls a `signEvent` op on the plugin, which is also what backs the thin `ISigner` that keeps welshman's session model working.

#### Why the core is Rust

The alternative is Swift and Kotlin implementing the same protocol twice.

- **The crypto has to be right once, not twice.** The [grant proof](./sync.md#grant-proofs) is a Cramer–Damgård–Schoenmakers OR-proof where a mistake is silent rather than loud — it produces a proof that verifies and proves nothing. Two independent implementations of that is two chances to get it silently wrong, and known-answer tests that have to be maintained in parallel.
- **Two implementations of one wire protocol diverge.** The relay protocol, the Merkle construction, negentropy and the GCS filter all have to agree byte-for-byte with a *peer*, which is another copy of this app on the other platform. Divergence shows up as a sync failure between an iPhone and an Android in someone's pocket, which is the hardest possible place to observe it.
- **The platform layer is genuinely platform-specific, and small.** CoreBluetooth and `android.bluetooth` are not convergent enough to share, and they are exactly the part with no cryptographic subtlety. That is the right seam.

The cost is a cross-compiled toolchain: `cargo` builds the core for every iOS and Android target and `uniffi-bindgen` generates the bindings, both before `cap sync`.

## Decisions log

| Decision | Rationale |
| --- | --- |
| Proximity enforced by configuration, not runtime checks | A heartbeat that tears down a wrong connection is reactive; disabling discovery and relay means it can't form. |
| BLE is the only address-lookup service | The only way to learn an address is to have been in radio range. Makes I1 structural. |
| BLE only; no second transport | A LAN upgrade serves only peers on one access point, can't run while backgrounded, and drags back the discovery stack I1 exists to exclude. |
| The transport seam stays open anyway | Scheme-dispatched adapters, and reachability exchanged only over the authenticated BLE channel. Keeps I1 structural for whatever arrives. |
| L2CAP is the reserved bandwidth upgrade | Same radio, same connection, same Noise session — a data plane under the existing `ble://` peer, so it needs no scheme, no discovery, and no change to I1 or to the sync layer. |
| Own BLE framing, not `iroh-ble-transport` | AGPL-3.0 vs. app store distribution, no legal review planned. Also drops the 1200-byte datagram floor and L2CAP fallback. |
| Heartbeat is liveness, not authorization | Proximity already guaranteed structurally, so teardown can be lenient and never kills a live transfer. |
| Advertisement carries no identity | Not a choice — iOS strips background advertisement payload. Identification is necessarily post-connect. |
| Consent is compiled, not prompted | Encounters happen with no user present. A prompt that cannot be shown has to resolve to a decision made earlier, so the snapshot carries paired Noise keys plus any open discoverable window. |
| Nostr relay wire protocol as the peer protocol | A specified protocol with prior art on both sides, and policy compiles to filters. Reusing it removes a protocol from the project rather than adding one. |
| Both halves of the peer protocol are native | Sync starts when a peer appears, which is when the webview does not exist. A TypeScript implementation could only run in the foreground, which is not the case the app is for. |
| The Rust core holds everything below the UI | The alternative is implementing one wire protocol and one silent-failure crypto construction twice, and having them disagree between an iPhone and an Android in two pockets. |
| Mutual NIP-42 bound to transport identity | Without the binding, any peer you authenticate to can impersonate you to every other peer. |
| Auth events are signed; content events are not | NIP-42 needs a verifiable signature. Content authenticity comes from the grant instead. |
| Content events carry no signature | An unsigned event is invalid everywhere, so a leak is rejected by relays rather than stored. |
| Two-hop cap via delivery grants | Makes the social topology structural. A grant names one recipient and cannot be forged, so the bound needs no honest-node assumption. |
| One grant per synchronized chunk, over a Merkle root | Handing a peer *n* events costs one signature and one grant on the wire, not *n* of each. At BLE bandwidth the bytes matter as much as the signatures. |
| Grant is detached, not folded into the event id | Hashing the recipient in would give one post a different id per recipient, and reconciliation diffs id sets — it could never converge. Also breaks threading and dedup. |
| Ids are plain NIP-01 hashes | Nothing app-specific in serialization, so `@welshman/util` is used unmodified and a promoted post keeps its id and its replies. |
| The nsec is the only custody model | Authenticating a session needs a signature at encounter time, with no webview and possibly no network. A signer the app cannot reach then fails the core loop rather than degrading. |
| The webview never signs on the gossip path | Both signed kinds are produced at encounter time. Moving signing native also means the key never crosses the bridge except in the export flows that exist to move it. |
| Identity key readable while the device is locked | `AfterFirstUnlock`. A locked phone that cannot sign cannot authenticate, and a peer that cannot authenticate can neither send nor receive — which is pocket-to-pocket gossip, the core case. |
| Key stored as bytes, not a hardware key handle | Secure Enclave and Android Keystore do P-256, not secp256k1. There is no hardware-backed option for a nostr identity key; the platform protects the bytes at rest instead. |
| Grants are nostr events, not bare signatures | Ordinary NIP-01 serialization, so grants and auth events share one signing and verification path, and a grant is inspectable as an event. |
| Login with device for multi-device | Reuses the proximity stack. Needs a short authentication string — the user has no prior knowledge of the target's key. |
| Backup is an exported text file, optionally NIP-49 encrypted | Follows Flotilla's `KeyDownload`. Multi-device is the happy path; most users have one phone. Instructions ship in the file. |
| One-shot GCS on connect, negentropy only if the session lasts | Session duration, not latency: a drive-by lasts seconds and a multi-round negotiation may never converge. |
| Reconciliation scope is `created_at`; ordering is `seen_at` | `seen_at` is local and private, so it can't define a scope both peers can compute. |
| `seen_at` set once, never updated, never transmitted | Duplicate arrivals must not churn the recently-discovered view; encounter times are movement data. |
| Sync scope is social distance, not a boolean | On/off is either too leaky or useless. Manyverse's `hops=2` is the precedent; "hops" is reserved here for delivery distance under I5. |
| Mute and block conflated, kind 10000 | One concept: nothing to do with this person. Blocks ingest, egress, transitive gossip, and peering; purges stored events. |
| Media in three tiers | BLE at 5–15 KB/s carries previews, not originals. Tier 0 makes the timeline render instantly, and tier 2 never transfers automatically. |
| Native SQLite is source of truth and speaks the relay protocol | Background serving is non-negotiable; the relay protocol keeps the app layer unchanged. |
| `SQLITE_STORAGE_URL` adapter for webview → native reads | Same protocol as peers, resolved by `getAdapter`. Hydration is welshman's existing `makeLoadItem` chain pointed at a local URL. |
| Working set is kept, not replaced by direct SQLite reads | welshman derives from a `Repository` synchronously. Dropping it makes every derived store async — an app-layer rewrite, not a perf tradeoff. |
| Capacitor over Tauri and KMP | The UI is TypeScript and stays TypeScript; the hard problems are background iOS, where Capacitor has the most prior art. The shared core is Rust either way, so the shell is chosen on its webview and lifecycle story alone. |
| shadcn-svelte, vendored rather than depended on | The surfaces are unusual — hop badges, consent gates, peers that vanish mid-session. Owning the component source makes those edits rather than fights with someone's variant API. See [`ui.md`](./ui.md). |
| Design values live only in Tailwind tokens | shadcn components are written against `shadow-sm` and `rounded-lg`, so redefining the scales restyles the vendored set without editing it, and the linter can then reject any hard-coded value. |
| Fonts and icons bundled, never fetched | A CDN request on first paint fails exactly where the app is meant to work — I3. |
