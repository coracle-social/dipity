# Storage

Where events live, who can answer a query, and what happens while the app is asleep.

## The schema

Four tables in the core's SQLite:

| Table | Holds |
| --- | --- |
| `events` | The events themselves. |
| `event_tags` | One row per tag, so NIP-01 tag filters are an index lookup. |
| `event_fts` | Full-text index over content, for search. |
| `event_provenance` | `event_id`, `seen_at`, `peer_pubkey`. Unique on (`event_id`, `peer_pubkey`). |
| `preferences` | A key/value store for storing app policies and ui preferences. |

## The sqlite store

The store is `rusqlite` inside the core with a simple interface for inserting and deleting events and event provenance. This layer also handles processing deletions, addressable events, and syncing tags/fts tables.

The store is accessed in two places. [`sync.md`](./sync.md) accesses the store directly, interpreting the relay protocol and enforcing authentication, policy, validation, grants, etc. This layer only exposes events themselves, not their provenance.

The view layer accesses the database through a capacitorjs `storage` plugin which is a thin wrapper around a view-oriented `core` api, which provides reactive access to events as well as their provenance. Content isn't cached in the `view`, but some metadata (like profiles, follow lists, and mute lists) may be cached for random access. Access should all flow through utilities defined in `src/lib/data`, including content, metadata, aggregation, and writes.

When an event is written to the store (whether from an incoming sync or when the user writes to the store via the `view` layer), it becomes immediately available for propagation to any connected peers whose subscription matches the event, and whom the user's policy grants access.

## Setting up the store

`rusqlite` is built with the bundled amalgamation, so both platforms run one pinned SQLite rather than whatever the OS shipped, and neither `libsqlite3.dylib` nor `android.database.sqlite` is on the path. **There is no storage API in the shell.**

What the shell contributes is small: the database directory, the Keychain or Keystore entry holding the identity key, and the blob directory. The directory is passed in at startup rather than fetched through a callback, since it does not change during a run; the key stays a callback so it can be read on demand and zeroized. See [the call direction](./overview.md#architecture) for why these are traits the core declares rather than platform imports.

On iOS the shell also sets the database's data-protection class, and SQLite's `-wal` and `-shm` sidecars have to carry the same class. A stricter class on any of the three breaks a write during a background wake on a locked phone, which is the failure mode [`AfterFirstUnlock`](./keys.md#signing-happens-at-encounter-time-in-the-background) exists to avoid. The default for app-container files is already the class we want, so the thing to avoid is hardening it later.

## Event provenance

Every peer an event has been seen from, each with the first time they handed it over — the `event_provenance` rows for that event. Accumulated across the store, they are a connectivity graph.

Provenance is never transmitted. It is not part of an event, has no place in an `EVENT` frame, and like `seen_at` it records who the user was physically near and when. It also drives quota accounting.

## Blobs

Blob bytes are stored outside the event store, keyed by SHA-256 hash. Partial transfers persist with their byte offset so a transfer interrupted on BLE resumes later. See [`media.md`](./media.md).

Media is written to disk unsealed, protected by the platform's data-protection class rather than app-layer encryption. See [`privacy.md`](./privacy.md#what-we-do-not-defend-against).
