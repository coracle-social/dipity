# Dip — agent guide

Read [`docs/overview.md`](./docs/overview.md) before making design decisions — it carries what the app is, the principles, and the architecture. This file is the short version plus the things that are easy to get wrong.

The [`dip` skill](./.agents/skills/dip/SKILL.md) maps the code the rules apply to: where each subsystem lives, which parts are built and which are still scaffolding, and the conventions a diff has to match. Load it before an edit.

## Principles

Load-bearing, and stated in full in [`overview.md`](./docs/overview.md#principles). Do not write code that violates them, and flag any request that would.

## Hard rules

Each is a decision already made; the linked doc carries the reasoning.

- **No relay fallback, no hole punching, no DHT, no mDNS, no global discovery.** Every one of those exists to connect peers who are not co-present. [`transport.md`](./docs/transport.md).
- **BLE is the only transport.** Do not add one that needs a discovery service, a rendezvous server, or a relay to reach its peer; reachability is exchanged over the authenticated BLE channel or not at all. L2CAP is the bandwidth upgrade, not a second transport — same connection, same Noise session, no scheme. [`transport.md`](./docs/transport.md#the-l2cap-bandwidth-upgrade).
- **No bridging peers who have not been co-present.** This is what separates the project from bitchat's global-reach path.
- **Content events are never signed.** They carry an id and no `sig`; authorship comes from the session at the first hop and an authorship proof at the second, and there is no third. Do not add a signature, and do not add a mechanism that extends either register. [`proofs.md`](./docs/proofs.md#events-are-not-signed).
- **Never send the author's signature to anyone but the peer it names.** It is portable evidence that attributes the event permanently and to everyone, so the second hop gets a designated-verifier proof instead. [`proofs.md`](./docs/proofs.md#the-authors-signature-stays-with-the-peer-it-names).
- **Every inbound event is authorized by the session or by a proof, or it is dropped.** If the sender authored it, the authenticated session is enough — checked against the pubkeys the peer authenticated as, a set rather than a scalar, since a peer may authenticate as several. Otherwise it needs a proof designated to this device. No policy flag relaxes it. [`proofs.md`](./docs/proofs.md#authorship-proofs).
- **Never claim the second hop cannot identify the author.** A proof is worthless to anyone but its recipient, but the *first* hop holds the author's raw signature and can publish it. Deniability stops at the peer the author chose. [`privacy.md`](./docs/privacy.md#what-deniability-covers).
- **Ids are plain NIP-01 hashes.** Nothing app-specific in serialization, so `@welshman/util` is used unmodified. Do not fold the recipient, the partition, or anything else into the id — reconciliation diffs id sets and would stop converging.
- **The nsec is the only custody model, and the view never signs.** Signatures are produced at encounter time, in a background wake, with no view and maybe no network — so an external signer fails the core loop rather than degrading. Do not add NIP-46 or NIP-55, and do not move signing into TypeScript. [`keys.md`](./docs/keys.md#key-custody).
- **Key bytes never cross the bridge.** Backup export is written and shared by the core and shell; the view starts the flow and gets back shared or canceled, never the string and never the path. [`keys.md`](./docs/keys.md#backup).
- **The identity key is readable while the device is locked** — `AfterFirstUnlock` on iOS, no user-auth requirement on Android. Deliberate: a phone that cannot sign cannot authenticate, and a peer that cannot authenticate can neither send nor receive. Do not "harden" this to `WhenUnlocked`; it silently kills pocket-to-pocket gossip. [`keys.md`](./docs/keys.md#signing-happens-in-the-background).
- **Provenance never leaves the device.** `event_seen` holds one row per event per peer it has been seen from, written once and never updated; an event's `seen_at` is the earliest of them. Neither is part of an event, and neither is ever served to a peer — together they record the user's movements and who they were with. [`storage.md`](./docs/storage.md#the-schema), [`privacy.md`](./docs/privacy.md).
- **Nothing that outlives a session is disclosed before the consent gate.** The Noise static key is generated per handshake, and paired peers are recognized by a MAC over the handshake hash keyed on a per-pair secret — never by a stable key, an epoch-derived tag, or anything else a stranger could collect twice. Noise XX completes before the gate runs, so anything durable in the handshake is a device identifier obtainable on demand by anything in radio range. [`transport.md`](./docs/transport.md#the-static-key-is-generated-per-session), [`discovery.md`](./docs/discovery.md#recognition).
- **Kind 22242 auth events are the only signed nostr events.** An authorship proof rests on a bare signature over `event_id ‖ recipient_pubkey`, which is not an event. Unlike an authorship proof an auth event is *not* designated-verifier, so it is portable evidence binding a pubkey to a channel, which is why that channel's key must not be durable. [`privacy.md`](./docs/privacy.md#the-auth-event-is-portable-evidence).
- **Scope is the trust graph, and trust, block and mute are three things.** Author sets come from explicit trust, never from follows. Block is the wire control — dropped on ingest, never served, sessions refused. Mute (kind 10000) is a display filter and never gates propagation, so do not fold the two together. [`policy.md`](./docs/policy.md#social-graph), [`sync.md`](./docs/sync.md#event-sync).
- **Never claim posts only reach nearby people.** Copy may say the phone trades with devices in range, but copy about who ends up with a post says reach is bounded at two hops, because sync is store-and-forward and a second-hop recipient may be anywhere. [`privacy.md`](./docs/privacy.md).
- **Never claim the app is untrackable.** An active attacker can always complete a handshake; what is true is that no identifier survives a session, so tracking costs continuous observation rather than a single sighting. The user does disclose to strangers, by design, as often as the disclosure bucket allows. [`privacy.md`](./docs/privacy.md#what-an-active-radio-attacker-learns).
- **A comment is one line**, and `just comments` fails a build that says otherwise. That holds for an inline comment and for the comment above a function; a module header is the only place a longer one belongs. A function that needs more explanation than one line needs splitting, not a longer comment — code is readable on its own terms, and documentation and architecture complement it rather than make up for it. `///` and `//!` are documentation and are exempt: rustdoc is the API's text, not a note about the code under it.

## The plugin boundary

The central constraint on the whole app: **the view is suspended in the background, so everything that must survive backgrounding lives in the core.** The test for where a thing goes is whether it runs at *encounter time* — a peer appearing while both phones are in pockets — not which layer it belongs to. The full split is tabulated in [`overview.md`](./docs/overview.md#architecture).

**The core is Rust so the protocol and the crypto exist once**, because both have to agree byte-for-byte with a peer running the *other* platform's build and the OR-proof fails silently when it is wrong. Swift and Kotlin get the parts that are genuinely per-platform and cryptographically dull. [`overview.md`](./docs/overview.md#architecture).

**Calls run one way: view → shell → core.** A platform capability the core needs — the radio, the Keychain, a directory path — is a trait the core declares and the shell implements at startup, never an import pointing the other way. SQLite is `rusqlite` inside the core, not a storage API the shell provides. [`overview.md`](./docs/overview.md#architecture).

**Policy is stored as user preferences and interpreted by the core**, because there is no user to prompt during a background wake. The view edits preferences and computes nothing the core depends on — no author set, no trust graph. [`policy.md`](./docs/policy.md), [`discovery.md`](./docs/discovery.md#the-consent-gate).

Peers speak the **nostr relay wire protocol**: `REQ`/`EVENT`/`EOSE`/`CLOSE`/`OK`/`AUTH`/`NEG-*`, plus four verbs of ours in the same NIP-01 shape — `RECIPIENT-SIGNATURE`, `AUTHORSHIP-PROOF`, `BLOSSOM-REQ` and `BLOSSOM-RES`. The view does not — it reads the store through a query method that filters on seen time and peer, which the relay protocol cannot express. [`sync.md`](./docs/sync.md#the-additions), [`storage.md`](./docs/storage.md#the-sqlite-store).

**The view addresses one store, never a peer.** Queries go over the bridge to the core's SQLite; `ble://` exists only in the core, and there is no way to name a peer from TypeScript — peers get proof checking, `AUTH` and policy, none of which the view has. Events crossing the bridge arrive verified; do not re-check them. [`storage.md`](./docs/storage.md).

**The view keeps no event store.** A controller layer in `src/lib/data/` owns every query and holds per-use-case caches — profiles, trust lists, mutes — never a mirror. Seen-time and peer criteria live on the local query method, never on a NIP-01 filter, so the gossip filter stays something safe to hand a peer. [`storage.md`](./docs/storage.md#the-sqlite-store).

## UI

shadcn-svelte over bits-ui and Tailwind 4. Read [`ui.md`](./docs/ui.md) before touching the view; most of it is enforced by `just lint`, so a violation is a build failure rather than a review comment.

- **`src/app.css` is the only place a design value lives.** Color, elevation, motion and radius are Tailwind tokens. Restyle by changing a token, never by adding a value to a component.
- **No arbitrary values outside `src/lib/components/`** — `bg-[#3a2f28]`, `w-[13px]`. Feature code composes components; components own the pixels. Arbitrary *variants* (`supports-[…]:`, `[&_svg]:`) are fine. [`ui.md`](./docs/ui.md#the-composition-rule).
- **Semantic tokens only** — `bg-card`, not `bg-white`. A palette color is correct in exactly one theme.
- **Runes only in `.svelte` files.** `$state`, `$derived`, `$effect`, `$props` are compiler syntax; in a plain `.ts` module they are an undefined global that fails at runtime. Shared reactive state goes in a `svelte/store` store. [`ui.md`](./docs/ui.md#runes-stay-in-components).
- **No `<style>` blocks in components.** A scoped rule cannot participate in the token system.
- **`src/lib/components/ui/` is generated** by `just ui <name>`. Prettier ignores it and lint is relaxed there; hand-edit only deliberately, because the next `add` overwrites it. Our components go in `src/lib/components/`.
- **Never `{@html}` nostr content.** It is attacker-controlled, and this is the one remotely exploitable mistake available in the view.
- **An event kind is a `KindFactory` in `src/lib/kinds/`** — a reader/writer pair from `@welshman/domain`. No `tags.find(t => t[0] === …)` in a component. [`ui.md`](./docs/ui.md#domain-kinds).
- **A collection of events is a store in `src/lib/data/`.** Components never open a query themselves. Not `src/lib/plugins/` — `plugin` already means the Capacitor boundary here. [`ui.md`](./docs/ui.md#components-do-not-query-the-core).
- **Read [flotilla](https://gitea.coracle.social/coracle/flotilla) for Svelte idiom** before inventing a pattern. Its data layer is `@welshman/app` against real relays and does not transfer.

## Stack and commands

Capacitor 8 · Svelte 5 · Vite 8 · TypeScript · welshman `0.12.x` · Tailwind 4 + shadcn-svelte · Rust + uniffi for the core, with `coracle-lib` for nostr types there.

**Tasks live in the [`justfile`](./justfile), not in `package.json`** — which has no `scripts` block, deliberately, because half the pipeline is `cargo`. `just` on its own lists everything.

```sh
just setup        # rust targets, pnpm deps — once after cloning
just dev          # Vite dev server, over the simulated core in src/lib/dev
just ui <name>    # vendor a shadcn-svelte component into src/lib/components/ui
just lint         # eslint over the view
just fmt          # prettier, eslint --fix, cargo fmt
just qa           # types, lint, format, the Rust half, the Android shell
just core-test    # core tests alone, the fast loop
just bindings     # regenerate Swift + Kotlin from the built cdylib
just sync         # both platforms: sync-android and sync-ios
just sync-android # core → bindings → Android's library → web → cap sync android
just sync-ios     # web → cap sync ios; Xcode builds the core itself
just ios          # sync-ios, then open Xcode
just android      # sync-android, then open Android Studio
```

[`.gitea/workflows/ci.yml`](./.gitea/workflows/ci.yml) runs one step per `qa` recipe on every push to `master`, in two jobs split by toolchain. `android-check` is the exception: provisioning the SDK costs the shared runner more than the compile does, so the Kotlin is compiled by `just qa` and by nothing after the push. Pull requests are not built, so `just qa` locally is the gate before opening one. The Swift is compiled by nothing anywhere — that needs a macOS runner. What CI can read is the project file and the names: `just xcode` fails if a file under `ios/App/App/` is not in the App target, which is the failure that produces a stock app rather than a build error, and `just swift` fails if the shell names an enum case the generated bindings do not have. See [`core/README.md`](./core/README.md#the-xcode-project).

App ID `social.coracle.dip`. Web assets build to `dist/`; the Android shell loads the *built* output, so `just sync-android` after web changes or it runs stale code. iOS rebuilds them on every build.

**The core builds before the shells**, and `just sync` enforces the order — `cargo` cross-compiles for each target, `uniffi-bindgen` generates bindings from the *compiled* library, then `cap sync`. Never run `pnpm exec cap sync` directly; it skips the first two steps and the Android shell links against whatever was there before. The iOS project runs that same order itself, as the App target's first build phase, which is why opening it is all a fresh clone needs. Generated output stages in `core/target/ffi/` and is never committed. [`core/README.md`](./core/README.md#the-xcode-project).


Native projects in `ios/` and `android/` are committed and regenerable. Capacitor does not propagate `appId` changes into them — change `capacitor.config.ts`, then delete and re-add the platforms rather than hand-editing.

## welshman

The view uses [welshman](https://github.com/coracle-social/welshman) `0.12.x` for nostr types and typed kinds. Clone the source into `./ref/welshman` if you need to read or change it — see [Reference materials](#reference-materials).

**Per-package skills are installed** in `.agents/skills/` (symlinked into `.claude/skills/`) — `welshman`, plus `welshman-{app,util,lib,net,store,signer,feeds,domain,content,editor}`. Load the relevant one before working against a package rather than guessing at its API; they are the authoritative reference here. Refresh with `pnpm dlx skills add coracle-social/welshman`.

The packages in use are listed explicitly in `package.json` rather than resolved transitively.

### What welshman is and is not used for

**Used for:** `@welshman/util` (event types, kinds, tags, filters), `@welshman/lib` (standalone helpers), and `@welshman/domain` (typed reader/writer pairs per kind). This is nostr knowledge, consumed unmodified.

**`@welshman/app` is not a dependency.** It assumes an in-memory event store, and there is none. [`ui.md`](./docs/ui.md#organizing-against-welshman).

**Not used for the peer protocol, either half.** Sync begins when a peer appears, which the view is not around for, so `@welshman/net` is not a dependency either — the core reimplements NIP-77 and NIP-42 against the same specifications, taking the negentropy algorithm from `coracle-lib`. [`sync.md`](./docs/sync.md#event-sync).

**`@welshman/signer` survives as an interface only.** Authorship signatures and auth events are signed in the core. Nothing on the gossip path goes through TypeScript.

## Documents

| Document | Covers |
| --- | --- |
| [`overview.md`](./docs/overview.md) | Overview, principles, stack, architecture, and a summary of every subsystem |
| [`discovery.md`](./docs/discovery.md) | Advertisement, connection scheduling, identification, session lifecycle, consent gate, heartbeat |
| [`transport.md`](./docs/transport.md) | BLE link layer and framing, Noise XX, the bandwidth ceiling |
| [`sync.md`](./docs/sync.md) | Relay wire protocol as peer protocol, the two registers, reconciliation, how policy compiles to filters, quotas |
| [`proofs.md`](./docs/proofs.md) | Unsigned events, session auth, authorship proofs, the two-hop cap and where its guarantee stops |
| [`storage.md`](./docs/storage.md) | SQLite in the core as source of truth and relay, `seen_at`, provenance, background serving |
| [`keys.md`](./docs/keys.md) | The nostr identity key: custody, storage, login with device, backup |
| [`privacy.md`](./docs/privacy.md) | Threat model, what leaks, what users wrongly assume |
| [`ui.md`](./docs/ui.md) | Component framework, design tokens, the conventions the linter enforces |
| [`stories.md`](./docs/stories.md) | What a person does with the app, the screen that answers each, and what has no screen yet |
| [`nips/p2p-auth.md`](./docs/nips/p2p-auth.md) | Peer authentication — the NIP-42 additions covering transports without URLs |
| [`nips/imeta-blake3.md`](./docs/nips/imeta-blake3.md) | The `imeta` addition carrying a BLAKE3 root, for verified streaming of blobs |
| [`nips/imeta-preview.md`](./docs/nips/imeta-preview.md) | The `imeta` addition naming the original a preview stands in for |

## Reference materials

Five other codebases inform this design. They may be cloned into `./ref/`, which is gitignored — they are read-only prior art, not part of this app. Do not build, modify, or stage them. Nothing here depends on their being present.

| Reference | Clone URL | License | Consult for |
| --- | --- | --- | --- |
| welshman | `https://github.com/coracle-social/welshman.git` | MIT | Library source for the whole app layer — read it before guessing at an API |
| flotilla | `https://gitea.coracle.social/coracle/flotilla.git` | MIT | Another Coracle app on the same stack; `KeyDownload.svelte` and `lib/html.ts` model the backup flow in [`keys.md`](./docs/keys.md#backup); includes a large corpus of how to use welshman in a svelte project |
| bitchat | `https://github.com/permissionlesstech/bitchat.git` | Unlicense (public domain) | BLE transport engineering — framing, connection scheduling, `BLERecentPeripheralCache`, `GCSFilter` |
| samiz | `https://github.com/KoalaSat/samiz.git` | MIT | nostr gossip over a BLE mesh — the nearest running implementation of this sync layer. Android only |
| manyverse | `https://gitlab.com/staltz/manyverse.git` | **MPL-2.0** | Offline sync model and `hops` scoping |

```sh
mkdir -p ref && git clone <url> ref/<name>
```

### Manyverse license note

**Manyverse is MPL-2.0, which is weak copyleft**, so treat it as read-only prior art. Reading it for design is unrestricted, but copying a file, or a substantial part of one, carries the license with it. Avoid copying verbatim from any reference in any case; they are there primarily as a conceptual guide.
