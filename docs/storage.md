# Storage

Where events live, who can answer a query, and what happens while the app is asleep.

## The schema

| Table | Holds |
| --- | --- |
| `event` | The events themselves, plus earliest `seen_at`: the earliest sighting in `event_seen`, cached on the row because the aggregate cannot be indexed and arrival order is how the feed reads. It is provenance, so it is never part of an event and never served. |
| `event_fts` | Full-text index over content, for search. |
| `event_tag` | One row per tag, so NIP-01 tag filters are an index lookup. |
| `event_seen` | `event_id`, `seen_at`, `pubkey`. Unique on (`event_id`, `pubkey`). |
| `event_refused` | `id`, `created_at`, `by_policy`, `refused_at`. One row per event this device was offered and would not store, or that the [retention sweep](#retention) let go, which [reconciliation](./sync.md#event-sync) counts as held and the store will not take back. Names no peer. A refusal the Accept scope made goes when the policy is recompiled; the rest are capped by count. |
| `event_trashed` | `event_id`, `trashed_at`. One row per event the user put in the [trash](#the-trash). Local, and goes with its event by cascade. |
| `spending` | `pubkey`, `meter`, `pooled`, `spent_at`, `bytes`. The [quota](./sync.md#quotas) ledger: what each peer wrote to this device inside the rolling 24 hours, so the window survives a relaunch. Rows older than the window go as new ones are written. It names who handed this device something and when, so like `event_seen` it is never served. |
| `event_shared` | `event_id`, `pubkey`, `shared_at`, `signed`. Unique on (`event_id`, `pubkey`). One row per peer this device has handed an event to, written where the event is served. Provenance in the other direction from `event_seen`, so it is never served either, and a later handoff to the same peer is ignored — a row says this device carried something to somebody rather than how often it answered for it. `signed` says the user's recipient signature went with it, and one that did not is [sent once forwarding is allowed](./sync.md#resyncing). |
| `recipient_signature` | `event_id`, `author_pubkey`, `recipient_pubkey`, `sig`. Unique on (`event_id`, `recipient_pubkey`). The author's signature naming a recipient, held by the peer it names. It is the witness an [authorship proof](./proofs.md#authorship-proofs) is built from, never the proof itself, and it is never served to a peer. The author is carried rather than joined, since a signature without the key it is by neither verifies nor proves; a composite foreign key onto `event (id, pubkey)` is what keeps the copy honest. |
| `pref` | A key/value store for storing app policies and ui preferences. |
| `blob` | A mapping of blob sha256 to the metadata in the `imeta` tag of the first event seen to reference it, plus how much of the file is on disk. |
| `blob_reference` | `sha256`, `event_id`. One row per event that references a hash. The record and its bytes live as long as any of them, and a peer is served the bytes only when it may be served all of them. |
| `pair_secret` | `pubkey`, `secret`, `updated_at`. One row per peer this device has paired with, derived from that session's handshake hash. What a later encounter is [recognized](./discovery.md#recognition) from before either side names a pubkey. Provenance, so it is never served. |
| `disclosure_bucket` | `tokens`, `updated_at`. One row holding the level of the [disclosure bucket](./policy.md#discoverability) when it was last spent from, and absent while full. It names no recipient, because the dialer discloses before the peer has named itself, and keeps no history, because a log of strangers met would record where the user has been. |

An event with an address — replaceable or addressable — keeps one row for it, and the later `created_at` wins. Two versions carrying the same second are settled by the lower id, NIP-01's own rule, so the loser is refused outright rather than stored beside the winner. A write is therefore not proof that what it wrote is what a read answers, and anything rewriting a list it already holds stamps past the version it replaces.

## The sqlite store

The store is `rusqlite` inside the core, with migrations and one instance per open database. This store provides regular query functionality, as well as reactive queries - when a record is written, subscribers should be notified. This allows the UI to be reactive, and for events to be gossiped immediately upon write.

The instance is passed to every query and command rather than reached through a global, and it carries its own change channels, so a process can hold two stores that share neither tables nor subscribers. The shell opens one over the directory it owns and hands it around. Two peers in one process is what exercises the protocol without radios.

`rusqlite` is built with the bundled amalgamation, so both platforms run one pinned SQLite rather than whatever the OS shipped, and neither `libsqlite3.dylib` nor `android.database.sqlite` is on the path. **There is no storage API in the shell.**

What the shell contributes is small: the database directory, the Keychain or Keystore entry holding the identity key, and the blob directory. The directory is passed in at startup rather than fetched through a callback, since it does not change during a run; the key stays a callback so it can be read on demand and zeroized. See [the call direction](./overview.md#architecture) for why these are traits the core declares rather than platform imports.

On iOS the database and SQLite's `-wal` and `-shm` sidecars keep the default data-protection class for app-container files, which is the class we want. A stricter class on any of the three breaks a write during a background wake on a locked phone, the failure [`AfterFirstUnlock`](./keys.md#signing-happens-in-the-background) exists to prevent, so the class must not be hardened later.

The shell excludes the database and the blob directory from device backups, with `isExcludedFromBackup` on iOS and `noBackupFilesDir` on Android. `event_seen` and `event_shared` record who the user was near, and [provenance never leaves the device](./overview.md#principles), including into a cloud backup. Android's backup rules also exclude the Keystore-wrapped identity, because its wrapping key does not travel with a backup and a restored copy cannot be read.

## Blob store

Blob bytes are stored outside the event store, keyed by SHA-256 hash. A partial transfer persists as the count of proved bytes on disk, so a transfer interrupted on BLE resumes later. Groups are fetched in order, so what is on disk is always a prefix and that count is the whole record of progress. A blob held whole is stored with the outboard BLAKE3 tree over it beside the bytes, under the same name with an `.obao` suffix; that is what the device proves a range with when a peer asks for one. See [`sync.md`](./sync.md#blob-sync).

The core ships a file-backed store over a directory the shell provides, the same way it opens SQLite in one. It reaches that store through a trait, which is what lets the sync layer be tested against memory rather than a disk.

A file lives exactly as long as the `blob` row for its hash, and that row lives as long as any event references it. Both ways a row goes — LRU eviction, and the deletion of the last event to reference it — announce the removal on the blob channel, and the node deletes the bytes when it drains that channel. The channel is lossy and nothing listens on it while the app is closed, so the node also sweeps the store against the table at open and whenever it finds it has fallen behind. The table is the record; the directory is a cache of it.

That holds for the device's own media too, so the bytes the user attaches reach the core with the event that names them rather than while the post is being written. The BLAKE3 root has to be known before the event is signed, so the core answers the `imeta` entries from the bytes alone and stores nothing until there is a row to keep them: an abandoned composition leaves the store as it found it.

Media is written to disk unsealed, protected by the platform's data-protection class rather than app-layer encryption. See [`privacy.md`](./privacy.md#what-we-do-not-defend-against).

## Retention

[Forgetting is the default](./overview.md#principles). An event carried for someone else is dropped once it has been on this device longer than the retention window, 90 days unless the user sets `policy.retention_days`. The window starts at `event.seen_at`, when the event first reached this device, which is the same time the feed orders by and the one a user can see.

Age plays no part. A post written a year ago that reaches this device today is new here, and is kept and ordered as such. Circulation plays none either: later sightings do not extend the window. Measuring from the latest sighting looked like counting repeated propagation, but a peer offering back what this device handed it counts as a sighting too, so a community passing a post around kept it alive on every phone for as long as any two of them kept meeting. What spreads still spreads, because each phone it reaches gets its own window.

What the sweep forgets it also refuses, in `event_refused`, which reconciliation counts as held. Otherwise the peer it was handed to would offer it straight back, it would arrive as new, and the window would start again. The store checks refusals on every save, so a live push is turned away too. These refusals are kept up to 50,000, newest first, which is a few megabytes; one that falls off the end lets its event back in as new, for one more window. Refusals the user's settings made are cheap to make again and age out with the window instead.

Four things are never swept:

- Events the user wrote. This device is their origin and no peer hands one back, so a sweep would not be letting a copy go, it would be deleting the last one.
- Replaceable events, which are state rather than content. A trust list arrives once and is never offered again, so a sweep reading only circulation would take the [graph](./policy.md#social-graph) that policy is measured against. A [contact card](./policy.md#social-graph) is addressable rather than replaceable, so the sweep spares its kind by name, since sweeping one would leave a person nameless on a device that still holds their writing.
- Deletion requests, kind 5. The store refuses an event a stored request covers, so sweeping the request would let the deleted event back in the next time a peer offers it.
- Events the user bookmarked. Circulation is a measure of what the neighbourhood is still interested in, and a bookmark is the one place the person holding the device says otherwise. The sweep reads the `e` tags on their own NIP-51 list, kind 10003, which is replaceable and so survives on the rule above; somebody else's bookmark list keeps nothing here.

It runs when the core opens and at most hourly after that. Opening is the moment that always happens — a device meeting nobody never ticks — and an hour is far below a window measured in days. The query groups `event_seen` by event, which no index answers — affordable at that cadence, and the reason `event.seen_at` is cached on the row for the reads where it would not be.

## Dropping one thing

The sweep is the device deciding; dropping is the user deciding. Either removes the event, its sightings, the record of who it was handed to, the author's signature over it and the media it references, and both go through the same delete.

Dropping is local and tells nobody. A device holds somebody else's writing at their author's sufferance and can stop holding it at any time, but it cannot ask the neighbourhood to do the same — only the author can, by publishing a kind 5, which travels the way the event did and is a request rather than an instruction. So the two are separate operations.

Nothing stops a dropped event arriving again from somebody who still has it. Dropping is not a block, and refusing a person's events is [policy](./policy.md#accept).

### The trash

The user drops and retracts through the trash. Throwing something out marks it in `event_trashed`, which hides it from the board and from the saved bookmarks, and nothing in the trash is served to a peer. Throwing out the user's own event also retracts it there and then, with a kind 5 the core writes itself; anybody else's goes in telling nobody. Retracting and keeping are separate: the retracted event stays in the trash on this phone like anything else.

A kind 5 that arrives does the same to what it names, putting it in the trash rather than deleting it, so somebody else's retraction leaves the board at once and still gives the user the week to look at it. `event_trashed.deletion` records which kind 5 put a thing there.

Putting the user's own event back undoes its retraction for everybody, by a kind 5 naming the kind 5 that retracted it. Wherever that arrives, the retracted kind 5 goes in the trash, stops counting as a deletion, and takes back out exactly what it put in, leaving what the user threw out by hand where it is. The refusals it caused are forgotten, so a device that never held the event can take it again. Somebody else's event comes back on this phone only, and only if its author has not retracted it: the trash marks what its author retracted, and that cannot be put back by anybody but the author. Everything in the trash is dropped from this device when the trash is emptied, by hand or by the hourly sweep once it has been there a week. Emptying writes a kind 5 for any of the user's own events that went in without one, so a retraction goes out while the view is suspended.

## Notifications

Notifications are local, because nothing here talks to a server. The core raises them, because the view is suspended whenever one would matter, and only while the app is in the background. It raises only what the user switched on in `notifications.pairing` and `notifications.content`, which are both off until the user switches them on.

A pairing notification goes out once for each person the user has not named, or once per held link while the gate does not yet know who is on it. New writing is counted rather than announced one event at a time. It covers notes, comments, polls, calendar entries and articles from anybody the user has not muted, and not reactions, boosts, deletions, lists or contact cards. The count runs from the last time the app was open and the notification updates at most once a minute, so a first sync that brings in a hundred things is one notification. It quotes the latest post, up to 140 characters or its title, under the user's name for its author when they have one. Opening the app clears both.

The view offers notifications once, after the user has published writing twice, and asks the phone for permission only when the user switches one on.
