# Serendipity — agent guide

Offline-first nostr social gossip over Bluetooth. The network is the people physically around you: events propagate device-to-device, with no internet in the core loop. The setting it is designed around is a neighborhood or a town.

Read [`docs/overview.md`](./docs/overview.md) before making design decisions. It carries the invariants and the decisions log. This file is the short version plus the things that are easy to get wrong.

## Invariants

Load-bearing. Do not write code that violates these, and flag any request that would.

- **I1 — Proximity.** Connections only ever form with a currently-nearby device. Enforced structurally: BLE range is the only way to learn how to reach a peer.
- **I2 — Progressive enhancement.** Bluetooth is the floor, and currently the only transport. Anything above it is a bandwidth optimization; no feature may depend on one existing.
- **I3 — Offline-first gossip.** Discovery, sync, storage, signing and forwarding never require internet. No exceptions — custody is a key the device holds, so signatures are local.
- **I4 — Data outlives connections.** Sync is store-and-forward, and events propagate transitively through people who move. I1 constrains *connections*, not *information*.
- **I5 — Bounded reach.** An event travels at most two hops from its author. Enforced cryptographically by delivery grants, not by policy — a device with no grant naming it cannot forward, whatever its software does.

## Hard rules

Each is a decision already made; the linked doc carries the reasoning.

- **No relay fallback, no hole punching, no DHT, no mDNS, no global discovery.** Every one of those exists to connect peers who are not co-present. [`transport.md`](./docs/transport.md).
- **BLE is the only transport, and the seam for a second one stays open.** Do not add a transport that needs a discovery service, a rendezvous server, or a relay to reach its peer; reachability is exchanged over the authenticated BLE channel or not at all. L2CAP is the reserved bandwidth upgrade, not a second transport — same connection, same Noise session, no scheme. [`transport.md`](./docs/transport.md#adding-a-transport-later), [`transport.md`](./docs/transport.md#l2cap-is-the-reserved-bandwidth-upgrade).
- **No bridging peers who have not been co-present.** This is what separates the project from bitchat's global-reach path.
- **Content events are never signed.** They carry an id and no `sig`; authenticity comes from the delivery grant. Do not add one. [`identity.md`](./docs/identity.md#events-are-not-signed-grants-are).
- **Never send a grant to anyone but the peer it names.** A grant is portable proof of authorship, so the second hop gets a proof instead. [`sync.md`](./docs/sync.md#grant-proofs).
- **Every inbound event needs a grant or a grant proof, or it is dropped.** A grant naming *me* if the sender authored it, a grant proof from *the sender* otherwise, checked against the pubkeys the peer authenticated as — a set, not a scalar, since a peer may authenticate as several. Grants cover a Merkle root over a whole chunk, so "covers this event" means an inclusion check. No policy flag relaxes it. [`sync.md`](./docs/sync.md#delivery-grants).
- **Ids are plain NIP-01 hashes.** Nothing app-specific in serialization, so `@welshman/util` is used unmodified. Do not fold the recipient, the partition, or anything else into the id — reconciliation diffs id sets and would stop converging.
- **The nsec is the only custody model, and the webview never signs.** Both signed kinds are produced at encounter time, in a background wake, with no webview and maybe no network — so an external signer fails the core loop rather than degrading. Do not add NIP-46 or NIP-55, and do not move signing into TypeScript. [`identity.md`](./docs/identity.md#key-custody).
- **The identity key is readable while the device is locked** — `AfterFirstUnlock` on iOS, no user-auth requirement on Android. Deliberate: a phone that cannot sign cannot authenticate, and a peer that cannot authenticate can neither send nor receive. Do not "harden" this to `WhenUnlocked`; it silently kills pocket-to-pocket gossip. [`identity.md`](./docs/identity.md#the-key-is-readable-while-the-device-is-locked).
- **`seen_at` is never transmitted.** Set once on first insert, never updated, never served to a peer. It is a record of the user's movements. [`storage.md`](./docs/storage.md#seen_at) and [`privacy.md`](./docs/privacy.md).
- **Grants and auth events are the only signed kinds.** Both are ordinary nostr events.
- **Never claim posts only reach nearby people.** False under I4 — a second-hop recipient may be anywhere. Say reach is bounded at two hops instead. [`privacy.md`](./docs/privacy.md).

## The plugin boundary

The central constraint on the whole app: **the webview is suspended in the background, so everything that must survive backgrounding lives in the native plugin.** The test for where a thing goes is whether it runs at *encounter time* — a peer appearing while both phones are in pockets — not which layer it belongs to. The full split is tabulated in [`overview.md`](./docs/overview.md#the-plugin-boundary).

**The core is Rust so the protocol and the crypto exist once**, because both have to agree byte-for-byte with a peer running the *other* platform's build and the OR-proof fails silently when it is wrong. Swift and Kotlin get the parts that are genuinely per-platform and cryptographically dull. [`overview.md`](./docs/overview.md#why-the-core-is-rust).

Native holds the durable store and serves peers autonomously from a **compiled policy snapshot** — a materialized author set, paired Noise static keys, any open discoverable window, plus limits, handed down by the webview. Native applies policy; it never computes the web of trust. Consent is part of the snapshot, because there is no user to prompt during a background wake. [`discovery.md`](./docs/discovery.md#the-consent-gate).

Peers speak the **nostr relay wire protocol** over every hop, including webview → native: `REQ`/`EVENT`/`EOSE`/`CLOSE`/`OK`/`AUTH`/`NEG-*`. [`sync.md`](./docs/sync.md).

Three URL families, one protocol — but **the webview only resolves two.** `LOCAL_RELAY_URL` (in-memory working set) and `SQLITE_STORAGE_URL` (native store over the bridge) go through a `getAdapter` override; `ble://` exists only in the core. There is no `BleAdapter` in TypeScript and there should never be one — peers get grant checking, `AUTH` and policy, none of which the webview has. Events crossing the bridge arrive pre-verified with `verifiedSymbol` set; do not re-check them. [`storage.md`](./docs/storage.md).

**The in-memory `Repository` is not optional.** welshman's reactive layer (`deriveEventsById`, `deriveItemsByKey`, `getter`) derives from a `Repository` instance synchronously. Querying SQLite directly instead would make every derived store async.

## Stack and commands

Capacitor 8 · Svelte 5 · Vite 8 · TypeScript · welshman `0.9.x` · Rust + uniffi for the core.

**Tasks live in the [`justfile`](./justfile), not in `package.json`** — which has no `scripts` block, deliberately, because half the pipeline is `cargo`. `just` on its own lists everything.

```sh
just setup        # rust targets, npm deps — once after cloning
just dev          # Vite dev server, browser only
just qa           # svelte-check, tsc, cargo fmt/clippy/test — what CI runs
just core-test    # core tests alone, the fast loop
just bindings     # regenerate Swift + Kotlin from the built cdylib
just sync         # core → bindings → xcframework → web → cap sync
just ios          # sync, then open Xcode
just android      # sync, then open Android Studio
```

App ID `social.coracle.serendipity`. Web assets build to `dist/`; native shells load the *built* output, so `just sync` after web changes or the native app runs stale code.

**The core builds before the shells**, and `just sync` enforces the order — `cargo` cross-compiles for each target, `uniffi-bindgen` generates bindings from the *compiled* library, then `cap sync`. Never run `npx cap sync` directly; it skips the first two steps and the shells link against whatever was there before. Generated output stages in `core/target/ffi/` and is never committed. [`core/README.md`](./core/README.md).

The core is scaffolding at present: two crates, one exported call, enough to prove the toolchain links. Neither native project references the artifacts yet — that reference belongs to the Capacitor plugin, which does not exist either.

Native projects in `ios/` and `android/` are committed and regenerable. Capacitor does not propagate `appId` changes into them — change `capacitor.config.ts`, then delete and re-add the platforms rather than hand-editing.

## welshman

The app layer is [welshman](https://github.com/coracle-social/welshman) `0.9.x`. Clone the source into `./ref/welshman` if you need to read or change it — see [Reference materials](#reference-materials).

**Per-package skills are installed** in `.agents/skills/` (symlinked into `.claude/skills/`) — `welshman`, plus `welshman-{app,util,lib,net,store,signer,feeds,domain,content,editor}`. Load the relevant one before working against a package rather than guessing at its API; they are the authoritative reference here. Refresh with `npx skills add coracle-social/welshman`.

Siblings of `@welshman/app` are *peer* deps, so they are listed explicitly in `package.json` rather than resolved transitively. `@welshman/content` and `@welshman/editor` are not installed.

### What welshman is and is not used for

**Used for:** the `Repository` and the reactive layer it feeds, domain kinds, feeds, web of trust, and `AbstractAdapter` / `getAdapter` for the two local URLs. This is the app layer above the bridge, and it is consumed unmodified.

**Not used for the peer protocol, either half.** Sync begins when a peer appears, which the webview is not around for, so `@welshman/net` is not on that path at all — no `BleAdapter`, no `diff`/`pull`/`push` against a peer, no `Tracker` provenance for peer events. The core reimplements NIP-77 and NIP-42 against the same specifications. [`sync.md`](./docs/sync.md#peers-speak-the-relay-wire-protocol).

**`@welshman/signer` survives as an interface only.** Grants and auth events are signed in the core. What remains in TypeScript is a thin `ISigner` backed by a plugin `signEvent` op, which keeps welshman's session model working and serves the one deliberate foreground flow — promoting your own post to the open network. Nothing on the gossip path goes through it.

If you find yourself wanting to patch welshman, that is a signal the boundary above is being crossed.

## Documents

| Document | Covers |
| --- | --- |
| [`overview.md`](./docs/overview.md) | Overview, invariants, stack, plugin boundary, decisions log |
| [`discovery.md`](./docs/discovery.md) | Advertisement, connection scheduling, identification, session lifecycle, consent gate, heartbeat |
| [`transport.md`](./docs/transport.md) | BLE link layer and framing, Noise XX, the bandwidth ceiling, the seam for a second transport |
| [`sync.md`](./docs/sync.md) | Relay wire protocol as peer protocol, delivery grants and the two-hop cap, reconciliation, scopes, mute, quotas |
| [`storage.md`](./docs/storage.md) | Native SQLite as source of truth and relay, `seen_at`, background serving, retention |
| [`media.md`](./docs/media.md) | Blob tiers, transfer, fetch policy, quotas |
| [`identity.md`](./docs/identity.md) | Keys, unsigned events and grants, custody, login with device, backup |
| [`privacy.md`](./docs/privacy.md) | Threat model, what leaks, what users wrongly assume |
| [`nip-p2p-auth.md`](./docs/nip-p2p-auth.md) | Peer authentication — the NIP-42 additions covering transports without URLs |

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
