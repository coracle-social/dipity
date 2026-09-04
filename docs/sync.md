# Sync

What moves between peers, how the two sides agree on what's missing, and what policy governs it.

## Authorship

Content events carry an id and no `sig`, so what authenticates an event is what decides how far it travels. There are two registers.

**The session** carries the first hop. An event whose `pubkey` is one the peer authenticated as is proof of authorship to that peer and to nobody else.

**An authorship proof** carries the second. The author may choose to hand the first recipient a signature over the event id and that recipient's pubkey. The recipient can then forward the event with a designated-verifier proof of holding it. The mechanism and its limits are in [`proofs.md`](./proofs.md).

Ingest applies this once, in the core. An event is accepted from the peer that authored it, tested against the set of pubkeys that peer authenticated as, or from a forwarder presenting a proof designated to this device. Everything else gets dropped.

## Event sync

Once a connection reached `SYNCING` status, each side can initiate synchronization by requesting data it is missing. Event syncing uses the nostr client/relay protocol without modification. Each peer acts as both a client and a relay.

Sync begins with a NIP 77 NEGENTROPY sync over events accepted from this peer according to policy. Peers may respond with fewer than the requested events depending on their visibility and gossip policies, and every event should be checked against the receiver's accept policy.

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

At most one of the first two accompanies an event. It follows the `EVENT` on the same subscription, and the receiver keys it on the event id rather than on arrival order, since either may be dropped without the other. A device that has neither sends the bare event, which then goes no further than the peer receiving it. The BLOSSOM pair drives [blob sync](#blob-sync).

### How policy reaches the wire

Policy never becomes a NIP-01 filter. A filter is positive-only, so a scope like "anyone except the people I blocked" has no expression in one, and a filter is something a peer reads — compiling a trust graph into one would hand it over. Scope is applied where the events are read instead.

| Setting | Applied | Where |
| --- | --- | --- |
| Gossip, visibility | to what this device serves | the store query answering a peer, before the limit |
| The two [registers](./proofs.md#authorship-proofs) | to what may travel at all | the same query |
| Accept | to what this device stores | ingest, per event, against the author |

A peer's `REQ` therefore contributes only its filter. The registers and the scopes are added on this side, and a peer cannot widen either.

Reconciliation is bounded from both ends by the same rule, with one asymmetry. Answering a peer's negotiation uses exactly the query a `REQ` would, so a device never advertises holding something it would refuse to serve. Opening one uses everything this device **holds**, because the question is what it is missing: an event that is already stored has to be in the set even when it is offerable to nobody, or it is reported missing on every encounter and fetched forever.

### Quotas

Accepting gossiped events is an unbounded write from whoever is standing nearby. Independent of scope:

- Per-peer event-count and byte budgets over a rolling 24 h window. A session's own accepts land in the same window, so one meter bounds the session and the reconnect alike.
- Per-event size cap.
- A separate, smaller budget for untrusted peers, and above it a hard ceiling on what every untrusted peer together may write, which cannot crowd out known peers. bitchat's courier trust tiers are the pattern.

The ceiling is the one that has to hold, because the per-peer budget below it does not bind a stranger. Content events carry no signature, so an identity costs an attacker a keypair: metering per pubkey assumes identity is expensive, and here it is free. The ceiling is keyed on nothing at all, so there is nothing for a burner to reset.

## Blob sync

Blobs follow, on their own channel. They are addressed by the SHA-256 in the event's `imeta` tag and verified against the BLAKE3 root also included in the `imeta` tag ([`nips/imeta-blake3.md`](./nips/imeta-blake3.md)). Content addressing makes transfers resumable, dedupable across peers, and verifiable group by group as they arrive.

The want list is every hash a stored event references and the device does not hold. There is no per-blob decision: [Accept](./policy.md#accept-and-gossip) gates ingest against the author of each inbound event, so a stored event has already passed the scope check and its blobs are in scope for the same reason its text is. Previews take precedence over originals, and within a role a partly-fetched blob takes precedence over one not yet started, so the most nearly complete transfer is the first to be finished.

Each group verifies against the BLAKE3 root as it arrives, so a bad group costs one group and names the peer that sent it. A forwarder cannot alter the root: it rides in `imeta`, and the event id commits to it. A transfer that drops leaves the groups that verified on disk, and the next session picks the blob up there — from whichever peer is in range then, and from one holding no more than a prefix, since a prefix is worth taking once each group of it verifies.

Blob sync is only started above an RSSI and battery threshold. On a weak link a 32 KB blob can take 30 seconds and starve other traffic. It also yields to control traffic, so the heartbeat survives a large transfer, runs on its own channel so it cannot block event sync, and moves to the L2CAP channel wherever one opens.

To detect the presence of a blob, we first run `HEAD /<sha256>`. This also gives us the content type, and length. If only part of the blob is held, content-range should describe the range available (groups are fetched in order).

Before any bytes move we fetch the blob's group tree with `GET /<sha256>/blake3`: the BLAKE3 chaining value of each group, end to end. Folded the way BLAKE3 folds its own tree those produce the root in `imeta`, so a tree that produces it is the author's and every group under it is committed to by the event id. We then request each group individually using `GET /<sha256>` with a `range` header, and check its bytes against that group's chaining value before writing them.

The whole tree is a thirty-second of a percent of the blob, which is less than a [Bao](https://github.com/oconnor663/bao) slice per group costs, and a tree the fetcher holds outright needs no re-sending when the transfer is picked up again. A device serving a blob it holds whole builds the tree from the bytes and keeps it; one part way through a transfer of its own serves the tree it verified, which is what lets it prove the prefix it holds.

A blob of a single group has no tree — its only subtree is the root — so it is checked whole. So is one whose `imeta` carries no root at all: those are fetched only from a peer holding the entire file, checked once against the SHA-256 at the end, and started over when a transfer drops.

Each exchange then travels as the two BLOSSOM verbs of [the additions](#the-additions).

When an event references a blob, we save a record to the `blob` table which maps the sha256 to the blob's metadata - including everything in the `imeta` tag, as well as the id of the event the blob was first referred to (by `seen_at`, not `created_at`), and whether the blob is a `preview` or an `original`. A tag is a `preview` when it names the original it stands in for ([`nips/imeta-preview.md`](./nips/imeta-preview.md)); anything else is an `original`, so a marker that names no blob buys nothing the roles below hand out. Blobs inherit the permissions of this event.

That anchor is also the blob's lifetime: the record goes when the event does, and the bytes go with the record ([`storage.md`](./storage.md#blob-store)). Only the anchoring event is recorded, so a hash a second event also references is dropped along with the first. The second is left with an `imeta` tag pointing at a blob this device no longer holds and will not ask for again, since the want list is built from the table and only a newly stored event writes to it.

### Quotas

Blob quotas are separate from and much tighter than event quotas:

- Per-peer bytes per session and per rolling 24 h.
- A `original` cache ceiling in bytes, evicted LRU. Previews are kept as long as their events are. Serving a blob's bytes to a peer is what marks it used, and is the only read of them the core has.
