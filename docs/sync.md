# Sync

What moves between peers, how the two sides agree on what's missing, and what policy governs it. See [`overview.md`](./overview.md).

## Peers speak the relay wire protocol

Each device is a relay for its peers and a client of its peers, simultaneously, over whichever transport is live. Event sync is not a new protocol — it is `REQ` / `EVENT` / `EOSE` / `CLOSE` / `OK` / `AUTH` / `NEG-*`.

Reusing it removes a protocol from the project rather than adding one: sync policy compiles to filters, reconciliation is NIP-77, authentication is NIP-42, and both sides of the wire already have implementations.

### Client half

`@welshman/net` provides the seam. `AbstractAdapter` plus the `getAdapter` context override runs the relay protocol over any transport:

```ts
request({
  relays: ['ble://<peer-id>'],
  filters: [{authors: [peerPubkey]}],
  context: {
    getAdapter: url =>
      url === SQLITE_STORAGE_URL ? new SqliteAdapter()  :
      url.startsWith('ble://')   ? new BleAdapter(url)  :
      url.startsWith('iroh://')  ? new IrohAdapter(url) : undefined,
  },
})
```

The same dispatch covers the durable store: `SQLITE_STORAGE_URL` reaches native SQLite over the Capacitor bridge, so the webview reads its own events through the protocol it uses for peers. See [`storage.md`](./storage.md#sqlite-is-the-source-of-truth-and-it-is-also-a-relay). Peer adapters and the storage adapter differ in trust, not in protocol — peers get verification, `AUTH`, and policy; the store gets none of them.

`diff` / `pull` / `push` give NIP-77 over that adapter unchanged.

### Relay half

`LocalAdapter` / `LOCAL_RELAY_URL` serves this app from its own repository. The server side handles a remote peer: it parses inbound `REQ`, matches filters, streams `EVENT` then `EOSE`, honours `CLOSE`, issues `AUTH` challenges, validates kind 22242, checks a [delivery grant](#delivery-grants) or [grant proof](#grant-proofs) on every inbound event and attaches the right one on every outbound event, and enforces policy on both ingest and egress.

**This is an app module, not an upstream contribution to `@welshman/net`.** The deciding factor is where the events come from. A library implementation would serve a `Repository`, but our authoritative store is native SQLite, and the working set in memory is a deliberate subset of it (see [`storage.md`](./storage.md#the-working-set-stays)). Owning the server side means a `REQ` can be answered from whichever backend suits the request — the working set when the answer is already resident, native SQLite when it is not — instead of from whatever a library API fixed on. Grant verification on ingest is app-local for the same reason ([Delivery grants](#delivery-grants)).

The native side implements the same protocol independently, for background serving — see [`storage.md`](./storage.md).

### Authentication

Mutual NIP-42 with transport binding, specified in [`nip-p2p-auth.md`](./nip-p2p-auth.md). Both directions run independently; neither blocks the other. Session identifiers are `iroh://<hex endpoint id>` and `noise://<hex static key>`.

Auth events are ordinary signed nostr events, unlike content events — see [`identity.md`](./identity.md#events-are-not-signed-grants-are).

## Delivery grants

Content events carry no signature. Authenticity and reach both come from a **delivery grant**, which is itself an ordinary signed nostr event:

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

Making the grant an event rather than a bare signature is deliberate: it is signed by `@welshman/signer` and verified by `@welshman/util`, both unmodified. There is no bespoke signing path anywhere in the app.

A grant does two jobs at once. Its `root` commits to exactly which events it covers, and the author's signature over that root proves the author produced each of them, since ids are hashes of content. Its `p` tag names exactly one recipient, which is what bounds reach.

Grants and auth events are the only signed kinds in the system. Content events are not signed.

### One signature per chunk

A grant covers the whole set of events handed over in one encounter, not a single event. Reconciliation already produces that set; the author builds a Merkle tree over it and signs the root once.

```
leaves    tagged_hash("grant/leaf", event_id), sorted ascending
internal  tagged_hash("grant/node", left ‖ right)
odd node  promoted unchanged to the next level
```

Domain-separating leaves from internal nodes is not optional — without it an internal node can be replayed as a leaf, and the tree stops binding. Sorting makes the tree a deterministic function of the id set, so both sides derive the same root with nothing to negotiate.

The recipient holds every event in the chunk, so it rebuilds the tree itself and needs no inclusion proofs. It stores the grant with the sorted id list, which is what lets it produce proofs later even after retention has evicted some of the events.

This matters most for remote signers. Handing a peer a thousand events costs one signature rather than a thousand, which is the difference between a workable NIP-46 or NIP-55 login and a hopeless one — see [`identity.md`](./identity.md#key-custody).

Ingest is one comparison, against the transport-authenticated peer identity established by mutual NIP-42 ([`nip-p2p-auth.md`](./nip-p2p-auth.md)):

| The event was authored by | Accept only with | Meaning |
| --- | --- | --- |
| the sender | a **grant naming me** whose root covers it | First hop. The author handed it to me. |
| anyone else | a **[grant proof](#grant-proofs) from the sender** covering it | Second hop. The sender holds a grant naming them, and proves it without revealing it. |

Anything else is dropped, including any event arriving with neither.

### Why this bounds at two hops

Alice meets Bob and hands over a chunk of her events with one grant naming Bob. Bob later meets Carol and forwards some of them, proving he holds that grant without handing it over. Carol accepts. Carol cannot forward them in turn: Dave would need proof of a grant naming Carol, only Alice can mint one, and Alice has never met Carol.

The bound needs no honest-node assumption. A modified client gains nothing by ignoring the rule, because the check runs on the receiver and a grant cannot be forged without the author's key.

### Unsigned events are the leak protection

An event with no `sig` is not a valid nostr event. A proximity event that escapes to a relay is rejected on arrival rather than stored, and no existing client can render it. This is what keeps proximity content off the open network, and it is stronger than any tagging convention because it needs no cooperation from the relay.

The one exception is deliberate and belongs to the author: they hold the key, so they can sign one of their own events normally and publish it. Nobody else can, for anyone else's content. Ids are plain NIP-01 hashes, so a post promoted that way keeps its id and its replies still resolve.

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

C knows A wrote the event and can say so. C simply cannot prove it. That asymmetry is the point: the second hop can read and trust, but not attest.

**Replay binding.** The Fiat–Shamir transcript includes both session identifiers from [`nip-p2p-auth.md`](./nip-p2p-auth.md). Designation already stops C relaying a proof to D, so this is the narrower protection: it stops a proof captured from one session being replayed to the same peer in a later one.

**Cost.** Roughly 200 bytes and a handful of secp256k1 scalar multiplications, all available from `@noble/curves` — no pairings, no trusted setup, no new dependency. One proof per event forwarded, computed at encounter time. The delicate part is the OR-proof itself, where a mistake is silent rather than loud, so it wants known-answer tests and a verifier that rejects malformed input before doing any arithmetic.

**No key access.** B proves knowledge of `s`, which is data B already holds — not B's own private key. Grant proofs therefore work unchanged when the user signs through an external signer.

### The grant is detached, not folded into the id

Binding the recipient into the event id — by hashing it in — enforces the same two-hop bound, and was the first shape this took. It fails because it gives one logical post a different id per recipient:

- **Reconciliation would not converge.** Both the GCS filter and NIP-77 negentropy diff sets of event ids. Two peers holding the same post would share no ids, so every exchange would report everything as missing in both directions and re-send it.
- **Threading would fragment.** A reply names a parent id. Recipients holding different ids for that parent cannot resolve it.
- **Dedup would fail.** Receiving a post from its author and again from a forwarder would produce two entries, churning the recently-discovered view that `seen_at` exists to keep stable.

Keeping the binding detached leaves the id canonical, so everything above works unchanged.

### Cost

One signature per (chunk, recipient), produced at encounter time on the author's device, plus one proof of knowledge per chunk forwarded and a multiproof over the events taken from it. Grants are stored with the events they cover and never transmitted; second-hop recipients hold neither a grant nor anything they could forward. See [`storage.md`](./storage.md).

## Reconciliation

### `seen_at` changes what "recent" means

The app's primary view is *recently discovered*, ordered by when **we** first saw an event, not when it was created. An event authored three years ago and discovered five minutes ago is new to the user and belongs at the top.

This has a sharp consequence for sync: **the reconciliation window cannot be a `created_at` recency window.** Filtering `created_at > now - 7d` would mean never receiving old events, and old events are precisely what proximity gossip surfaces — you meet someone carrying an archive.

But `seen_at` is local and private (see [`privacy.md`](./privacy.md)), so it cannot define the *shared* scope either. The peer has no idea what you have seen or when. The scope both sides can name has to be computable from the event data itself: authors, kinds, and `created_at` range.

So the two clocks do different jobs, and neither substitutes for the other:

| | Defined by | Used for |
| --- | --- | --- |
| `created_at` | The event itself | The shared reconciliation scope. Both peers can compute it. |
| `seen_at` | Local storage | Ordering, the "recently discovered" view, retention. Never transmitted. |

### One shot over BLE, negentropy over iroh

**Over BLE: one-shot GCS filter.** Send a compact probabilistic sketch of what we hold in the scope; the peer replies with what we appear to be missing. False positives cost a missed event this round, which the next encounter or an iroh upgrade fixes.

The argument for one-shot is **session duration, not latency**. BLE round trips are 50–150 ms, so a few negentropy rounds would be tolerable in principle — but someone walks past you for eight seconds. A single round trip completes and captures most of the value; a multi-round negotiation may never converge before the link drops.

Sizing, using bitchat's `GCSFilter` as the reference implementation:

- Cost is roughly `p + 2` bits per element. At `p = 10` (≈1/1024 false positive rate) that is 1.5 bytes per event.
- bitchat budgets 400 bytes, but **that is a mesh-flooding constraint, not a BLE constraint** — they broadcast filters through a multi-hop network where every relay pays. We are point-to-point over an established link.
- Budget 4–8 KB. At 10 KB/s that costs under a second and covers 2,700–5,400 events.

**Over iroh: NIP-77 negentropy.** Range-based reconciliation, multiple round trips, converges precisely over the full scope. `diff` / `pull` / `push` already do this.

Run both: one-shot GCS immediately on connect for the drive-by case, then negentropy if the session survives and especially once upgraded. Same event store, two entry points.

### Walking backward

Because the scope is not a recency window, it can be far larger than one filter covers. Reconcile newest-first and keep a **per-peer watermark** — the `created_at` down to which this peer has been reconciled.

- The first exchange covers the most recent range, so a brief encounter still yields the freshest events.
- Subsequent encounters with the same peer extend the watermark backward.
- A long session, or an iroh upgrade, jumps straight to negentropy over the whole scope.

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

The collective-moderation objection is real — one user's mute slightly degrades what they relay to others — but it is small in a network with redundant paths, and the alternative is a phone that physically carries a harasser's posts into new rooms. That is not defensible in a UI.

**Muting purges.** Already-stored events by that author are deleted, or egress silently keeps serving them.

### Quotas

Accepting gossiped events is an unbounded write from whoever is standing nearby. Independent of scope:

- Per-peer event-count and byte budgets per session and per rolling 24 h.
- Per-event size cap.
- A separate, smaller budget for peers outside the web of trust, with a hard ceiling that cannot crowd out known peers. bitchat's courier trust tiers are the pattern.
- Blob quotas are separate and much tighter — see [`media.md`](./media.md).

## Bounded propagation

Information still flows further than connections do. Alice syncs with Bob; Alice later walks past Carol; Alice's events reach Carol though Alice and Carol were never co-present at the same moment as Bob. That is invariant I4, and it is what makes offline gossip work.

Invariant I5 caps how far. An event reaches the people its author meets, and the people they meet — two hops, and no further, enforced by [delivery grants](#delivery-grants) rather than by every device applying policy correctly.

Reach is therefore a property of the author's social topology, not of how long the app has run or how many devices a post has passed through. Two consequences for the UI:

- **"You only ever connect to people nearby" is true. "Your posts only reach people nearby" is still false** — a second-hop recipient may be anywhere the first-hop recipient has since travelled. What is now true, and worth saying plainly, is that reach is bounded and describable: your posts reach people you meet, and people they meet.
- **Gossip scope narrows reach within that bound and cannot extend it.** Scope filters authors ([Scope, not booleans](#scope-not-booleans)); the two-hop cap is structural and applies whatever the scope says.
