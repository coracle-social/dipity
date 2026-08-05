# Sync

What moves between peers, how the two sides agree on what's missing, and what policy governs it.

## Peers speak the relay wire protocol

Each device is a relay for its peers and a client of its peers, simultaneously, over whichever transport is live. Event sync is `REQ` / `EVENT` / `EOSE` / `CLOSE` / `OK` / `AUTH` / `NEG-*`.

Reusing it removes a protocol from the project rather than adding one: sync policy compiles to filters, reconciliation is NIP-77, authentication is NIP-42, and both sides of the wire are specified rather than invented.

**Both halves live in the core.** A session begins when a peer appears, which is a moment the view does not exist for ([`overview.md`](./overview.md#architecture)). Anything that runs then is core code, and for sync that is everything: driving the peer, serving the peer, and the crypto in between.

### Client half

Driving a peer — issuing `REQ`, running `NEG-*`, consuming `EVENT` — begins at IDENTIFIED and has to complete inside a background wake. So it is core code, and `@welshman/net` is not on the peer path at all. The cost is NIP-77: `diff` / `pull` / `push` would have given negentropy over any adapter for free. The core takes the algorithm from `coracle-lib` and supplies the rest — a `SyncStorage` implementation over SQLite, and the wire codec — carrying the [GCS filter](#one-shot-first-negentropy-if-the-session-lasts) alongside.

What the view keeps is the same seam pointed at local storage. `AbstractAdapter` plus the `getAdapter` override still resolves two URLs, and both of them are ours:

```ts
getAdapter: url =>
  url === LOCAL_RELAY_URL    ? new LocalAdapter(repository) :
  url === SQLITE_STORAGE_URL ? new SqliteAdapter()          : undefined
```

`SQLITE_STORAGE_URL` reaches the core's SQLite over the Capacitor bridge, so the view reads its own events through the protocol the core uses for peers — see [`storage.md`](./storage.md#sqlite-is-the-source-of-truth-and-it-is-also-a-relay).

**Peers are absent from that dispatch by construction.** There is no `ble://` adapter in the view and no way to acquire one, so the untrusted side of the protocol is unreachable from the layer that has no verification, no `AUTH` and no policy.

### Relay half

Serving a peer: parse inbound `REQ`, match filters, stream `EVENT` then `EOSE`, honour `CLOSE`, issue `AUTH` challenges, validate kind 22242, check a [delivery grant](#delivery-grants) or [grant proof](#grant-proofs) on every inbound event and attach the right one on every outbound event, and enforce policy on both ingest and egress.

This is core code, and it answers from SQLite, which is the only store that exists when the query arrives. One backend, always: backgrounded there is no working set to answer from. The view finds out what arrived the same way it finds out anything else, through its live subscription against the store ([`storage.md`](./storage.md#writes)).

### Authentication

Mutual NIP-42 with transport binding — see [`nip-p2p-auth.md`](./nip-p2p-auth.md). Both directions run independently; neither blocks the other. The session identifier is `noise://<hex static key>`, naming the key the BLE handshake already authenticated.

Kind 22242 auth events are real nostr events, signed normally, because NIP-42 requires a verifiable signature and the peer checks it as one. They are the exception to "content events are unsigned". Transport binding keeps them from being replayed anywhere useful — the `relay` tag is `noise://…`, which no relay will ever match.

This exchange is also the only thing binding the nostr identity to the Noise static key ([`transport.md`](./transport.md#channel-security)). No long-lived mapping is published anywhere, and the binding is scoped to the session.

The core signs auth events, reading the identity key from platform secure storage, which is why that key has to be [readable while the device is locked](./keys.md#the-key-is-readable-while-the-device-is-locked). A peer that cannot authenticate can neither send nor receive.

## Delivery grants

### Events are not signed, grants are

Content events carry an id and no `sig`. Authenticity comes instead from a **delivery grant**, and this is also what keeps proximity content off the open network ([`privacy.md`](./privacy.md#unsigned-events-as-leak-protection)).

Ids are plain NIP-01 hashes. Nothing about serialization is app-specific, so an id computed here matches what any nostr implementation would compute for the same content, and `@welshman/util` is used unmodified. Two implementations compute them — `@welshman/util` in the view and `coracle-lib` in the core — so they are checked against shared known-answer vectors. JSON string escaping is where they would diverge, and a divergence is silent: reconciliation would report every event as missing in both directions rather than failing.

The arrangement carries two properties:

- **Authenticity.** A grant covers the id, and only the author's key can produce one, so verifying a grant proves the author produced exactly this content. An event with no valid grant is never stored.
- **Containment.** An event with no `sig` is not a valid nostr event. Proximity content that escapes to a relay is rejected on arrival rather than stored, and no existing client can render it — without relying on any tag, convention, or relay policy.

And three consequences, accepted deliberately:

- **The author can promote their own posts, and only their own.** Holding the key, they can sign one of their events normally and publish it to the open network. Because ids are canonical, a post promoted that way keeps its id and its replies still resolve. Nobody can do this for anyone else's content.
- **No interoperability in the other direction.** Unsigned events cannot be read or stored by relays or existing clients, so nothing in the wider ecosystem is a fallback.
- **The key has to be one the device holds.** Both signed kinds are produced at encounter time, in the background, which rules out every signer the app cannot reach then — see [`keys.md`](./keys.md#key-custody).

### The grant

A grant is an ordinary signed nostr event:

```jsonc
{
  "kind": 20666,              // ephemeral; grants are never stored by relays
  "pubkey": "<author>",       // the author of the content events
  "created_at": 1740000000,
  "tags": [
    ["root", "<merkle root, hex>"],   // commits to the chunk being handed over
    ["p", "<recipient pubkey>"]       // the one peer allowed to hold it
  ],
  "content": "",
  "sig": "<author's signature>"
}
```

Grants and auth events then share one serialization, one signing path and one verification path in the core, and a grant is readable with the same tools as any other nostr event. There is no bespoke signature format anywhere in the app.

A grant does two jobs at once. Its `root` commits to exactly which events it covers, and the author's signature over that root proves the author produced each of them, since ids are hashes of content. Its `p` tag names exactly one recipient, which is what bounds reach.

Grants and auth events are the only signed kinds.

### One signature per chunk

A grant covers the whole set of events handed over in one encounter, not a single event — one signature per chunk per recipient. Reconciliation already produces that set; the author builds a Merkle tree over it and signs the root once.

```
leaves    tagged_hash("grant/leaf", event_id), sorted ascending
internal  tagged_hash("grant/node", left ‖ right)
odd node  promoted unchanged to the next level
```

Domain-separating leaves from internal nodes is not optional — without it an internal node can be replayed as a leaf, and the tree stops binding. Sorting makes the tree a deterministic function of the id set, so both sides derive the same root with nothing to negotiate.

The recipient holds every event in the chunk, so it rebuilds the tree itself and needs no inclusion proofs. It stores the grant with the sorted id list, which is what lets it produce proofs later even after retention has evicted some of the events.

Handing a peer a thousand events therefore costs one signature and one grant on the wire, not a thousand of each. Signing is local and fast ([`keys.md`](./keys.md#key-custody)), so the binding cost is bytes rather than CPU: a per-event grant would add roughly 200 bytes of tags and signature to every event at 5–15 KB/s, and would have to be produced inside a background wake measured in seconds.

Ingest is one comparison, against the pubkeys the peer has authenticated as under mutual NIP-42 ([`nip-p2p-auth.md`](./nip-p2p-auth.md)):

| The event was authored by | Accept only with | Meaning |
| --- | --- | --- |
| the sender | a **grant naming me** whose root covers it | First hop. The author handed it to me. |
| anyone else | a **[grant proof](#grant-proofs) from the sender** covering it | Second hop. The sender holds a grant naming them, and proves it without revealing it. |

Anything else is dropped.

**A peer may authenticate as several pubkeys.** NIP-42 permits a sequence of `AUTH` messages, and a shared or multi-account device legitimately holds more than one identity. So both "the sender" and "me" are *sets*, and every check above is set membership rather than equality — "authored by the sender" means the event's pubkey is one the peer authenticated as, and a grant satisfies the rule if its `p` tag names any of them. This does not loosen I5: each grant still authorises exactly one hop from its author, and holding several identities lets a device sit in several chains rather than extend any of them.

### Why this bounds at two hops

Alice meets Bob and hands over a chunk of her events with one grant naming Bob. Bob later meets Carol and forwards some of them, proving he holds that grant without handing it over. Carol accepts. Carol cannot forward them in turn: Dave would need proof of a grant naming Carol, only Alice can mint one, and Alice has never met Carol.

The bound needs no honest-node assumption. A modified client gains nothing by ignoring the rule, because the check runs on the receiver and a grant cannot be forged without the author's key.

### Grant proofs

A grant is transferable evidence. Anyone holding one can prove to anyone else that the author signed it, and since the root commits to the events it covers, that is a portable proof of authorship for every one of them. So **a grant is only ever sent to the peer it names.** The second hop gets a **grant proof** instead: evidence that convinces the recipient and nobody else.

B proves to C, designated to C, that B holds a valid grant from A naming B and covering this event — without revealing the grant.

Forwarding one event sends three things:

1. The content event.
2. **An inclusion proof** — the sibling hashes from this event's leaf up to the root, plus the grant's `created_at`. C recomputes the root and, with A and B already known, reconstructs the grant event body and therefore its id. C never sees the grant's signature, and learns nothing about the other ids in the chunk beyond their hashes along the path.
3. **A proof of knowledge of the grant's signature**, designated to C — below.

Forwarding several events from one chunk shares almost all of that: a single proof of knowledge covers the grant, and the inclusion paths collapse into one multiproof rather than repeating shared internal nodes.

A grant's `sig` is a BIP-340 Schnorr signature `(R, s)` over the grant's id, verifying as

```
s·G = R + e·A          e = tagged_hash(R.x ‖ A.x ‖ grant_id)
```

`R`, `e`, and `A` are all public, so C can compute the point `S = s·G` without knowing `s`. Holding the grant means knowing `s`, the discrete log of `S`. B proves exactly that, OR-composed with knowledge of C's private key:

```
know dlog(S)   OR   know dlog(C_pubkey)
```

This is a Cramer–Damgård–Schoenmakers OR-proof, made non-interactive with Fiat–Shamir. B proves the branch it can and simulates the other.

Each property falls out of one part of the construction:

- **C is convinced.** C knows it did not produce the proof, so the first branch must be the real one — B holds a signature by A over a grant naming B.
- **C cannot transfer it.** The proof is designated to C: verification uses C's pubkey, so D rejects it outright. And C could have produced an identical proof with its own key, so even shown to D it establishes nothing about A.
- **C cannot forward the event.** Convincing D means proving knowledge of a grant naming C. None exists, and forging one needs A's key.
- **The grant never leaves B.** Transferable evidence of A's authorship exists only on the device A handed the content to.

**Replay binding.** The Fiat–Shamir transcript includes both session identifiers from [`nip-p2p-auth.md`](./nip-p2p-auth.md). Designation already stops C relaying a proof to D, so this is the narrower protection: it stops a proof captured from one session being replayed to the same peer in a later one.

**Cost.** Roughly 200 bytes and a handful of secp256k1 scalar multiplications — no pairings, no trusted setup. One proof per event forwarded, computed at encounter time, inside a background wake.

**Implementation.** The arithmetic needs explicit scalars and points, which the `secp256k1` binding deliberately does not expose, so it uses `k256` from RustCrypto while signing and verification stay on the audited binding. A mistake here is *silent*: a broken OR-proof still produces bytes that verify and prove nothing. It wants known-answer tests and a verifier that rejects malformed input before doing any arithmetic, and it is the construction that most wants a single implementation rather than one per platform — see [`overview.md`](./overview.md#architecture).

**No key access.** B proves knowledge of `s`, which is data B already holds, not B's own private key. Forwarding therefore never reads secure storage, and no part of the proof can be steered into acting as a signing oracle for B's identity. The only signature B produces in an encounter is its own auth event.

### The grant is detached from the id

Binding the recipient into the event id, by hashing it in, enforces the same two-hop bound. It fails because it gives one logical post a different id per recipient:

- **Reconciliation would not converge.** Both the GCS filter and NIP-77 negentropy diff sets of event ids. Two peers holding the same post would share no ids, so every exchange would report everything as missing in both directions and re-send it.
- **Threading would fragment.** A reply names a parent id. Recipients holding different ids for that parent cannot resolve it.
- **Dedup would fail.** Receiving a post from its author and again from a forwarder would produce two entries, churning the recently-discovered view that `seen_at` exists to keep stable.


## Reconciliation

### `seen_at` changes what "recent" means

The app's primary view is *recently discovered*, ordered by when **we** first saw an event, not when it was created. An event authored long ago and discovered five minutes ago is new to the user and belongs at the top.

This has a sharp consequence for sync: **the reconciliation window cannot be a `created_at` recency window.** Filtering `created_at > now - 7d` would mean never receiving old events, and old events are precisely what proximity gossip surfaces — you meet someone carrying an archive.

But `seen_at` is local and private (see [`privacy.md`](./privacy.md)), so it cannot define the *shared* scope either. The peer has no idea what you have seen or when. The scope both sides can name has to be computable from the event data itself: authors, kinds, and `created_at` range.

So the two clocks do different jobs, and neither substitutes for the other:

| | Defined by | Used for |
| --- | --- | --- |
| `created_at` | The event itself | The shared reconciliation scope. Both peers can compute it. |
| `seen_at` | Local storage | Ordering, the "recently discovered" view, retention. Never transmitted. |

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

Invariant I5 caps how far. An event reaches the people its author meets, and the people they meet — two hops, enforced by [delivery grants](#delivery-grants) rather than by every device applying policy correctly.

Reach is therefore a property of the author's social topology, not of how long the app has run or how many devices a post has passed through. Two consequences for the UI:

- **"You only ever connect to people nearby" is true; "your posts only reach people nearby" is false.** A second-hop recipient may be anywhere the first-hop recipient has since travelled. Reach is bounded and describable: your posts reach people you meet, and people they meet.
- **Gossip scope narrows reach within that bound and cannot extend it.** Scope filters authors ([Scope, not booleans](#scope-not-booleans)); the two-hop cap is structural and applies whatever the scope says.
