# Media

Media is in scope. BLE runs at 5–15 KB/s in practice, so it cannot be treated as slow Wi-Fi. See [`overview.md`](./overview.md).

## Events and blobs are separate

Events reference blobs by SHA-256 hash in `imeta` tags. Event sync (see [`sync.md`](./sync.md)) moves references; blob transfer is a distinct protocol on its own channel with its own policy and quotas.

Content addressing makes transfers resumable, dedupable across peers, and verifiable on arrival.

## Three tiers

| Tier | Content | Size | Transport |
| --- | --- | --- | --- |
| 0 | Blurhash/thumbhash, dimensions, duration, mime — inline in the event's `imeta` | bytes | Always, with the event |
| 1 | Preview: downscaled image, or opus voice note | ≤ 32 KB | BLE, under policy |
| 2 | Full-resolution original | unbounded | iroh only; queued for upgrade |

Tier 0 is what makes the timeline render immediately with placeholders when nothing else has transferred. Over BLE alone, that is the difference between an app that feels alive and one that feels broken.

Rough BLE budget: a 32 KB preview is 3–7 s. A 30 s opus voice note at 16 kbps is ~60 KB, so ~10 s. A 2 MB photo is 3–7 minutes, which is why tier 2 is iroh-only.

## Fetch policy

Not a user-facing setting. Three gates, all automatic:

**Trust — reuse the sync scope.** In-scope authors (followed, or within *N* hops) fetch automatically; out-of-scope requires an explicit tap. You already accept their events; a 32 KB cap plus per-peer budget bounds the damage, and someone you follow flooding you with images is a social problem rather than a protocol one.

**Render proximity.** Fetch what is about to scroll into view, not everything that synced. Standard lazy loading, and it collapses most of the quota concern on its own.

**Link quality and battery.** Fetch freely over iroh. Over BLE, require decent RSSI and a battery threshold — on a weak link a 32 KB blob can take 30 seconds and starve other traffic.

## Transfer

- Chunked by offset, resumable across disconnections **and across transports** — a transfer started on BLE continues over iroh with the bytes already received.
- Hash-verified on completion; partial data discarded on mismatch.
- Yields to control traffic (see [`transport.md`](./transport.md)), so the heartbeat survives a large transfer.
- Runs on its own channel so it cannot head-of-line block event sync.

## Quotas

Blob quotas are separate from and much tighter than event quotas:

- Per-peer bytes per session and per rolling 24 h.
- Global storage ceiling with eviction — see [`storage.md`](./storage.md#retention).
- Blobs evict independently of the events referencing them. The timeline entry survives with its tier-0 placeholder, and the blob can be re-fetched on a later encounter.

Accepting blobs from nearby strangers is the most obviously exploitable surface in the design, which is why out-of-scope authors require a tap and never auto-fetch.

## Generation

Tier 1 previews are generated on the sending device at publish time. This costs the author storage but guarantees the preview is available from anyone carrying the event, rather than only from peers holding the tier 2 original.

Voice notes are tier 1 only. A 30 s note at 16 kbps is ~60 KB, small enough that a separate full-resolution copy carries no useful information.
