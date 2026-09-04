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
| `disclosure` | `id`, `disclosed_at`. One row per `AUTH` response this device handed an unrecognized peer, bounding the [disclosure budget](./policy.md#discoverability) per window. No recipient: the dialer discloses before the peer has named itself. Rows older than the window are pruned on write. |

## The sqlite store

The store is `rusqlite` inside the core, with migrations and one instance per open database. This store provides regular query functionality, as well as reactive queries - when a record is written, subscribers should be notified. This allows the UI to be reactive, and for events to be gossiped immediately upon write.

The instance is passed to every query and command rather than reached through a global, and it carries its own change channels, so a process can hold two stores that share neither tables nor subscribers. The shell opens one over the directory it owns and hands it around. Two peers in one process is what exercises the protocol without radios.

`rusqlite` is built with the bundled amalgamation, so both platforms run one pinned SQLite rather than whatever the OS shipped, and neither `libsqlite3.dylib` nor `android.database.sqlite` is on the path. **There is no storage API in the shell.**

What the shell contributes is small: the database directory, the Keychain or Keystore entry holding the identity key, and the blob directory. The directory is passed in at startup rather than fetched through a callback, since it does not change during a run; the key stays a callback so it can be read on demand and zeroized. See [the call direction](./overview.md#architecture) for why these are traits the core declares rather than platform imports.

On iOS the shell also sets the database's data-protection class, and SQLite's `-wal` and `-shm` sidecars have to carry the same class. A stricter class on any of the three breaks a write during a background wake on a locked phone, which is the failure mode [`AfterFirstUnlock`](./keys.md#signing-happens-in-the-background) exists to avoid. The default for app-container files is already the class we want, so the thing to avoid is hardening it later.

## Blob store

Blob bytes are stored outside the event store, keyed by SHA-256 hash. Partial transfers persist as a bitmap of verified chunks, so a transfer interrupted on BLE resumes later. See [`sync.md`](./sync.md#blob-sync).

The core ships a file-backed store over a directory the shell provides, the same way it opens SQLite in one. It reaches that store through a trait, which is what lets the sync layer be tested against memory rather than a disk.

A file lives exactly as long as the `blob` row for its hash. Both ways a row goes — LRU eviction, and the deletion of the event that anchors it — announce the removal on the blob channel, and the node deletes the bytes when it drains that channel. The channel is lossy and nothing listens on it while the app is closed, so the node also sweeps the store against the table at open and whenever it finds it has fallen behind. The table is the record; the directory is a cache of it.

Media is written to disk unsealed, protected by the platform's data-protection class rather than app-layer encryption. See [`privacy.md`](./privacy.md#what-we-do-not-defend-against).

## Retention

[Forgetting is the default](./overview.md#principles). An event carried for someone else is kept while it is still circulating and dropped once it stops: the sweep forgets those whose most recent sighting is older than the retention window, 30 days unless the user sets `policy.retention_days`.

The cutoff reads the latest row in `event_seen` rather than the `event.seen_at` the feed orders by, which is the earliest. A sighting is written once per peer, so an event that keeps arriving from peers it has not arrived from before keeps refreshing — repeated propagation, in the only unit a proximity network has. Keying on arrival instead would forget an event on its birthday no matter how many people were still passing it around.

Two things are never swept:

- Events the user wrote. This device is their origin and no peer hands one back, so a sweep would not be letting a copy go, it would be deleting the last one.
- Replaceable events, which are state rather than content. A trust list arrives once and is never offered again, so a sweep reading only circulation would take the [graph](./policy.md#social-graph) that policy is measured against.

It runs when the core opens and at most hourly after that. Opening is the moment that always happens — a device meeting nobody never ticks — and an hour is far below a window measured in days. The query groups `event_seen` by event, which no index answers — affordable at that cadence, and the reason `event.seen_at` is cached on the row for the reads where it would not be.
