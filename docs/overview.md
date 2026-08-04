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

**I3 — Offline-first.** No part of the core loop requires internet. If the device never sees a network, the app works.

**I4 — Data outlives connections.** Sync is store-and-forward. Tearing down a link does not discard what was synced over it, and events propagate transitively through people who move. This means I1 constrains *connections*, not *information* — see [`privacy.md`](./privacy.md).

## Non-goals

- No relay fallback, no hole punching, no global discovery, no DHT.
- No bridging of peers who have not been co-present.
- **No interoperability with the open nostr network.** Proximity events are partition-signed and cannot be published to relays or read by other clients. This is deliberate; see [`identity.md`](./identity.md#partition-signing).
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
| [`sync.md`](./sync.md) | Relay wire protocol as the peer protocol, reconciliation, sync policy, mute, quotas |
| [`storage.md`](./storage.md) | Native SQLite as source of truth and relay, `seen_at`, background serving, retention |
| [`media.md`](./media.md) | Blob tiers, transfer, fetch policy, quotas |
| [`identity.md`](./identity.md) | Keys, partition signing, key custody, login with device, backup |
| [`privacy.md`](./privacy.md) | Threat model, what leaks, what users will wrongly assume |
| [`nip-p2p-auth.md`](./nip-p2p-auth.md) | NIP — NIP-42 over transports without URLs |
| [`nip-partition-sig.md`](./nip-partition-sig.md) | NIP — protocol partitioning |

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
| Nostr relay wire protocol as the peer protocol | Reuses `@welshman/net` on both sides; policy compiles to filters; NIP-77 and NIP-42 come free. |
| Relay half lives upstream in welshman | Generally useful, and keeps peers on the same pipeline as relays. |
| Mutual NIP-42 bound to transport identity | Without the binding, any peer you authenticate to can impersonate you to every other peer. |
| Auth events stay in the default partition | Keeps the auth NIP generally useful upstream; transport binding already prevents replay to relays. |
| Partition-sign proximity events | Makes I1 cryptographic. A leaked event is invalid, not a real post. Accepts total non-interoperability as the point. |
| Bare nsecs in secure storage, no external signers | Forced by partition signing; NIP-07/46/55 signers can't produce partitioned signatures. |
| Partition identifier is an arbitrary string | Opaque, byte-compared, no registry. Default partition is the absence of the field, so existing NIP-01 serialization is unchanged. |
| Login with device for multi-device | Reuses the proximity stack. Needs a short authentication string — the user has no prior knowledge of the target's key. |
| Backup is an exported text file, optionally NIP-49 encrypted | Follows Flotilla's `KeyDownload`. Multi-device is the happy path; most users have one phone. Instructions ship in the file. |
| One-shot GCS over BLE, negentropy over iroh | Session duration, not latency: a drive-by lasts seconds and a multi-round negotiation may never converge. |
| Reconciliation scope is `created_at`; ordering is `seen_at` | `seen_at` is local and private, so it can't define a scope both peers can compute. |
| `seen_at` set once, never updated, never transmitted | Duplicate arrivals must not churn the recently-discovered view; encounter times are movement data. |
| Sync scope is hops, not a boolean | On/off is either too leaky or useless. Manyverse's `hops=2` is the precedent. |
| Mute and block conflated, kind 10000 | One concept: nothing to do with this person. Blocks ingest, egress, transitive gossip, and peering; purges stored events. |
| Media in three tiers | BLE at 5–15 KB/s carries previews, not originals. Tier 0 makes the timeline render instantly. |
| Native SQLite is source of truth and speaks the relay protocol | Background serving is non-negotiable; the relay protocol keeps the app layer unchanged. |
| `SQLITE_STORAGE_URL` adapter for webview → native reads | Same protocol as peers, resolved by `getAdapter`. Hydration is welshman's existing `makeLoadItem` chain pointed at a local URL. |
| Working set is kept, not replaced by direct SQLite reads | welshman derives from a `Repository` synchronously. Dropping it makes every derived store async — an app-layer rewrite, not a perf tradeoff. |
| Capacitor over Tauri and KMP | App layer is TypeScript and stays TypeScript; the hard problems are native iOS, where Capacitor has the most prior art. |
