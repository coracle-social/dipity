---
name: dip
description: "Use this skill when working in the dip repository — the Rust core under core/dip (store, session, sync, transport, model, node), the uniffi surface in core/dip-ffi, the Svelte view in src/, or the design documents in docs/. It maps where each subsystem lives, which parts are built and which are still scaffolding, the invariants a change has to keep true, and the conventions the code holds itself to. AGENTS.md carries the rules and docs/ carries the reasoning; this carries the code."
---

# dip — the codebase

Dip is a nostr client whose transport is proximity: events gossip device to device over BLE, and reach is capped at two hops by construction. `AGENTS.md` states the rules a change must not break, and `docs/` argues for each of them. This skill says where the code is and what shape it has.

## What is built

The design is written down in full. The implementation is not, and the gap is uneven.

| Layer | State |
| --- | --- |
| `core/dip` | Built out. Roughly 18,000 lines, a test module at the foot of nearly every file, and a module for every subsystem in `docs/` |
| `core/dip-ffi` | The uniffi surface. `node` (the radio loop and `Action`), `keys` (the Keychain callback), `store` (queries, preferences and a change callback). Declared by hand: records in, JSON out |
| `ios/`, `android/` | The shell. One `DipPlugin` each over the generated bindings, plus `Radio`, secure storage (`Keychain`, `Keystore`), `Lifecycle` and `Logging`. Nothing here decides anything the core decides |
| `src/` | The view. `src/lib/core.ts` is the plugin boundary and the only way the view reaches the core; `src/lib/kinds/` holds a reader and writer per event kind, `src/lib/data/` the stores over the bridge, `src/lib/components/` the screens, `src/lib/components/ui/` the vendored shadcn set, and `src/lib/dev/` the simulated core a browser gets instead of a shell |

The core is the codebase and the shell is glue. The view is five screens over them: a board, a person, the people list, settings and the pairing ritual, with a composer over the board and a tray above the bar.

Every `core/dip-ffi` entry point has a caller in both shells; read the plugin before adding another. The radio loop, identity storage, the store reads, logging, media authoring and the L2CAP upgrade are wired the whole way through.

L2CAP is the one place a shell holds something the core does not name. `Bulk.swift` and `Bulk.kt` each own one open channel, and the radio owns the map from link to channel, the pending publications and the teardown. Streams and sockets block or call back, so a fragment is reported written from the run loop or from the writer thread rather than from the call that queued it — otherwise the core releases the next one underneath the last and a blob transfer runs on one stack.

Each plugin opens the core in a private `open` and drops it in a private `close`, and `start` is the guard in front of them. That pair exists because `takeTransferredIdentity` reopens the node under the key it just wrote — a shell that only ever opens once would have to restart the app to finish a login with device. The store and the node are one field, opened and dropped together, because there is no call that wants half a core.

A plugin method is one of three shapes, and adding one means picking which: `answer` for a store read, which resolves with what it read; `perform` for a node call the view is waiting on, which resolves with nothing once the actions are carried out; `drive` for a radio or lifecycle event, which nobody is waiting on. Each takes the opened core and applies the actions, so a method that spells the guard and the `try`/`catch` out again is a method doing something those three are not. `clearPreference` and `takeTransferredIdentity` are the two that genuinely are, and `mediaTags` is the one that reaches a free function and so needs no core at all.

The core's `log` goes nowhere until a shell installs a `Logger`, so `Logging.swift` and `Logging.kt` implement one over `os.Logger` and logcat, and each plugin's `load` installs it before the core is opened. The Rust module path is the record's target and becomes the category or the tag, which is the only thing making a subsystem filterable on a device.

A browser has no shell, so `just dev` runs against `src/lib/dev/` — the plugin surface over an in-memory store and a neighborhood on a timer, reachable as `window.dip` for the key backup and the identity transfer, which have no screen. It is loaded by a dynamic import under `import.meta.env.DEV` and is in no shipped bundle. Nothing outside it imports it, and no view code branches on `DEV`: a browser-only fallback in the view is a path that never runs on a device.

## The view

Seven screens under `src/lib/components/`, switched by a `place` store rather than a router: `Board` (the feed, its two orderings and its category filter), `ItemDetail`, `Bookmarks`, `People`, `ContactDetail`, `Settings` and `Pairing`, with `Composer` over the board and `PairingTray` above the bar. `docs/stories.md` says what each one answers.

**A boost or a comment opens what it is about**, rather than its own page. `opensId` in `src/lib/data/feed.ts` is the one rule and `Quoted` draws the embedded line, off `summaryOf` in `src/lib/kinds/` — an article and a calendar event title themselves in a tag rather than in their content. Reacting and boosting are offered on everything, since the user's own event travels whatever this device may do with what it names.

**The board draws nothing whose subject this device does not hold.** A boost names what it passes on rather than carrying it, so one can arrive on its own and draw blank. `grounded` in `src/lib/data/feed.ts` drops a page entry whose subject is missing. A reaction and a vote need no filter, being drawn only on the card of the thing they are about, and a comment is exempt, since it says its own piece.

**Saying something about a thing writes a NIP-22 comment on it**, which is what `Composer`'s "Pass it on" does when the user types. A comment on a comment keeps the root its parent named, so `ItemDetail` opens on the parent and heads the page with a "Commenting on" section climbing one further up. `commentOn` and `commentedOn` in `src/lib/kinds/index.ts` are both halves; nothing else reads a `q` tag.

**The board holds its scroll offset and nothing else does.** `go` in `src/lib/data/nav.ts` records it on the way out and restores it after a `tick`. Every other screen opens at the top, being about the thing that was tapped to reach it.

**Nobody publishes a profile.** A name is one person's name for another, carried in a contact card (`src/lib/kinds/contact.ts`, kind 36017, addressed to the person it names and carrying that name as its content). There is one card per contact, so a name travels with that contact's own events and a visibility rule can withhold one card. It is addressable rather than replaceable, so the retention sweep spares it by name (`is_state` in `db/event/command.rs`). `nameOf` answers a name and whose name it is, and a person two hops out is shown under a neighbour's name for them with the claim attached.

**Trust, block and mute are published events; everything else is a preference.** `src/lib/data/contacts.ts` writes kinds 16017, 16018 and 10000 as plain `p` tag lists, which is what `db/pref/query.rs::graph` reads; `src/lib/data/policy.ts` writes the `policy.*` keys. Editing visibility rewrites the whole document, so `policy.ts`'s `defaults` restates every rule `Visibility::default` carries — the lists at `trusted` and the bookmark list at `nothing`. A rule missing there is dropped the first time the user changes a setting.

**The gate runs before anybody is named**, so the pet name typed while pairing is held against the link and written when the peer identifies itself. The core answers both halves: `Approval.code` is `session::sas` over the handshake transcript at `PAIRING_SPACE`, which is 24^5 and is why `Shapes` draws five, and `peerIdentified` names the peer once it has proved a pubkey.

The first version carries text. Nothing above the bridge attaches a file, and the simulator emits none.

## The core, module by module

Everything below is under `core/dip/src/`. Modules mirror the design documents one to one, and each module's `//!` header names the document it implements. Start there when you are about to change one.

| Module | Holds | Doc |
| --- | --- | --- |
| `node/` | The core as the shell sees it: `Node`, `Action`, the connection `Scheduler` | `discovery.md` |
| `session/` | One link's lifecycle: `State`, the consent `Gate`, `AuthExchange`, `recognition`, `Heartbeat`, `IdentityTransfer` (login with device) | `discovery.md`, `keys.md` |
| `transport/` | `Wire` (the encrypted pipe), `noise` (Noise XX over `snow`), `frame` (channels, `Pipe`, fragmentation, priority) | `transport.md` |
| `sync/` | `relay` (what this device serves), `client` (what it takes in), `blob`, `spending`, `message` | `sync.md` |
| `model/` | The types the store is expressed in: `Policy`, `Query`, `Registers`, `AuthorshipProof`, `Blob`, `Graph` | `policy.md`, `proofs.md` |
| `db/` | `Db`, `Tx`, `query`, `command`, and one submodule per group of tables | `storage.md` |
| `blobs/` | The `BlobStore` trait and its file-backed and in-memory implementations, and `verified`, the Bao proofs a range of a blob travels under | `sync.md` |
| `backup.rs` | The key backup file: `nsec`/`ncryptsec` encoding and the prose around it, written to the shell's cache directory | `keys.md` |
| `clock.rs` | `now()`, and `at()` for pinning it in a test | |
| `keys.rs` | `KeyCustody`, the trait the shell implements over Keychain and Keystore. The identity key is read through it for one use and never held | `keys.md` |
| `link.rs` | `LinkId`, `PeripheralId`, `Role`, the names the core and the shell share for a link | |

Nostr's own types are `coracle-lib`'s and are never redefined here: events, keys, tags, kinds, addresses, filters, NIP-77 items. `coracle-kinds` supplies a reader and writer per event kind, and `model/kinds.rs` holds only the three lists this app defines. Load the `coracle-rust` skill before reaching for any of them.

A stored event is a `HashedEvent`, which carries an id and no signature. `Event` appears in one place, the kind 22242 auth exchange, and is never written to the database.

## The store is the API

`db::query` and `db::command` are the way in and the only way in. There is one function per question asked and per thing that happens, and each takes the `Db`, opens a transaction with `Db::read` or `Db::write`, and threads that `Tx` through however many groups of tables the answer takes.

```
db::query / db::command                    take a Db, own the transaction
  db::{event,blob,pref,pairing,recipient_signature}::{query,command,channel}
                                           take a Tx, never reach for the database
```

- **A group owns every table it touches, and no table is touched from two groups.** An invariant between tables has one place it can be broken.
- **The connection mutex is not reentrant.** A second transaction opened while one is live deadlocks rather than failing, which is why the functions underneath take a `Tx`.
- **Every question about which events is `db::query::list_events`.** What separates the user's feed from a peer's `REQ` is which constraints the `Query` carries, not which function is called. Add a constraint to `Query` rather than a second listing function.
- **`Query` is where authorization rides.** It carries a `Filter` (what was asked for), a `ProvenanceFilter` and a seen-time (local only, and inexpressible to a peer), `Registers` (how far an event may travel), a `PeerPolicy` (what this peer is owed) and an `Order`. `sync::relay` builds one and hands it to `list_events`, so what a peer may see is decided in one place.
- **Writes announce on a channel per group** once their transaction commits. The view re-reads to stay live, and the session layer forwards a newly stored event to whoever is connected, so gossip moves without waiting for the next reconciliation.

The schema is `core/dip/migrations/`, listed in `db/core.rs::MIGRATIONS`, applied in order and recorded in SQLite's `user_version`. Tables are `STRICT`. Ids, pubkeys and signatures are lowercase hex `TEXT`, and `db/sql.rs` is the only place that conversion lives; timestamps are `i64` and need no conversion. Byte counts are `u64` everywhere above the store — a length is not negative and the wire counts in `u64` — and the narrowing to the column's signed integer happens in the command that writes it.

`event_seen` is provenance: one row per event per peer it has been seen from, written once and never updated. It is never part of an event and never served. `event.seen_at` denormalizes the earliest of those rows so arrival order can be indexed.

## Sessions

A `Session` is keyed on `LinkId` and performs no I/O. The lifecycle is `State`, and each phase's bookkeeping rides on its variant, so a phase cannot exist without the data it needs and no flag can disagree with the state it shadows.

```
Linked → Secured → DialerIdentified → Identified → Syncing → Draining → Closed
                                   ↘ GatePending ↗
```

Everything else is a behavior the session composes, each owning its own state: `Wire`, `Heartbeat`, `Gate`, `AuthExchange`, `Relay`, `Client`, `BlobExchange`, `SessionSpending`, `Upgrade`, `IdentityTransfer`. What stays on the session is the lifecycle, the `Peer` the exchange proved, and the policy it is bound under. None of them holds the wire: a behavior that owes the peer an answer returns the payload, and the session sends it.

There are three caps and they answer to different rules. `IDENTIFY_CAP_SECONDS` bounds a link that never names anybody, `GATE_HOLD_SECONDS` is how long the user has to look at their phone, and `DRAIN_CAP_SECONDS` is how long an in-flight transfer gets to finish. The last two are both 300 by coincidence, so do not collapse them.

Recognition, mutual NIP-42, the heartbeat, the L2CAP upgrade and the identity transfer share the control channel, so a control frame carries a one-byte discriminant ahead of its payload. Handshake frames carry none: they are raw Noise messages, and only `State::Linked` ever sees one.

A link has two pipes, not one. GATT carries everything until an L2CAP channel opens; from then on `Channel::Blob` rides the bulk pipe, each pipe has its own write in flight, and a bulk write carries a two-byte length because L2CAP is a stream. `session/l2cap.rs` holds the whole negotiation in `Upgrade`, which answers a control payload for the session to send and a `Step` for the shell to act on: the GATT peripheral publishes and the central connects, so a dialer that wants bulk asks rather than making one. Every failure path leaves the link on GATT — the upgrade is bandwidth, and nothing about it may close anything.

`IdentityTransfer` is the exception to the "each phase rides on its state variant" rule, deliberately: an identity transfer does not stop the session syncing, so it is a field like the other behaviours rather than a `State`. It owns the whole flow and answers each step with the control payload to send; what the session decides is whether the flow may run at all, which is `state == Syncing` and `gate.presence == Foreground` at both ends — not the consent gate.

## Authorization

Two functions in `sync/client.rs` carry the whole of the two-hop cap.

`admits` passes an event when the peer authored it, in which case the authenticated session is proof of authorship to this device and to nobody else, or when the peer holds a proof naming this device, in which case it is a forwarder and the event goes no further. Scope and quota narrow what survives that. Neither widens it. `admissible` is split out to hold everything that disqualifies an event whatever authorizes it — integrity, standing, size, quota, scope — so an event still waiting on its proof can be tested against all of it first.

`ingest` requires a proof to verify under a pair of one of the peer's pubkeys and one of ours. Neither is named on the wire, so the pair it verifies under is the pair it was built for.

`event.verify_id()` is checked here as well as at the wire boundary, deliberately. Both registers authorize an id rather than a body, so an event whose id is not its own hash carries no authorization at all.

`model/authorship_proof.rs` is the arithmetic: a Cramer–Damgård–Schoenmakers OR-proof over `dlog(S)` against `dlog(C)`, made non-interactive with Fiat–Shamir. The one value that has to agree byte for byte with the BIP-340 binding is the challenge, `e = tagged_hash("BIP0340/challenge", R.x ‖ A.x ‖ m)`. Get it wrong and `S` is not the point the signature is over, no proof this device makes verifies against a peer running the other platform's build, and nothing local notices. `AuthorshipProof::simulate` exists so the deniability claim can be tested and is on no path.

## Testing

`cargo test --workspace` from `core/`. Unit tests are colocated in a `#[cfg(test)] mod tests` at the foot of the module they cover. `core/dip/tests/` holds the two that need the public surface: `store.rs` runs the use cases against a real file, migrations and WAL included, and `forgery.rs` establishes that an id which does not name its content authorizes nothing.

The core is sans-io. No test needs a radio, a thread or a sleep.

- **Two sides run in one test.** `secured_pair`, `pump_frame`, `pump`, `full_exchange` and `handshake` in `session/mod.rs` and `node/mod.rs` drive a dialer and a receiver through the real state machine by handing each other bytes. Reach for one of those before writing a mock.
- **Time is pinned, never slept.** `clock::at(instant, || …)` pins a thread-local for one call and restores it while unwinding, so a failing test cannot leave the clock pinned for the ones after it. Pins nest.
- **Fixtures come from a seed.** `fixtures::{secret, author, note, event, blob_hash}` derive real keys and real hashes from a `u8`, so a fixture is reproducible and two seeds never collide. `TempDir` carries a process-wide counter as well as the pid, because parallel tests reading the clock in the same tick would otherwise share a directory.
- **`Db::open_in_memory()`** gives a store that shares neither tables nor channels with any other.
- **A test name is a sentence about behavior**, as in `a_stranger_without_admission_is_held_and_can_be_approved`. The suite reads as a specification of the documents and is worth keeping that way.

## Commands

Every task is in the `justfile`. `package.json` has no `scripts` block, because half the pipeline is `cargo`. `just` on its own lists them.

```sh
just core-test    # cargo test --workspace, the fast loop
just qa           # svelte-check, tsc, eslint, prettier, comments, the Xcode project, cargo fmt, clippy, cargo test, the Swift names, gradle
just fmt          # prettier --write, eslint --fix, cargo fmt
just dev          # Vite, over the simulated core in src/lib/dev: no BLE, but peers and a store
```

`clippy -D warnings` is part of `qa`, so a lint is a build failure here. `qa` is a dependency list and nothing else, and `.gitea/workflows/ci.yml` runs one step per entry, so CI and `just qa` cannot drift. `android-check` is the one carve-out: CI does not run it, so a push that skipped `just qa` leaves the Kotlin compiled by nothing.

`android-check` is in there too, so `qa` wants a JDK and the Android SDK. It compiles the Kotlin against the generated bindings and throws the APK away. The Swift half has no equivalent and needs a Mac, so two checks read it without a compiler: `just xcode` fails on a Swift file that is in no target, or on a project that does not build the core before it compiles. `just swift` fails on a case the shell names that the generated bindings do not have — uniffi capitalizes an error enum's cases (`NodeError.Link`) and lowercases every other enum's (`Action.scan`) — and on a uniffi global called unqualified, since the bindings compile into the App module where a plugin method shadows one.

## Conventions the code holds itself to

`AGENTS.md` states the rules about the design. These are about the shape and the prose, and a diff that ignores them reads as foreign.

- **Every module opens with a `//!` header** saying what it holds and, where one exists, naming the document it implements. Every public item has a doc comment. That is how the code stays legible with so few comments inside the functions.
- **Comments are a single line**, unless at the top of a module, and `just comments` fails a build that says otherwise. `///` and `//!` are documentation and are exempt. A comment says why, and the why usually belongs in the module header or in `docs/`.
- **Two constants that happen to be equal still get separate names and separate documentation**, because they answer to different rules and will diverge.
- **`#![forbid(unsafe_code)]`.**
- **The documents are part of the diff.** `docs/` describes behavior precisely enough that a change makes it wrong, so a change to what a module does carries the document and the `//!` header with it.

## Traps

- **`@welshman/net` and `@welshman/feeds` are installed without being dependencies.** `@welshman/domain` lists them as peers, so npm puts them in `node_modules` and an import of either resolves. `AGENTS.md` rules both out by name. Presence in `node_modules` is not permission to import.
- **`.claude/skills/` is symlinks into `.agents/skills/`.** A new skill needs the directory and the symlink both, or anything reading `.claude/skills/` never sees it. `skills-lock.json` records skills vendored from GitHub by `npx skills add` and is not where a hand-written one goes.
- **`ref/` is gitignored read-only prior art**, and manyverse is MPL-2.0. Read it for design, never copy from it.
- **The core cross-compiles before `cap sync`, always.** `npx cap sync` on its own links the shells against whatever was there before. Go through `just sync`.
- **`coracle-lib` and `coracle-kinds` are released crates.** A git dependency on either does not build: their `src/` is tangled from `book/` and gitignored upstream, so cargo clones the repository and finds no `lib.rs`. The `[patch.crates-io]` block at the foot of `core/Cargo.toml` points both at a local checkout for iterating and is commented out on purpose. Comment it back before committing, or the build stops resolving on every other machine.
