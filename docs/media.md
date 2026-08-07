# Media

Media is in scope. BLE runs at 5–15 KB/s in practice, so it cannot be treated as slow Wi-Fi.

## Events and blobs are separate

Events reference blobs by SHA-256 hash in `imeta` tags. Event sync (see [`sync.md`](./sync.md)) moves references; blob transfer is a distinct protocol on its own channel with its own quotas.

Content addressing makes transfers resumable, dedupable across peers, and verifiable on arrival.

## Three tiers

| Tier | Content | Size | Transport |
| --- | --- | --- | --- |
| 0 | Blurhash/thumbhash, dimensions, duration, mime — inline in the event's `imeta` | bytes | Always, with the event |
| 1 | Preview: downscaled image, or opus voice note | ≤ 32 KB | Automatic, before tier 2 |
| 2 | Full-resolution original | unbounded | Automatic, after tier 1; accumulates across encounters |

Tier 0 renders the timeline immediately, with placeholders wherever nothing else has transferred.

Rough GATT budget: a 32 KB preview is 3–7 s. A 30 s opus voice note at 16 kbps is ~60 KB, so ~10 s. A 2 MB photo is 3–7 minutes, longer than most encounters, so an original needs either an [upgraded link](./transport.md#the-l2cap-bandwidth-upgrade) or several meetings.

## Every referenced blob is fetched

The want list is every hash a stored event references and the device does not hold. There is no per-blob decision: [Accept](./sync.md#scope-is-the-trust-graph) gates ingest against the author of each inbound event, so a stored event has already passed the scope check and its blobs are in scope for the same reason its text is.

Previews go first. Two bands, tier 1 then tier 2, unordered within each. Otherwise one 2 MB original consumes an encounter while fifty previews behind it get nothing.

The core works the list against whoever is connected during [SYNCING](./discovery.md#session-lifecycle), and any peer holding the bytes can serve a want.

## Transfer

- Chunked by offset, resumable across disconnections, **across encounters, and across peers** — bytes from one source combine with bytes from another.
- Hash-verified on completion; partial data discarded on mismatch.
- Started only above an RSSI and battery threshold. On a weak link a 32 KB blob can take 30 seconds and starve other traffic.
- Yields to control traffic (see [`transport.md`](./transport.md)), so the heartbeat survives a large transfer.
- Runs on its own channel so it cannot block event sync, and moves to the [L2CAP channel](./transport.md#the-l2cap-bandwidth-upgrade) wherever one opens. Where it does not, originals take more encounters; the answer to one that will not transfer is more bandwidth inside Bluetooth, never a relay.

## Quotas

Blob quotas are separate from and much tighter than event quotas:

- Per-peer bytes per session and per rolling 24 h.
- A tier-2 cache ceiling in bytes, evicted LRU. Tier 0 and tier 1 are kept with the event, bounded by tier 1's 32 KB cap; tier 2 is unbounded without a ceiling.
- A blob that has not transferred leaves its event intact: the timeline entry renders from the tier-0 placeholder, and the blob arrives on a later encounter.

## Generation

**Images are compressed on import.** What gets stored, and therefore what tier 2 offers, is the compressed image rather than whatever the camera produced.

Tier 1 previews are generated on the sending device at publish time. This costs the author storage but guarantees the preview is available from anyone carrying the event, rather than only from peers holding the tier 2 original.

Voice notes are tier 1 only — at ~60 KB for 30 s, a separate full-resolution copy carries no useful information.
