# Sync

What moves between peers, how the two sides agree on what's missing, and what policy governs it.

## Authorship

Content events carry an id and no `sig`, so what authenticates an event is what decides how far it travels. There are two registers.

**The session** carries the first hop. An event whose `pubkey` is one the peer authenticated as is proof of authorship to that peer and to nobody else.

**An authorship proof** carries the second. The author may choose to hand the first recipient a signature over the event id and that recipient's pubkey. The recipient can then forward the event with a designated-verifier proof of holding it. The mechanism and its limits are in [`proofs.md`](./proofs.md).

Ingest applies this once, in the core. An event is accepted from the peer that authored it, tested against the set of pubkeys that peer authenticated as, or from a forwarder presenting a proof designated to this device. Everything else gets dropped.

## Event sync

Once a connection reached `SYNCING` status, each side can initiate synchronization by requesting data it is missing. Event syncing uses the nostr client/relay protocol without modification. Each peer acts as both a client and a relay.

Sync begins with a NIP 77 NEGENTROPY sync. The opening filter names no authors, because the peer reads the filter and a list of authors would hand it the contact graph. The dialing side's set is everything it holds plus every id it has refused — a deleted event, a superseded version, or an author outside its Accept scope — so the peer does not deliver a refused event again on every encounter. Peers may respond with fewer than the requested events depending on their visibility and gossip policies, and every event is checked against the receiver's accept policy.

Once the negentropy reconciliation is complete, a regular `REQ` is used to retrieve the desired events. Syncing is paginated in reverse chronological order by `created_at` timestamp with dynamic since/until windows.

### The additions

`REQ`, `EVENT`, `EOSE`, `CLOSE`, `OK`, `AUTH` and `NEG-*` are NIP-01 and NIP-77 unmodified. Four verbs are ours, in the same shape.

| Message | Carries |
| --- | --- |
| `["RECIPIENT-SIGNATURE", <sub>, <event id>, <64-byte sig, hex>]` | An author's signature over `event_id ‖ recipient_pubkey`, for an event the sender wrote |
| `["AUTHORSHIP-PROOF", <sub>, <event id>, <160-byte proof, hex>]` | A designated-verifier proof, for an event the sender is forwarding |
| `["BLOSSOM-REQ", <id>, <method>, <path>, <headers>, <body>]` | A Blossom request, wrapped because there is no HTTP on the link |
| `["BLOSSOM-RES", <id>, <status>, <headers>, <body>]` | Its response, correlated by the shared `<id>` |

The first two exist because an unsigned event needs something alongside it that the relay protocol has no way to carry. The proof's 160 bytes are five 32-byte fields in order: the author's signature nonce point, then the challenge and response of the author branch, then the challenge and response of the verifier branch. [`proofs.md`](./proofs.md#authorship-proofs) has the construction.

The sender attaches recipient signatures or authorship proofs to an event, never both, and one per pubkey the receiving peer authenticated as. Each follows the `EVENT` on the same subscription, and the receiver keys it on the event id rather than on arrival order, since either may be dropped without the other. A device that has neither sends the bare event, which then goes no further than the peer receiving it. The BLOSSOM pair drives [blob sync](#blob-sync).

### How policy reaches the wire

Policy never becomes a NIP-01 filter. A filter is positive-only, so a scope like "anyone except the people I blocked" has no expression in one, and a filter is something a peer reads — compiling the contact graph into one would hand it over. Scope is applied where the events are read instead.

| Setting | Applied | Where |
| --- | --- | --- |
| Sharing, and [relaying](./policy.md#relaying) only to contacts | to what this device serves | the store query answering a peer, before the limit |
| The two [registers](./proofs.md#authorship-proofs) | to what may travel at all | the same query |
| Accept | to what this device stores | ingest, per event, against the author |

A peer's `REQ` therefore contributes only its filter. The registers and the scopes are added on this side, and a peer cannot widen either.

Reconciliation is bounded from both ends by the same rule, with one asymmetry. Answering a peer's negotiation uses exactly the query a `REQ` would, so a device never advertises holding something it would refuse to serve. Opening one uses everything this device **holds**, because the question is what it is missing: an event that is already stored has to be in the set even when it is offerable to nobody, or it is reported missing on every encounter and fetched forever.

### Resyncing

A change to block, a contact card, or the Accept and Sharing settings applies to what is already stored and to every live session. What the new Accept scope no longer admits is evicted, unless the user bookmarked it, and is remembered as refused so reconciliation does not offer it again. The old settings' refusals are forgotten, so what the new ones admit is fetched.

Each live session then reconciles again. A device that receives a `NEG-OPEN` beyond the number it has opened itself answers with one of its own, so a change on either side makes both sides pull, and the counts stop the exchange at one round each.

A recipient signature withheld because the peer was not a contact, or because Sharing was `contacts`, is owed, not lost. `event_shared` records whether one went with each handoff, and the user's own events handed over unsigned get their signatures once signing is allowed: on the resync, or when the two next meet.

### Quotas

Accepting gossiped events is an unbounded write from whoever is standing nearby. Independent of scope:

- Per-peer event-count and byte budgets over a rolling 24 h window. A session's own accepts land in the same window, so one meter bounds the session and the reconnect alike.
- Per-event size cap.
- A separate, smaller budget for peers who are not contacts, and above it a hard ceiling on what all of them together may write, which cannot crowd out contacts. bitchat's courier trust tiers are the pattern.

The ceiling is the one that has to hold, because the per-peer budget below it does not bind a stranger. Content events carry no signature, so an identity costs an attacker a keypair: metering per pubkey assumes identity is expensive, and here it is free. The ceiling is keyed on nothing at all, so there is nothing for a burner to reset.

The window is kept in the store as well as in memory, so a background relaunch refills nothing. Only what was stored is charged: a duplicate, or an event whose proof did not verify, costs the peer nothing.

## Blob sync

Blobs follow, on their own channel. They are addressed by the SHA-256 in the event's `imeta` tag and verified against the BLAKE3 root also included in the `imeta` tag ([`nips/imeta-blake3.md`](./nips/imeta-blake3.md)). Content addressing makes transfers resumable, dedupable across peers, and verifiable group by group as they arrive.

The want list is every hash a stored event references and the device does not hold. There is no per-blob decision: [Accept](./policy.md#accept) gates ingest against the author of each inbound event, so a stored event has already passed the scope check and its blobs are in scope for the same reason its text is. Previews take precedence over originals, and within a role a partly-fetched blob takes precedence over one not yet started, so the most nearly complete transfer is the first to be finished.

Each group verifies against the BLAKE3 root as it arrives, so a bad group costs one group and names the peer that sent it. A forwarder cannot alter the root: it rides in `imeta`, and the event id commits to it. A transfer that drops leaves the groups that proved out on disk, and the next session picks the blob up there, from whichever peer is in range then.

Blob sync is only started above an RSSI and battery threshold. On a weak link a 32 KB blob can take 30 seconds and starve other traffic. It also yields to control traffic, so the heartbeat survives a large transfer, runs on its own channel so it cannot block event sync, and moves to the L2CAP channel wherever one opens.

To detect the presence of a blob, we first run `HEAD /<sha256>`. This also gives us the content type, and length. If only part of the blob is held, content-range should describe the range available (groups are fetched in order).

We then request each group individually using `GET /<sha256>` with a `range` header and, where the blob carries a root, `accept-encoding: bao`. The answer to that is a [Bao](https://github.com/oconnor663/bao) slice over the range — the subtree hashes on the path down to those bytes, then the bytes — under `content-encoding: bao`. The fetcher checks the whole answer against the root before a byte of it reaches the disk, so nothing about the sending device has to be believed, and nothing has to be fetched or stored ahead of the bytes to make that possible.

A group is 16 KiB, which is sixteen of Bao's 1 KiB chunks. The chunk size is Bao's and is not ours to raise: verification is defined over 1 KiB, so a range starting inside a chunk pays for the whole chunk anyway, and sizing the group as a whole number of chunks is what keeps a slice from carrying one the fetcher already holds. The proof is around a sixteenth of the group it proves. It rides the same radio as the bytes, so both budgets are spent on the size of the answer rather than on the size of what it carried.

A device proves a range out of the outboard tree over its own copy, which it builds once the file is whole and keeps beside the bytes. Only a whole copy can be proved that way, so a `HEAD` offers `accept-encoding: bao` only from one — and a peer holding a prefix of a rooted blob is passed by rather than taken from, since unproved bytes are what the root exists to end. A blob whose `imeta` carries no root at all is fetched from a peer holding the entire file, checked once against the SHA-256 at the end, and started over when a transfer drops.

Each exchange then travels as the two BLOSSOM verbs of [the additions](#the-additions), on the blob channel — its own queue, yielding to control and sync, and the one thing that moves to L2CAP where [an upgrade](./transport.md#the-l2cap-bandwidth-upgrade) opens.

When an event references a blob, we save a record to the `blob` table which maps the sha256 to the blob's metadata - including everything in the `imeta` tag, and whether the blob is a `preview` or an `original`. A tag is a `preview` when it names the original it stands in for ([`nips/imeta-preview.md`](./nips/imeta-preview.md)); anything else is an `original`, so a marker that names no blob buys nothing the roles below hand out. The metadata is the first referring event's, by `seen_at` rather than `created_at`: a later event's copy of the tag says nothing about bytes the hash already addresses.

Which events reference a hash is `blob_reference`, one row per pair, and it is where both the blob's lifetime and its permissions come from. The record and its bytes go when the last reference does ([`storage.md`](./storage.md#blob-store)), so deleting one event never takes media another still names.

A peer is served the bytes when it may be served every event that references them. The narrowest reference wins rather than the widest: the widest would let anyone who learned a hash publish an event naming it and be handed media they were never offered. The cost is that a reference the peer may not see withholds a blob it otherwise could have had, which is the direction that grants nothing.

### Quotas

Blob quotas are separate from and much tighter than event quotas:

- Per-peer bytes per rolling 24 h in each direction, counted across sessions.
- A `original` cache ceiling in bytes, evicted LRU. Previews are kept as long as their events are. Serving a blob's bytes to a peer is what marks it used, and is the only read of them the core has.
