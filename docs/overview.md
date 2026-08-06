# Dip — Overview

Offline-first nostr social gossip over Bluetooth. This is the overview; each subsystem has its own document.

## What this is

A nostr client whose network is the people physically around you. Events propagate device-to-device over Bluetooth, with no internet dependency in the core loop.

The setting it is designed around is a neighborhood or a town: local gossip propagating between people who might not otherwise talk, and conversations that start because two phones were in the same place.

It differs from its closest relatives:

- **Samiz** sits closest: a Bluetooth mesh for nostr, running beside a local relay and an ordinary client, reconciling with negentropy. Once a peer with internet joins the mesh, their device republishes what it synced to its own relays, so a note reaches people who were never nearby.
- **bitchat** gets local mesh right but is a chat app, and its "global reach" path bridges distant peers over public relays — precisely what this design excludes.
- **Briar** is the closest on framing: offline-first, with forums and blogs rather than only chat, syncing over Bluetooth and Wi-Fi. Its escape hatch is Tor rather than public relays, it runs on Android only, and its data model is its own rather than nostr's.
- **Manyverse** (SSB) gets offline gossip right but bridges freely over rooms and pubs, and its data model is SSB's append-only log rather than nostr's signed events.
- **Rhizome** (Serval Project) is the unbounded case, and the one I5 is written against: a bundle may end up replicated on every node in the network, and the project's own disclaimer says the app copies shared files to every other device running it, regardless of size, content, or intended recipient.

Dip takes Manyverse's sync model, bitchat's transport engineering, nostr's data model, and none of their bridging.

## Invariants

These are load-bearing.

**I1 — Proximity.** A peer connection is only ever established with a device that is currently physically nearby. Enforced structurally, not by runtime policy: the only way to learn how to reach a peer is to have been in Bluetooth range of them.

**I2 — Progressive enhancement.** Bluetooth is the floor and always works. Any transport above it is an optimization for bandwidth. See [`transport.md`](./transport.md).

**I3 — Offline-first gossip.** No part of the gossip protocol requires internet: discovery, session establishment, sync, storage, signing and forwarding all work with no network. The app has direct access to the user's key ([`keys.md`](./keys.md#key-custody)), so no network is needed for signing.

**I4 — Data outlives connections.** Sync is store-and-forward. Tearing down a link does not discard what was synced over it, and events propagate transitively through people who move. This means I1 constrains *connections*, not *information* — see [`privacy.md`](./privacy.md).

**I5 — Bounded reach.** An event travels at most two hops from its author: to someone the author met, and one step beyond. Enforced cryptographically by authorship proofs ([`proofs.md`](./proofs.md#direct-authorship-proofs)), not by policy.

## Non-goals

- No relay fallback, no hole punching, no global discovery, no DHT.
- No bridging of peers who have not been co-present.
- **No unbounded flooding.** Reach is capped at two hops by construction, not by a policy each device is trusted to apply. See [`sync.md`](./proofs.md#direct-authorship-proofs).
- **No interoperability with the open nostr network.** Content events carry no signature, so relays reject them and no existing client can read them. See [`keys.md`](./proofs.md#events-are-not-signed).
- Not a chat app. Rich nostr event types, including media, are first-class.

## Tech stack

- **TypeScript / Svelte 5** — the view: UI and reactivity.
- **Tailwind 4 / shadcn-svelte** — design tokens and vendored components. See [`ui.md`](./ui.md).
- **welshman** — `util`, `lib` and `domain` for nostr types, kinds and typed readers. Not `app`, and not the peer protocol — see [architecture](#architecture).
- **nostr-tools** — NIP-19 encoding for display.
- **Vite 8** — bundles the web assets to `dist/`, which the shells load.
- **Capacitor 8** — the shell, and the JSON bridge between view and core.
- **Rust** — the core: protocol, crypto, store, session state. Everything below the UI that has to run while backgrounded.
- **uniffi** — Swift and Kotlin bindings to that core.
- **Swift / Kotlin** — the platform shell: radio, secure storage, lifecycle, bridge.
- **CoreBluetooth** — iOS dual-role GATT, background modes, state restoration.
- **`android.bluetooth`** — Android advertiser, scanner, GATT server and client.
- **BLE GATT** — the transport, with [our own framing](./transport.md#framing) over one characteristic.
- **`coracle-lib`** — nostr types for the core: NIP-01 serialization, filters, NIP-77 negentropy. Its event hierarchy separates an unsigned `HashedEvent` from a signed `Event`, which is what the [content/grant split](./proofs.md#events-are-not-signed) needs; rust-nostr's mandatory signature cannot express it.
- **`snow`** — Noise XX: Curve25519 / ChaCha20-Poly1305 / SHA-256.
- **`rusqlite`** — durable event store, indexes, NIP-01 filter matching.
- **`secp256k1`** — signing and verifying authorship proofs and auth events; **`k256`** for the [indirect authorship proof](./proofs.md#indirect-authorship-proofs), which needs explicit group arithmetic that the binding does not expose.

## Architecture

There are three layers: the **core** (Rust), the **shell** (Swift and Kotlin), and the **view** (web).

Because gossip has to happen in the background with peers over bluetooth while the user's device is locked and in their pocket, none of it can live in the view. The responsibility for nearly all application logic therefore belongs to the core. This also allows us to avoid duplicating application logic in the shell.

The core covers:

- Both halves of the relay protocol
- Noise handshake and framing codec
- Session state machine, heartbeat
- Signing auth events and authorship proofs
- Merkle trees, indirect authorship proofs
- Reconciliation - GCS, negentropy
- SQLite store, filter matching
- Policy interpretation, scope and web of trust, quotas
- Blob transfer, assembly, verification

The shell covers:

- BLE advertise, scan, GATT both roles
- Keychain / Keystore
- Background lifecycle, state restoration
- Paths and data-protection classes
- Capacitor bridge marshalling

The view covers:

- Editing policy preferences: scope, mutes, discoverability
- Event parsing (for display), rendering, composition
- All user interfaces

Calls run one way:

```
view ──JSON over Capacitor──▶ shell ──uniffi──▶ core ──C FFI──▶ SQLite
```

The shell depends on the core at link time, and the core depends on nothing platform-specific. Where the core needs a platform capability it declares a trait and the shell hands it an implementation.

Only one of those boundaries is expensive. SQLite is in-process C, and uniffi passes scalars directly and everything else as a compact binary buffer, so the cost lives at the Capacitor bridge, which marshals as JSON.

The core holds the only store and serves peers autonomously, resolving sync scope from the follows and mutes it already holds. The view keeps no store of its own: a controller layer queries the core over the bridge and caches per use case. See [`storage.md`](./storage.md).

## Discovery

The BLE advertisement is a bare presence beacon: our service UUID and no payload. Identification is therefore always post-connect, and a stranger is indistinguishable from a close friend until the link is up and the handshake has run.


A session climbs a fixed ladder — IDLE, LINKED once GATT connects, SECURED once Noise XX completes, IDENTIFIED once mutual NIP-42 has run and policy has been evaluated, SYNCING, then DRAINING and CLOSED. Between SECURED and IDENTIFIED sits the consent gate, because authenticating discloses a long-term identity and, on a proximity transport, a physical presence at a time and place.

Read more at [`discovery.md`](./discovery.md)

## Transport

Bluetooth is the only transport. Every device runs a GATT peripheral and a GATT central at once, over one service and one characteristic, with our own framing over a Noise XX channel — multiplexed, priority-scheduled, fragmented to the MTU, resumable by offset.

Measured throughput is 5–15 KB/s. Event sync fits in a drive-by, image previews fit, but originals do not. Increased bandwidth can be obtained by upgrading to an L2CAP channel over the same connection and Noise session.

Read more at [`transport.md`](./transport.md)

## Keys

Two keypairs with two jobs: a long-term secp256k1 nostr identity, and a per-install Curve25519 Noise static key. Nothing binds them durably — mutual NIP-42 binds them per session, naming `noise://<static key>`, an identity the BLE handshake has already authenticated.

The app holds the key in platform secure storage, readable while the device is locked. Moving it to a second device runs over the same proximity stack, gated on explicit action at both ends and a short authentication string; the fallback is a file export.

Read more at [`keys.md`](./keys.md)

## Sync

Each device is a p2p nostr relay and a nostr client at once, reusing the relay protocol in both directions.

Content events carry an id and no `sig`, so authenticity comes instead from a direct authorship proof — a signed nostr event which names a recipient and commits through a Merkle root to a whole chunk of events at once. Ids stay plain NIP-01 hashes, so an author can sign one of their own posts and promote it to the open network, keeping its id and its replies. Every inbound event needs a grant naming us if the sender wrote it, or a grant proof from the sender if someone else did; anything else is dropped. A grant is transferable evidence of authorship, so it never leaves the peer it names and the second hop receives a proof designated to it instead. That is what caps reach at two hops without trusting anyone's software.

Reconciliation runs a one-shot GCS filter on connect and negentropy for as long as the session survives. What can be received or sent to a given peer depends on social graph data, including follows and mutes, as well as specific settings for app behavior, including whether to ask the user before peering with a stranger and whether to gossip second hops.

Read more at [`sync.md`](./sync.md)

## Storage

The core's SQLite is the only store: events, their tags, a full-text index, and provenance: one row per event per peer it has been seen from. It answers peers with the relay protocol, and the view with a live query method built for the store, which filters on seen time and peer. The view holds caches for what it reads synchronously.

Ingest happens once, in the core: id recomputation, proof verification, quota accounting, and the provenance row. Nothing unverified ever reaches the view.

Read more at [`storage.md`](./storage.md)

## Media

Blobs are referenced by hash and transferred on their own channel, separately from events. Three tiers follow from 5–15 KB/s. Tier 0 — blurhash, dimensions, duration, mime — rides inline with the event and makes the timeline render immediately; tier 1 is a preview of at most 32 KB, fetched automatically under policy; tier 2 is the full-resolution original, never automatic, and may take several encounters to arrive. Images added by the user are compressed before they are stored.

Read more at [`media.md`](./media.md)

## Privacy

A passive radio observer learns that a device running this app is present, and nothing else. A peer who completes a session learns the user's pubkey, that they were physically present at a time and place, and whatever the gossip scope serves — which is why the consent gate sits before authentication.

`seen_at` and provenance are records of the user's movements, so they never leave the device. A direct authorship proof is transferable evidence, so it stays one hop from the author: a second-hop recipient knows where an event came from but can't prove it.

Read more at [`privacy.md`](./privacy.md)

## Interface

shadcn-svelte over bits-ui and Tailwind 4, vendored by CLI rather than taken as a dependency.

Design values live in exactly one file: colour, elevation, motion and radius are Tailwind tokens in `src/app.css`, and the standard scales are redefined rather than supplemented, so the vendored components restyle without being edited. The look is restrained claymorphism.

An event kind is a `KindFactory` in `src/lib/kinds/`, a collection of events is a store in `src/lib/data/`, and nothing outside them pokes at tags or opens a query.

Read more at [`ui.md`](./ui.md)
