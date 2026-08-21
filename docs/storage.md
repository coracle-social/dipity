# Storage

Where events live, who can answer a query, and what happens while the app is asleep.

## The schema

| Table | Holds |
| --- | --- |
| `event` | The events themselves, plus earliest `seen_at`: the earliest sighting in `event_seen`, cached on the row because the aggregate cannot be indexed and arrival order is how the feed reads. It is provenance, so it is never part of an event and never served. |
| `event_fts` | Full-text index over content, for search. |
| `event_tag` | One row per tag, so NIP-01 tag filters are an index lookup. |
| `event_seen` | `event_id`, `seen_at`, `peer_pubkey`. Unique on (`event_id`, `peer_pubkey`). |
| `recipient_signature` | `event_id`, `author_pubkey`, `recipient_pubkey`, `sig`. Unique on (`event_id`, `recipient_pubkey`). The author's signature naming a recipient, held by the peer it names. It is the witness an [authorship proof](./proofs.md#authorship-proofs) is built from, never the proof itself, and it is never served to a peer. The author is carried rather than joined, since a signature without the key it is by neither verifies nor proves; a composite foreign key onto `event (id, pubkey)` is what keeps the copy honest. |
| `pref` | A key/value store for storing app policies and ui preferences. |
| `blob` | A mapping of blob sha256 metadata extracted from the first event seen that referenced it. |
| `pair_secret` | `pubkey`, `secret`, `updated_at`. One row per peer this device has paired with, derived from that session's handshake hash. What a later encounter is [recognized](./discovery.md#recognition) from before either side names a pubkey. Provenance, so it is never served. |
| `disclosure` | `pubkey`, `disclosed_at`. One row per pubkey this device has disclosed its identity to, bounding the [disclosure budget](./policy.md#discoverability) per window. Provenance, so it is never served. |

## The sqlite store

The store is `rusqlite` inside the core, with migrations and one instance per open database. This store provides regular query functionality, as well as reactive queries - when a record is written, subscribers should be notified. This allows the UI to be reactive, and for events to be gossiped immediately upon write.

The instance is passed to every query and command rather than reached through a global, and it carries its own change channels, so a process can hold two stores that share neither tables nor subscribers. The shell opens one over the directory it owns and hands it around. Two peers in one process is what exercises the protocol without radios.

`rusqlite` is built with the bundled amalgamation, so both platforms run one pinned SQLite rather than whatever the OS shipped, and neither `libsqlite3.dylib` nor `android.database.sqlite` is on the path. **There is no storage API in the shell.**

What the shell contributes is small: the database directory, the Keychain or Keystore entry holding the identity key, and the blob directory. The directory is passed in at startup rather than fetched through a callback, since it does not change during a run; the key stays a callback so it can be read on demand and zeroized. See [the call direction](./overview.md#architecture) for why these are traits the core declares rather than platform imports.

On iOS the shell also sets the database's data-protection class, and SQLite's `-wal` and `-shm` sidecars have to carry the same class. A stricter class on any of the three breaks a write during a background wake on a locked phone, which is the failure mode [`AfterFirstUnlock`](./keys.md#signing-happens-in-the-background) exists to avoid. The default for app-container files is already the class we want, so the thing to avoid is hardening it later.

## The event store

The event store handles processing deletions, addressable events, syncing tags/fts tables, tracking provenance, and storing/retrieving authorship signatures. This is just a CRUD layer that enforces invariants between event related tables.

## The preference store

This is a think kv layer around the `pref` table.

## The relay store

An in-memory p2p relay implementation implementing the relay side of the [`sync.md`](./sync.md) protocol accesses the event store, interprets the relay protocol, and enforces authentication, policy, validation, authorship proofs, and so on.

## The view store

The view store is an internal interface, accessed by the `view` layer via a capacitorjs plugin. The plugin provides an interface which returns data as snapshots (non-reactive), event emitters (imperative reactivity), and as svelte stores (declarative reactivity).

## Blob store

Blob bytes are stored outside the event store, keyed by SHA-256 hash. Partial transfers persist as a bitmap of verified chunks, so a transfer interrupted on BLE resumes later. See [`sync.md`](./sync.md#blob-sync).

Media is written to disk unsealed, protected by the platform's data-protection class rather than app-layer encryption. See [`privacy.md`](./privacy.md#what-we-do-not-defend-against).
