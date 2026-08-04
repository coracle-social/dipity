# Serendipity — agent guide

Offline-first nostr social gossip over local transports. The network is the people physically around you: events propagate device-to-device over Bluetooth and local Wi-Fi, with no relays, no servers, and no internet in the core loop.

Read [`docs/overview.md`](./docs/overview.md) before making design decisions. It carries the invariants and the decisions log. This file is the short version plus the things that are easy to get wrong.

## Invariants

Load-bearing. Do not write code that violates these, and flag any request that would.

- **I1 — Proximity.** Connections only ever form with a currently-nearby device. Enforced structurally: BLE range is the only way to learn how to reach a peer.
- **I2 — Progressive enhancement.** Bluetooth is the floor. Network transport is an optimization; no feature may depend on the upgrade succeeding.
- **I3 — Offline-first gossip.** Discovery, sync, storage and forwarding never require internet. Login and signing may, depending on key custody.
- **I4 — Data outlives connections.** Sync is store-and-forward, and events propagate transitively through people who move. I1 constrains *connections*, not *information*.
- **I5 — Bounded reach.** An event travels at most two hops from its author. Enforced cryptographically by delivery grants, not by policy — a device with no grant naming it cannot forward, whatever its software does.

## Hard rules

Each of these is a decision already made, with reasoning in the linked doc. They read like arbitrary restrictions if you meet them cold.

- **No relay fallback, no hole punching, no DHT, no mDNS, no global discovery.** Every one of those exists to connect peers who are not co-present. iroh is configured with all of it off — [`transport.md`](./docs/transport.md).
- **No bridging peers who have not been co-present.** This is the thing that separates the project from bitchat's global-reach path.
- **Content events are never signed.** They carry an id and no `sig`. Authenticity comes from the delivery grant, and unsignedness is what keeps proximity content off relays. Do not add a `sig` to a content event. [`identity.md`](./docs/identity.md#events-are-not-signed-grants-are).
- **Never send a grant to anyone but the peer it names.** A grant is portable proof of authorship; that is the whole reason the second hop gets a proof instead. [`sync.md`](./docs/sync.md#grant-proofs).
- **Every inbound event needs a grant or a grant proof, or it is dropped.** A grant naming *me* if the sender authored it, a grant proof from *the sender* otherwise, checked against the NIP-42-authenticated peer. Grants cover a Merkle root over a whole chunk, so "covers this event" means an inclusion check. This is the whole of I5; no policy flag relaxes it. [`sync.md`](./docs/sync.md#delivery-grants).
- **Ids are plain NIP-01 hashes.** Nothing app-specific in serialization, so `@welshman/util` is used unmodified. Do not fold the recipient, the partition, or anything else into the id — reconciliation diffs id sets and would stop converging.
- **External signers are supported; backup and login-with-device are not, on those paths.** Both move a key the app does not have, so gate them on holding the nsec rather than letting them fail. A NIP-46 user additionally cannot peer with no connectivity, since session auth needs a signature. [`identity.md`](./docs/identity.md#key-custody).
- **`seen_at` is never transmitted.** Set once on first insert, never updated, never served to a peer. It is a record of the user's movements. [`storage.md`](./docs/storage.md#seen_at) and [`privacy.md`](./docs/privacy.md).
- **Grants and auth events are the only signed kinds.** Both are ordinary nostr events. Content events are not signed, ever.
- **Never claim posts only reach nearby people.** False under I4 — a second-hop recipient may be anywhere. Say reach is bounded at two hops instead. [`privacy.md`](./docs/privacy.md).

## The plugin boundary

The central constraint on the whole app: **the webview is suspended in the background, so everything that must survive backgrounding lives in the native plugin.**

| Native (Swift / Kotlin / Rust) | Webview (TypeScript) |
| --- | --- |
| BLE advertise, scan, GATT both roles | UI |
| Noise handshake, channel encryption | Social graph, WoT, scope computation |
| Framing, fragmentation, scheduling | Feed construction, rendering, composition |
| Heartbeat, session state machine | Signing, key management |
| iroh endpoint (Rust via uniffi) | Working-set `Repository` |
| SQLite store, filter matching, serving | |
| Blob transfer and storage | |

Native holds the durable store and serves peers autonomously from a **compiled policy snapshot** — a materialized author set plus limits, handed down by the webview. Native applies policy; it never computes the web of trust.

Peers speak the **nostr relay wire protocol** over every hop, including webview → native. Sync is `REQ`/`EVENT`/`EOSE`/`CLOSE`/`OK`/`AUTH`/`NEG-*`, not a new protocol. [`sync.md`](./docs/sync.md).

Three URL families, one protocol, resolved by a `getAdapter` override: `LOCAL_RELAY_URL` (in-memory working set), `SQLITE_STORAGE_URL` (native store over the bridge), and `ble://` / `iroh://` (peers). They differ in *trust*, not protocol — peers get grant checking, `AUTH`, and policy; the store gets none of them, and events crossing the bridge arrive pre-verified with `verifiedSymbol` set. Do not re-check them. [`storage.md`](./docs/storage.md).

**The in-memory `Repository` is not optional.** welshman's reactive layer (`deriveEventsById`, `deriveItemsByKey`, `getter`) derives from a `Repository` instance synchronously. Querying SQLite directly instead would make every derived store async.

## Stack and commands

Capacitor 8 · Svelte 5 · Vite 8 · TypeScript · welshman `0.9.x` · uniffi.

```sh
npm run dev       # Vite dev server, browser only
npm run check     # svelte-check + tsc
npm run sync      # vite build && cap sync  — run before any native build
npm run ios       # sync, then open Xcode
npm run android   # sync, then open Android Studio
```

App ID `social.coracle.serendipity`. Web assets build to `dist/`; native shells load the *built* output, so `npm run sync` after web changes or the native app runs stale code.

Native projects in `ios/` and `android/` are committed and regenerable. Capacitor does not propagate `appId` changes into them — change `capacitor.config.ts`, then delete and re-add the platforms rather than hand-editing.

## welshman

The app layer is [welshman](https://github.com/coracle-social/welshman) `0.9.x`. Clone the source into `./ref/welshman` if you need to read or change it — see [Reference materials](#reference-materials).

**Per-package skills are installed** in `.agents/skills/` (symlinked into `.claude/skills/`) — `welshman`, plus `welshman-{app,util,lib,net,store,signer,feeds,domain,content,editor}`. Load the relevant one before working against a package rather than guessing at its API; they are the authoritative reference here. Refresh with `npx skills add coracle-social/welshman`.

Siblings of `@welshman/app` are *peer* deps, so they are listed explicitly in `package.json` rather than resolved transitively. `@welshman/content` and `@welshman/editor` are not installed.

### What stays in this app

Two pieces that look like library work are deliberately app modules. Do not "fix" either by patching welshman.

- **Delivery grants** — issuing, attaching, and verifying them, plus the two-hop rule on ingest. Grants are ordinary nostr events, so `@welshman/signer` and `@welshman/util` handle the crypto unmodified; what is app-local is the ingest rule and the grant lifecycle. [`sync.md`](./docs/sync.md#delivery-grants).
- **The relay half** — serving inbound `REQ`, `AUTH` challenges, egress policy. A version in `@welshman/net` would serve a `Repository`; ours answers from whichever backend fits the request, the in-memory working set or native SQLite over the bridge. [`sync.md`](./docs/sync.md#relay-half).

welshman is still consumed unmodified for everything else. If you find yourself wanting to change it, that is a signal the boundary above is being crossed.

## Documents

| Document | Covers |
| --- | --- |
| [`overview.md`](./docs/overview.md) | Overview, invariants, stack, plugin boundary, decisions log |
| [`discovery.md`](./docs/discovery.md) | Advertisement, connection scheduling, identification, session lifecycle, consent gate, heartbeat |
| [`transport.md`](./docs/transport.md) | BLE link layer and framing, Noise XX, iroh config, address exchange, capability gap |
| [`sync.md`](./docs/sync.md) | Relay wire protocol as peer protocol, delivery grants and the two-hop cap, reconciliation, scopes, mute, quotas |
| [`storage.md`](./docs/storage.md) | Native SQLite as source of truth and relay, `seen_at`, background serving, retention |
| [`media.md`](./docs/media.md) | Blob tiers, transfer, fetch policy, quotas |
| [`identity.md`](./docs/identity.md) | Keys, unsigned events and grants, custody, login with device, backup |
| [`privacy.md`](./docs/privacy.md) | Threat model, what leaks, what users wrongly assume |
| [`nip-p2p-auth.md`](./docs/nip-p2p-auth.md) | NIP — NIP-42 over transports without URLs |

## Reference materials

Four other codebases inform this design. They may be cloned into `./ref/`, which is gitignored — they are read-only prior art, not part of this app. Do not build, modify, or stage them. Nothing here depends on their being present.

| Reference | Clone URL | License | Consult for |
| --- | --- | --- | --- |
| welshman | `https://github.com/coracle-social/welshman.git` | MIT | Library source for the whole app layer — read it before guessing at an API |
| flotilla | `https://gitea.coracle.social/coracle/flotilla.git` | MIT | Another Coracle app on the same stack; `KeyDownload.svelte` and `lib/html.ts` model the backup flow in [`identity.md`](./docs/identity.md#backup) |
| bitchat | `https://github.com/permissionlesstech/bitchat.git` | Unlicense (public domain) | BLE transport engineering — framing, connection scheduling, `BLERecentPeripheralCache`, `GCSFilter` |
| manyverse | `https://gitlab.com/staltz/manyverse.git` | **MPL-2.0** | Offline sync model and `hops` scoping |

```sh
mkdir -p ref && git clone <url> ref/<name>
```

### Manyverse license note

**Manyverse is MPL-2.0, which is weak copyleft**, so treat it as read-only prior art. Reading it for design is unrestricted, but copying a file, or a substantial part of one, carries the license with it. Avoid copying verbatim from any reference in any case; they are there primarily as a conceptual guide.
