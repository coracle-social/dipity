# Sync

What moves between peers, how the two sides agree on what's missing, and what policy governs it. See [`overview.md`](./overview.md).

## Peers speak the relay wire protocol

Each device is a relay for its peers and a client of its peers, simultaneously, over whichever transport is live. Event sync is not a new protocol — it is `REQ` / `EVENT` / `EOSE` / `CLOSE` / `OK` / `AUTH` / `NEG-*`.

This is the highest-leverage decision in the design. Sync policy compiles to filters, reconciliation is NIP-77, authentication is NIP-42, and both sides of the wire already have implementations.

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

`LocalAdapter` / `LOCAL_RELAY_URL` serves this app from its own repository. The server side handles a remote peer: it parses inbound `REQ`, matches filters, streams `EVENT` then `EOSE`, honours `CLOSE`, issues `AUTH` challenges, validates kind 22242, and enforces policy on both ingest and egress.

**This lives in `@welshman/net`, upstream.** Any nostr app that wants to be a relay to something needs it, and keeping it in the library keeps peers on the same policy, dedup, and tracker pipeline as everything else.

A second implementation exists natively for background serving — see [`storage.md`](./storage.md).

### Authentication

Mutual NIP-42 with transport binding, specified in [`nip-p2p-auth.md`](./nip-p2p-auth.md). Both directions run independently; neither blocks the other. Session identifiers are `iroh://<hex endpoint id>` and `noise://<hex static key>`.

Auth events stay in the default partition — see [`identity.md`](./identity.md).

## Reconciliation

### `seen_at` changes what "recent" means

The app's primary view is *recently discovered*, ordered by when **we** first saw an event, not when it was created. An event authored three years ago and discovered five minutes ago is new to the user and belongs at the top.

This has a sharp consequence for sync: **the reconciliation window cannot be a `created_at` recency window.** Filtering `created_at > now - 7d` would mean never receiving old events, and old events are precisely what proximity gossip surfaces — you meet someone carrying an archive.

But `seen_at` is local and private (see [`privacy.md`](./privacy.md)), so it cannot define the *shared* scope either. The peer has no idea what you have seen or when. The scope both sides can name has to be computable from the event data itself: authors, kinds, and `created_at` range.

So the two clocks do different jobs, and neither substitutes for the other:

| | Defined by | Used for |
| --- | --- | --- |
| `created_at` | The signed event | The shared reconciliation scope. Both peers can compute it. |
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
| **Accept** — what we ingest from a peer | Nothing · Peer's own events · Peer + peer's follows · Within *N* hops of me · Everything |
| **Gossip** — what we serve to a peer | My events only · My follows · Within *N* hops of the peer · Everything |

Default *N* = 2, following Manyverse's `hops` setting.

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

## Transitive propagation

Invariant I4 means information flows further than connections do. Alice syncs with Bob; Alice later walks past Carol; Bob's events reach Carol though Bob and Carol were never co-present.

This is the feature — it is what makes offline gossip work — but it means **I1 constrains connections, not information.** Any user-facing claim must be phrased accordingly: "you only ever connect to people nearby," never "your posts only reach people nearby." The gossip scope is the control users have over reach, and the UI presents it as such.
