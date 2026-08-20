# Dip

Offline-first nostr social gossip over Bluetooth. The network is the people physically around you — events propagate device-to-device with no internet dependency in the core loop.

Dip is a nostr client whose transport is proximity. It takes Manyverse's sync model, bitchat's transport engineering, and nostr's data model. Reach is capped at two hops by construction, not by policy.

## How it works

**Discovery.** A bare presence beacon carries no identity. A recognition exchange keyed on per-pair secrets identifies peers without naming anything durable. Completing a handshake with a stranger prompts an introduction ceremony; only after consent does gossip proceed.

**Transport.** Bluetooth is the only transport. Every device runs both GATT roles at once, with a custom framing layer over a Noise XX channel. Bandwidth upgrades to L2CAP over the same connection and Noise session.

**Sync.** Each device is both a nostr relay and a nostr client, reusing the relay wire protocol in both directions. Content events carry an id and no `sig` — authorship comes from the authenticated session at the first hop and a designated-verifier proof at the second. Event reconciliation uses negentropy; blob sync uses sha256 hashes.

**Storage.** SQLite in the core stores everything. `seen_at` and provenance are records of the user's movements and never leave the device. Keys live in platform secure storage and are readable while the device is locked, so pocket-to-pocket gossip works.

## Architecture

Three layers: **core** (Rust), **shell** (Swift/Kotlin), **view** (TypeScript/Svelte 5).

The core owns protocol, crypto, and storage — everything that must survive backgrounding. The shell provides platform capabilities (BLE, Keychain, lifecycle) through traits the core declares. The view is a Capacitor webview, suspended in the background, handling only UI.

Calls run one way: view → shell → core. SQLite is in-process C. The Capacitor bridge marshals as JSON; uniffi passes scalars directly and everything else as compact binary.

## Getting started

```sh
just setup        # install dependencies, add Rust targets
just dev          # Vite dev server (browser only, no BLE)
just core-test    # run Rust tests
just sync         # core → bindings → xcframework → web → cap sync
just ios          # sync, then open Xcode
just android      # sync, then open Android Studio
just qa           # types, lint, format, clippy, tests — what CI runs
```

`just` on its own lists everything. Never run `npx cap sync` directly — it skips the core and bindings build.

Native projects live in `ios/` and `android/` and are committed. After changing web code, `just sync` before building natively; the shells load built assets from `dist/`, not the dev server.

## Stack

TypeScript + Svelte 5 + Tailwind 4 + shadcn-svelte → Rust + uniffi + Swift/Kotlin. Capacitor 8 bridges the halves. Core deps: `coracle-lib` (nostr types), `snow` (Noise XX), `rusqlite` (SQLite), `secp256k1` + `k256` (signing). View deps: welshman `0.9.x` (`util`, `lib`, `domain`).

App ID `social.coracle.dip`. Web build output to `dist/`.

## Documents

| Document | Covers |
| --- | --- |
| [`docs/overview.md`](./docs/overview.md) | Principles, architecture, and a summary of every subsystem |
| [`docs/discovery.md`](./docs/discovery.md) | Advertisement, connection scheduling, consent gate, session lifecycle |
| [`docs/transport.md`](./docs/transport.md) | BLE link layer, framing, Noise XX, the bandwidth upgrade |
| [`docs/sync.md`](./docs/sync.md) | Relay wire protocol as peer protocol, the two registers, reconciliation |
| [`docs/proofs.md`](./docs/proofs.md) | Unsigned events, session auth, authorship proofs, the two-hop cap |
| [`docs/storage.md`](./docs/storage.md) | SQLite as source of truth, `seen_at`, provenance, background serving |
| [`docs/keys.md`](./docs/keys.md) | Identity key custody, storage, backup, signing in the background |
| [`docs/privacy.md`](./docs/privacy.md) | Threat model, what leaks, what users wrongly assume |
| [`docs/ui.md`](./docs/ui.md) | Component framework, design tokens, conventions the linter enforces |
| [`docs/policy.md`](./docs/policy.md) | Trust graph, block vs mute, scope |
