# Serendipity — Overview

Offline-first nostr social gossip over local transports. This is the overview; each subsystem has its own document.

## What this is

A nostr client whose network is the people physically around you. Events propagate device-to-device over Bluetooth and local Wi-Fi. There are no relays, no servers, and no internet dependency in the core loop.

It differs from its two closest relatives:

- **Manyverse** (SSB) gets offline gossip right but bridges freely over rooms and pubs, and its data model is SSB's append-only log rather than nostr's signed events.
- **bitchat** gets local mesh right but is a chat app, and its "global reach" path bridges distant peers over public relays — precisely what this design excludes.

Serendipity takes Manyverse's sync model, bitchat's transport engineering, nostr's data model, and neither project's bridging.

## Invariants

These are load-bearing. Everything else follows.

**I1 — Proximity.** A peer connection is only ever established with a device that is currently physically nearby. Enforced structurally, not by runtime policy: the only way to learn how to reach a peer is to have been in Bluetooth range of them.

**I2 — Progressive enhancement.** Bluetooth is the floor and always works. Network transport is an optimization for bandwidth and reliability. No feature may depend on the upgrade succeeding.

**I3 — Offline-first gossip.** No part of the *gossip* loop requires internet: discovery, session establishment, sync, storage and forwarding all work with no network. Account-level operations may reach the network — that is a property of the login method, not of the protocol, and [`identity.md`](./identity.md#key-custody) records what it costs.

**I4 — Data outlives connections.** Sync is store-and-forward. Tearing down a link does not discard what was synced over it, and events propagate transitively through people who move. This means I1 constrains *connections*, not *information* — see [`privacy.md`](./privacy.md).

**I5 — Bounded reach.** An event travels at most two hops from its author: to someone the author met, and one step beyond. Enforced cryptographically by delivery grants ([`sync.md`](./sync.md#delivery-grants)), not by policy — a device holding no grant that names it cannot forward, whatever its software does. This makes the social topology structural in the same way I1 makes proximity structural.

## Non-goals

- No relay fallback, no hole punching, no global discovery, no DHT.
- No bridging of peers who have not been co-present.
- **No unbounded flooding.** Reach is capped at two hops by construction, not by a policy each device is trusted to apply. See [`sync.md`](./sync.md#delivery-grants).
- **No interoperability with the open nostr network.** Content events carry no signature, so relays reject them and no existing client can read them. This is deliberate; see [`identity.md`](./identity.md#events-are-not-signed-grants-are).
- Not a chat app. Rich nostr event types, including media, are first-class.

## The stack

```
┌──────────────────────────────────────────────────────┐
│  Svelte UI · welshman app layer          (webview)   │
├──────────────────────────────────────────────────────┤
│  Sync — relay wire protocol, policy      sync.md     │
├──────────────────────────────────────────────────────┤
│  Storage — SQLite, seen_at, blobs        storage.md  │
├──────────────────────────────────────────────────────┤
│  Discovery — advertise, connect,         discovery.md│
│              identify, heartbeat                     │
├──────────────────────────────────────────────────────┤
│  Transport — BLE framing · iroh QUIC     transport.md│
└──────────────────────────────────────────────────────┘
```

| Document | Covers |
| --- | --- |
| [`discovery.md`](./discovery.md) | Advertisement, connection scheduling, identification, session lifecycle, heartbeat and teardown |
| [`transport.md`](./transport.md) | BLE link layer and framing, Noise XX, iroh configuration, address exchange, the Wi-Fi capability gap |
| [`sync.md`](./sync.md) | Relay wire protocol as the peer protocol, delivery grants and the two-hop cap, reconciliation, sync policy, mute, quotas |
| [`storage.md`](./storage.md) | Native SQLite as source of truth and relay, `seen_at`, background serving, retention |
| [`media.md`](./media.md) | Blob tiers, transfer, fetch policy, quotas |
| [`identity.md`](./identity.md) | Keys, unsigned events and grants, key custody, login with device, backup |
| [`privacy.md`](./privacy.md) | Threat model, what leaks, what users will wrongly assume |
| [`nip-p2p-auth.md`](./nip-p2p-auth.md) | NIP — NIP-42 over transports without URLs |

## Client architecture

Capacitor 8 + Svelte 5 + welshman `0.9.x` + uniffi.

### The plugin boundary

The webview is suspended in the background. **Everything that must survive backgrounding lives in the native plugin.** This is the central constraint on the whole app.

| Native (Swift / Kotlin) | Webview (TypeScript) |
| --- | --- |
| BLE advertise, scan, GATT both roles | UI, all of it |
| Noise handshake, channel encryption | Social graph, WoT, scope computation |
| Framing, fragmentation, scheduling | Feed construction, rendering, composition |
| Heartbeat and session state machine | Signing, key management |
| iroh endpoint (Rust via uniffi) | Working-set `Repository` |
| SQLite store, filter matching, serving | |
| Blob transfer and storage | |

Native holds the durable store and serves peers autonomously using a policy snapshot compiled by the webview. The webview reads its own store over the same relay protocol, through a `SQLITE_STORAGE_URL` adapter that crosses the Capacitor bridge — so one protocol covers webview → native, native → peer, and peer → peer. The in-memory `Repository` stays as the working set because welshman's reactive layer derives from it synchronously. See [`storage.md`](./storage.md).

## Decisions log

| Decision | Rationale |
| --- | --- |
| Proximity enforced by configuration, not runtime checks | A heartbeat that tears down a wrong connection is reactive; disabling discovery and relay means it can't form. |
| BLE is the only address-lookup service | The only way to learn an address is to have been in radio range. Makes I1 structural. |
| iroh with no pkarr/DHT/mDNS/relay | Every one of those exists to connect peers who aren't co-present. |
| Own BLE framing, not `iroh-ble-transport` | AGPL-3.0 vs. app store distribution, no legal review planned. Also drops the 1200-byte datagram floor and L2CAP fallback. |
| Heartbeat is liveness, not authorization | Proximity already guaranteed structurally, so teardown can be lenient and never kills a live transfer. |
| Advertisement carries no identity | Not a choice — iOS strips background advertisement payload. Identification is necessarily post-connect. |
| Nostr relay wire protocol as the peer protocol | Client half is `@welshman/net` unchanged; policy compiles to filters; NIP-77 and NIP-42 come free. |
| Relay half is an app module, not upstream | A library version would serve a `Repository`; ours answers from the working set or native SQLite per request. |
| Mutual NIP-42 bound to transport identity | Without the binding, any peer you authenticate to can impersonate you to every other peer. |
| Auth events are signed; content events are not | NIP-42 needs a verifiable signature. Content authenticity comes from the grant instead. |
| Content events carry no signature | An unsigned event is invalid everywhere, so a leak is rejected by relays rather than stored. Replaces partition signing, which did the same job with more machinery. |
| Two-hop cap via delivery grants | Makes the social topology structural. A grant names one recipient and cannot be forged, so the bound needs no honest-node assumption. |
| One grant per synchronized chunk, over a Merkle root | Handing a peer *n* events costs one signature, not *n*. Without it a remote signer means *n* round trips, which no drive-by encounter survives. |
| Grant is detached, not folded into the event id | Hashing the recipient in would give one post a different id per recipient, and reconciliation diffs id sets — it could never converge. Also breaks threading and dedup. |
| Ids are plain NIP-01 hashes | Nothing app-specific in serialization, so `@welshman/util` is used unmodified and a promoted post keeps its id and its replies. |
| NIP-55 and NIP-46 both supported | Grants are events, so any signer produces them. A bunker user cannot peer without connectivity, which is stated rather than prevented. |
| Backup and login-with-device require the nsec | Both move a key the app does not hold under NIP-55; gated rather than broken. |
| Grants are nostr events, not bare signatures | `@welshman/signer` signs them and `@welshman/util` verifies them, both unmodified. No bespoke signing path. |
| Login with device for multi-device | Reuses the proximity stack. Needs a short authentication string — the user has no prior knowledge of the target's key. |
| Backup is an exported text file, optionally NIP-49 encrypted | Follows Flotilla's `KeyDownload`. Multi-device is the happy path; most users have one phone. Instructions ship in the file. |
| One-shot GCS over BLE, negentropy over iroh | Session duration, not latency: a drive-by lasts seconds and a multi-round negotiation may never converge. |
| Reconciliation scope is `created_at`; ordering is `seen_at` | `seen_at` is local and private, so it can't define a scope both peers can compute. |
| `seen_at` set once, never updated, never transmitted | Duplicate arrivals must not churn the recently-discovered view; encounter times are movement data. |
| Sync scope is social distance, not a boolean | On/off is either too leaky or useless. Manyverse's `hops=2` is the precedent; "hops" is reserved here for delivery distance under I5. |
| Mute and block conflated, kind 10000 | One concept: nothing to do with this person. Blocks ingest, egress, transitive gossip, and peering; purges stored events. |
| Media in three tiers | BLE at 5–15 KB/s carries previews, not originals. Tier 0 makes the timeline render instantly. |
| Native SQLite is source of truth and speaks the relay protocol | Background serving is non-negotiable; the relay protocol keeps the app layer unchanged. |
| `SQLITE_STORAGE_URL` adapter for webview → native reads | Same protocol as peers, resolved by `getAdapter`. Hydration is welshman's existing `makeLoadItem` chain pointed at a local URL. |
| Working set is kept, not replaced by direct SQLite reads | welshman derives from a `Repository` synchronously. Dropping it makes every derived store async — an app-layer rewrite, not a perf tradeoff. |
| Capacitor over Tauri and KMP | App layer is TypeScript and stays TypeScript; the hard problems are native iOS, where Capacitor has the most prior art. |
