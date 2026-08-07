# Media

Media is in scope. BLE runs at 5–15 KB/s in practice, so it cannot be treated as slow Wi-Fi.

## Events and blobs are separate

Events reference blobs by SHA-256 hash in `imeta` tags. Event sync (see [`sync.md`](./sync.md)) moves references; blob transfer is a distinct protocol on its own channel with its own policy and quotas.

Content addressing makes transfers resumable, dedupable across peers, and verifiable on arrival.

## Three tiers

| Tier | Content | Size | Transport |
| --- | --- | --- | --- |
| 0 | Blurhash/thumbhash, dimensions, duration, mime — inline in the event's `imeta` | bytes | Always, with the event |
| 1 | Preview: downscaled image, or opus voice note | ≤ 32 KB | BLE, under policy |
| 2 | Full-resolution original | unbounded | Explicit request only; accumulates across encounters |

Tier 0 is what makes the timeline render immediately with placeholders when nothing else has transferred.

Rough BLE budget: a 32 KB preview is 3–7 s. A 30 s opus voice note at 16 kbps is ~60 KB, so ~10 s. A 2 MB photo is 3–7 minutes, which is why tier 2 is never automatic.

**Tier 2 is user-initiated and patient.** BLE is the only transport, so there is no faster link to promote a transfer onto ([`transport.md`](./transport.md#what-fits-in-an-encounter)). An original moves only when someone asks for it, and then outlives the encounter: transfers are chunked by offset and content-addressed, so bytes accumulate across encounters and across peers until the hash verifies. A 2 MB photo may take several meetings, or never complete. The answer to unreliable originals is [L2CAP](./transport.md#the-l2cap-bandwidth-upgrade), never a relay.

## Fetch policy

Not a user-facing setting. Three gates, all automatic:

**Trust — reuse the sync scope.** Authors inside the [Accept scope](./sync.md#scope-is-the-trust-graph) fetch automatically; anyone outside it requires an explicit tap. You already accept their events; a 32 KB cap plus per-peer budget bounds the damage, and someone you trust flooding you with images is a social problem rather than a protocol one.

**Render proximity.** Fetch what is about to scroll into view, not everything that synced. Standard lazy loading, and it collapses most of the quota concern on its own.

**Link quality and battery.** Require decent RSSI and a battery threshold — on a weak link a 32 KB blob can take 30 seconds and starve other traffic.

## Transfer

- Chunked by offset, resumable across disconnections, **across encounters, and across peers** — content addressing means bytes from one source combine with bytes from another.
- Hash-verified on completion; partial data discarded on mismatch.
- Yields to control traffic (see [`transport.md`](./transport.md)), so the heartbeat survives a large transfer.
- Runs on its own channel so it cannot head-of-line block event sync.

## Quotas

Blob quotas are separate from and much tighter than event quotas:

- Per-peer bytes per session and per rolling 24 h.
- A blob that has not transferred leaves its event intact: the timeline entry renders from the tier-0 placeholder, and the blob can be fetched on a later encounter.

Accepting blobs from nearby strangers is the most obviously exploitable surface in the design.

## Generation

**Images are compressed on import.** What gets stored, and therefore what tier 2 offers, is the compressed image rather than whatever the camera produced.

Tier 1 previews are generated on the sending device at publish time. This costs the author storage but guarantees the preview is available from anyone carrying the event, rather than only from peers holding the tier 2 original.

Voice notes are tier 1 only — at ~60 KB for 30 s, a separate full-resolution copy carries no useful information.
