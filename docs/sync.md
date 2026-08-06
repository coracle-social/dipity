# Sync

What moves between peers, how the two sides agree on what's missing, and what policy governs it.

## Peers speak the relay wire protocol

Each device is a relay for its peers and a client of its peers, simultaneously, over whichever transport is live. Event sync is `REQ` / `EVENT` / `EOSE` / `CLOSE` / `OK` / `AUTH` / `NEG-*`.

Reusing it removes a protocol from the project rather than adding one: sync policy compiles to filters, reconciliation is NIP-77, authentication is NIP-42, and both sides of the wire are specified rather than invented.

**Both halves live in the core.** A session begins when a peer appears, which is a moment the view does not exist for ([`overview.md`](./overview.md#architecture)). Anything that runs then is core code, and for sync that is everything: driving the peer, serving the peer, and the crypto in between.

### Client half

Driving a peer — issuing `REQ`, running `NEG-*`, consuming `EVENT` — begins at IDENTIFIED and has to complete inside a background wake. So it is core code, and no TypeScript library is on the peer path at all. The cost is NIP-77: `diff` / `pull` / `push` would have given negentropy over any adapter for free. The core takes the algorithm from `coracle-lib` and supplies the rest — a `SyncStorage` implementation over SQLite, and the wire codec — carrying the [GCS filter](#one-shot-first-negentropy-if-the-session-lasts) alongside.

The view does not speak this protocol at all. It reads the same store through a query method built for it, which can filter on seen time and peer — things the relay protocol has no way to phrase. See [`storage.md`](./storage.md#the-sqlite-store).

**Peers are unreachable from the view by construction.** The bridge addresses one store and offers no way to name a peer, so the untrusted side of the protocol is out of reach of the layer that has no verification, no `AUTH` and no policy.

### Relay half

Serving a peer: parse inbound `REQ`, match filters, stream `EVENT` then `EOSE`, honour `CLOSE`, issue `AUTH` challenges, validate kind 22242, check an [authorship proof](./proofs.md) on every inbound event and attach the right one on every outbound event, and enforce policy on both ingest and egress.

This is core code, and it answers from SQLite, which is the only store that exists when the query arrives. One backend, always: backgrounded there is no working set to answer from. The view finds out what arrived the same way it finds out anything else, through its live subscription against the store ([`storage.md`](./storage.md#the-sqlite-store)).

### Authentication

Mutual NIP-42 with transport binding — see [`nip-p2p-auth.md`](./nip-p2p-auth.md). Both directions run independently; neither blocks the other. The session identifier is `noise://<hex static key>`, naming the key the BLE handshake already authenticated.

Kind 22242 auth events are real nostr events, signed normally, because NIP-42 requires a verifiable signature and the peer checks it as one. They are the exception to "content events are unsigned". Transport binding keeps them from being replayed anywhere useful — the `relay` tag is `noise://…`, which no relay will ever match.

This exchange is also the only thing binding the nostr identity to the Noise static key ([`transport.md`](./transport.md#channel-security)). No long-lived mapping is published anywhere, and the binding is scoped to the session.

The core signs auth events, reading the identity key from platform secure storage, which is why that key has to be [readable while the device is locked](./keys.md#signing-happens-in-the-background). A peer that cannot authenticate can neither send nor receive.

## Reconciliation

### `seen_at` changes what "recent" means

What the app surfaces is *recently discovered* — judged by when **we** first saw an event, not when it was created. An event authored long ago and discovered five minutes ago is new to the user.

This has a sharp consequence for sync: **the reconciliation window cannot be a `created_at` recency window.** Filtering `created_at > now - 7d` would mean never receiving old events, and old events are precisely what proximity gossip surfaces — you meet someone carrying an archive.

But `seen_at` is local and private (see [`privacy.md`](./privacy.md)), so it cannot define the *shared* scope either. The peer has no idea what you have seen or when. The scope both sides can name has to be computable from the event data itself: authors, kinds, and `created_at` range.

So the two clocks do different jobs, and neither substitutes for the other:

| | Defined by | Used for |
| --- | --- | --- |
| `created_at` | The event itself | The shared reconciliation scope. Both peers can compute it. |
| `seen_at` | Local storage | What this device has already seen. Never transmitted. |

### One shot first, negentropy if the session lasts

**On connect: one-shot GCS filter.** Send a compact probabilistic sketch of what we hold in the scope; the peer replies with what we appear to be missing. False positives cost a missed event this round, which the next encounter fixes.

The argument for one-shot is **session duration, not latency**. BLE round trips are 50–150 ms, so a few negentropy rounds would be tolerable in principle, but someone walks past you for eight seconds. A single round trip completes and captures most of the value; a multi-round negotiation may never converge before the link drops.

Sizing, using bitchat's `GCSFilter` as the reference implementation:

- Cost is roughly `p + 2` bits per element. At `p = 10` (≈1/1024 false positive rate) that is 1.5 bytes per event.
- bitchat budgets 400 bytes, but **that is a mesh-flooding constraint, not a BLE constraint** — they broadcast filters through a multi-hop network where every relay pays. We are point-to-point over an established link.
- Budget 4–8 KB. At 10 KB/s that costs under a second and covers 2,700–5,400 events.

**If the link holds: NIP-77 negentropy.** Range-based reconciliation, multiple round trips, converging precisely over the full scope. At 50–150 ms per round trip this is affordable whenever the encounter is measured in minutes rather than seconds — two people in the same room, or two phones in the same building for an afternoon. The core implements it ([Client half](#client-half)), so carrying both modes costs an implementation as well as the decision of when to switch.

Run both: one-shot GCS immediately on connect for the drive-by case, then negentropy for as long as the session survives.

### Walking backward

Because the scope is not a recency window, it can be far larger than one filter covers. Reconcile newest-first and keep a **per-peer watermark** — the `created_at` down to which this peer has been reconciled.

- The first exchange covers the most recent range, so a brief encounter still yields the freshest events.
- Subsequent encounters with the same peer extend the watermark backward.
- A long session jumps straight to negentropy over the whole scope.

The cursor mechanism follows bitchat's: `buildFilter` returns `includedCount`, trimming drops from the input tail, so the first *n* inputs are exactly what the filter covers and the sender derives its `since` cursor from that. Without it the peer cannot distinguish "you are missing this" from "this is outside the range your filter describes," and floods you with everything old.

## Sync policy

Policy is expressed as filters on both ingest and egress, so it compiles onto the wire protocol rather than being enforced by a separate layer.

### Scope, not booleans

Both directions are a scope. On/off means either relaying your whole database to a stranger standing nearby, or relaying nothing.

| Setting | Values |
| --- | --- |
| **Accept** — what we ingest from a peer | Nothing · Peer's own events · Peer + peer's follows · Within social distance *N* of me · Everything |
| **Gossip** — what we serve to a peer | My events only · My follows · Within social distance *N* of the peer · Everything |

Default *N* = 2.

**Two different distances, and they are easy to confuse.** *Social distance* is follow-graph distance — you, your follows, their follows. *Hops* means delivery distance under I5, capped at two. They are independent: an event from someone at social distance 1 still cannot travel more than two hops. Manyverse calls the follow-graph one `hops`, which is where the collision comes from; this document reserves that word for delivery.

"Peer's own events" compiles to `{authors: [peerPubkey]}`. Everything else compiles to an author set derived from the social graph.

**UX consequence:** at the narrow end you receive a peer's reply without the note it replies to. Threads will have holes. That is inherent to author-scoped sync, and the UI must render it gracefully rather than pretend the parent is loading.

### Mute

**One concept, kind 10000.** Mute and block are conflated — a muted pubkey is one the user wants nothing to do with, and it acts everywhere:

- **Ingest** — their events are dropped at receipt, never stored.
- **Egress** — their events are never served onward, regardless of scope.
- **Transitive gossip** — we do not carry their events for anybody. A muted author's events do not propagate through this device.
- **Peering** — their sessions are refused. Because identity is only known after auth, this happens post-IDENTIFIED and the session closes immediately.

This costs a little collective moderation: one user's mute slightly degrades what they relay to others. The alternative is a phone that physically carries a harasser's posts into new rooms.

**Muting purges.** Already-stored events by that author are deleted, or egress silently keeps serving them.

### Quotas

Accepting gossiped events is an unbounded write from whoever is standing nearby. Independent of scope:

- Per-peer event-count and byte budgets per session and per rolling 24 h.
- Per-event size cap.
- A separate, smaller budget for peers outside the web of trust, with a hard ceiling that cannot crowd out known peers. bitchat's courier trust tiers are the pattern.
- Blob quotas are separate and much tighter — see [`media.md`](./media.md).

## Bounded propagation

Information flows further than connections do. Alice syncs with Bob; Alice later walks past Carol; Alice's events reach Carol though Alice and Carol were never co-present at the same moment as Bob. That is invariant I4, and it is what makes offline gossip work.

Invariant I5 caps how far. An event reaches the people its author meets, and the people they meet — two hops, enforced by [authorship proofs](./proofs.md) rather than by every device applying policy correctly.

Reach is therefore a property of the author's social topology, not of how long the app has run or how many devices a post has passed through. Two consequences for the UI:

- **"You only ever connect to people nearby" is true; "your posts only reach people nearby" is false.** A second-hop recipient may be anywhere the first-hop recipient has since travelled. Reach is bounded and describable: your posts reach people you meet, and people they meet.
- **Gossip scope narrows reach within that bound and cannot extend it.** Scope filters authors ([Scope, not booleans](#scope-not-booleans)); the two-hop cap is structural and applies whatever the scope says.
