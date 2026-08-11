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

## Background

James Carey distinguished two views of communication. The transmission view understands communication as the movement of messages across space for the purpose of control. The ritual view understands communication as the maintenance of a society in time — the shared ceremony, the repeated act, the drawing of people into fellowship.

Space-binding media are light and portable, and they favor expansion, administration, and empire because they make it possible to govern distant things from a center. Time-binding media are heavy, durable, and local, and they favor continuity, community, and memory, because they make it possible for a place to persist as itself.

General-purpose digital networks tend to be aggressively space-binding. The marginal cost of moving a message one meter and ten thousand kilometers is identical, and the economics built on that fact reward the annihilation of distance and the aggregation of attention at a center. A culture that relies too heavily on a space-binding medium suffers from extraction by third parties, is displaced by its representation, its context collapses, and its activity is instrumentalized.

This project is time-biased. It treats distance as something to articulate rather than eliminate.

## Principles

1. **Digital localism is structural, not based on policy.** Exclusive use of physical media enforces proximity. Limited device storage enforces ephemerality. Reach is a function of trust, which means amplification reflects communal assent. Cryptography governs verifiability of public speech and protects confidential speech from middlemen.

2. **Trust is explicit.** Peering with an unknown person prompts a introduction ceremony. Only after is that complete can gossip proceed. Users locally maintain contact lists and policies which allow for fine-grained control over what events they receive, transmit, and who they peer with.

2. **Forgetting is the default.** If someone does not actively participate in the community, they fall out of it. If content is not repeatedly invoked, it disappears. Reach is a function of communal value, expressed through repeated propagation.

3. **Private speech is deniable; public speech is attributable.** Separate registers, separate cryptographic treatment, and a difference the interface makes obvious. Reach is bounded - an event travels at most two hops from its author.

4. **Communication requires rich content types.** Communication should not be limited to chat. Different types of communication should be presented in different ways.

5. **The wire carries mass; the mesh carries meaning.** Internet transport is used exclusively for emergencies which require space-binding transmission, and for propagation of large files.

## Non-goals

- No relay fallback, no hole punching, no global discovery, no DHT.
- No bridging of peers who have not been co-present.
- **No unbounded flooding.** Reach is capped at two hops.
- **No compatibility with public relays.** Content events carry no signature, so relays reject them and no existing client can read them.

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
- **`coracle-lib`** — nostr types for the core: NIP-01 serialization, filters, NIP-77 negentropy. Its event hierarchy separates an unsigned `HashedEvent` from a signed `Event`, which is what [unsigned content events](./proofs.md#events-are-not-signed) need; rust-nostr's mandatory signature cannot express it.
- **`snow`** — Noise XX: Curve25519 / ChaCha20-Poly1305 / SHA-256.
- **`rusqlite`** — durable event store, indexes, NIP-01 filter matching.
- **`secp256k1`** — signing and verifying authorship proofs and auth events; **`k256`** for the [authorship proof](./proofs.md#authorship-proofs), which needs explicit group arithmetic that the binding does not expose.

## Architecture

There are three layers: the **core** (Rust), the **shell** (Swift and Kotlin), and the **view** (web).

Because gossip has to happen in the background with peers over bluetooth while the user's device is locked and in their pocket, none of it can live in the view. The responsibility for nearly all application logic therefore belongs to the core. This also allows us to avoid duplicating application logic in the `shell`, which is mostly glue code (capacitor plugins, BLE, keychain/keystore), or in the `view`, which is dedicated to UI concerns.

The shell depends on the core at link time, and the core depends on nothing platform-specific. Where the core needs a platform capability it declares a trait and the shell hands it an implementation.

Only one of those boundaries is expensive. SQLite is in-process C, and uniffi passes scalars directly and everything else as a compact binary buffer, so the cost lives at the Capacitor bridge, which marshals as JSON.

## Storage

All storage is managed by `core`. Events, provenance, preferences, etc. are all stored in sqlite, while blobs are stored in a blob store managed by `core` and configured by `shell`.

Read more at [`storage.md`](./storage.md)

## Discovery

Peers are found by a bare presence beacon that carries no identity. A recognition exchange keyed on per-pair secrets identifies peers without naming anything durable.

Read more at [`discovery.md`](./discovery.md)

## Transport

Bluetooth is the only transport. Every device runs both GATT roles at once, with our own framing over a Noise XX channel. Increased bandwidth can be obtained by upgrading to an L2CAP channel over the same connection and Noise session.

Read more at [`transport.md`](./transport.md)

## Keys

There are two keypairs: a long-term secp256k1 nostr identity, and a Curve25519 Noise static key generated fresh for every session. Mutual NIP-42 binds the two per session, naming `noise://<static key>`, a channel the handshake established.

The app holds the nostr key in platform secure storage, readable while the device is locked. Moving it to a second device runs over the same proximity stack, gated on explicit action at both ends and a short authentication string; the fallback is a file export.

Read more at [`keys.md`](./keys.md)

## Sync

Each device is a p2p nostr relay and a nostr client at once, reusing the relay protocol in both directions. Content events carry an id and no `sig`. At the first hop the authenticated session establishes authorship; the second hop is enabled using authorship proofs which aren't forwardable.

Event syncing happens via negentropy; blob syncing is done by sha256 hash. What can be received or sent to a given peer depends on user policy.

Read more at [`sync.md`](./sync.md) and [`policy.md`](./policy.md).

## Storage

The core's SQLite is the only store: events, their tags, a full-text index, and provenance: one row per event per peer it has been seen from. It answers peers with the relay protocol, and the view with a live query method built for the store. The view holds caches for what it reads synchronously.

Ingest happens once, in the core: id recomputation, proof verification, quota accounting, and the provenance row. Nothing unverified ever reaches the view.

Read more at [`storage.md`](./storage.md)

## Privacy

A passive radio observer learns only that some device running this app is nearby. An active one can always complete a handshake, so nothing that handshake discloses outlives the session, and outside a discoverable window no pubkey moves at all. A peer who completes a session learns the user's pubkey, that they were physically present at a time and place, and whatever the gossip scope serves — which is why the consent gate sits before authentication.

`seen_at` and provenance are records of the user's movements, so they never leave the device. An authorship proof convinces its recipient and nobody else, so a second-hop recipient knows where an event came from and cannot prove it.

Read more at [`privacy.md`](./privacy.md)

## Interface

shadcn-svelte over bits-ui and Tailwind 4, vendored by CLI rather than taken as a dependency.

Design values live in exactly one file: color, elevation, motion and radius are Tailwind tokens in `src/app.css`, and the standard scales are redefined rather than supplemented, so the vendored components restyle without being edited. The look is restrained claymorphism.

An event kind is a `KindFactory` in `src/lib/kinds/`, a collection of events is a store in `src/lib/data/`, and nothing outside them pokes at tags or opens a query.

Read more at [`ui.md`](./ui.md)
