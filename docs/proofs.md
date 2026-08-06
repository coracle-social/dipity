# Authorship proofs

Content events carry no signature. What establishes that an author wrote one, and what stops it travelling more than two hops.

## Events are not signed

Content events carry an id and no `sig`. Authenticity comes instead from an **authorship proof**, and this is also what keeps proximity content off the open network ([`privacy.md`](./privacy.md#unsigned-events-as-leak-protection)).

Ids are plain NIP-01 hashes. Nothing about serialization is app-specific, so an id computed here matches what any nostr implementation would compute for the same content, and `@welshman/util` is used unmodified. Two implementations compute them — `@welshman/util` in the view and `coracle-lib` in the core — so they are checked against shared known-answer vectors. JSON string escaping is where they would diverge, and a divergence is silent: reconciliation would report every event as missing in both directions rather than failing.

The arrangement carries two properties:

- **Authenticity.** A proof covers the id, and only the author's key can produce one, so verifying it proves the author produced exactly this content. An event with no valid proof is never stored.
- **Containment.** An event with no `sig` is not a valid nostr event. Proximity content that escapes to a relay is rejected on arrival rather than stored, and no existing client can render it — without relying on any tag, convention, or relay policy.

And three consequences, accepted deliberately:

- **The author can promote their own posts, and only their own.** Holding the key, they can sign one of their events normally and publish it to the open network. Because ids are canonical, a post promoted that way keeps its id and its replies still resolve. Nobody can do this for anyone else's content.
- **No interoperability in the other direction.** Unsigned events cannot be read or stored by relays or existing clients, so nothing in the wider ecosystem is a fallback.
- **The key has to be one the device holds.** Both signed kinds are produced at encounter time, in the background, which rules out every signer the app cannot reach then — see [`keys.md`](./keys.md#key-custody).

Proofs come in two forms, and which one applies is exactly the hop. The author hands over a **direct authorship proof**; anyone forwarding from there produces an **indirect** one.

## Direct authorship proofs

A direct proof is an ordinary signed nostr event:

```jsonc
{
  "kind": 20666,              // ephemeral; never stored by relays
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

Direct proofs and auth events then share one serialization, one signing path and one verification path in the core, and a direct proof is readable with the same tools as any other nostr event. There is no bespoke signature format anywhere in the app.

It does two jobs at once. Its `root` commits to exactly which events it covers, and the author's signature over that root proves the author produced each of them, since ids are hashes of content. Its `p` tag names exactly one recipient, which is what bounds reach — a proof of authorship that only one named peer can hold is also a capability to forward, once.

Direct proofs and auth events are the only signed kinds.

### One signature per chunk

A direct proof covers the whole set of events handed over in one encounter, not a single event — one signature per chunk per recipient. Reconciliation already produces that set; the author builds a Merkle tree over it and signs the root once.

```
leaves    tagged_hash("proof/leaf", event_id), sorted ascending
internal  tagged_hash("proof/node", left ‖ right)
odd node  promoted unchanged to the next level
```

Domain-separating leaves from internal nodes is not optional — without it an internal node can be replayed as a leaf, and the tree stops binding. Sorting makes the tree a deterministic function of the id set, so both sides derive the same root with nothing to negotiate.

The recipient holds every event in the chunk, so it rebuilds the tree itself and needs no inclusion proofs. It stores the direct proof with the sorted id list, which is what lets it produce indirect proofs later — chunk membership is not recoverable from the events themselves.

Handing a peer a thousand events therefore costs one signature and one proof on the wire, not a thousand of each. Signing is local and fast ([`keys.md`](./keys.md#key-custody)), so the binding cost is bytes rather than CPU: a per-event proof would add roughly 200 bytes of tags and signature to every event at 5–15 KB/s, and would have to be produced inside a background wake measured in seconds.

Ingest is one comparison, against the pubkeys the peer has authenticated as under mutual NIP-42 ([`nip-p2p-auth.md`](./nip-p2p-auth.md)):

| The event was authored by | Accept only with | Meaning |
| --- | --- | --- |
| the sender | a **direct proof naming me** whose root covers it | First hop. The author handed it to me. |
| anyone else | an **[indirect proof](#indirect-authorship-proofs) from the sender** covering it | Second hop. The sender holds a direct proof naming them, and proves it without revealing it. |

Anything else is dropped.

**A peer may authenticate as several pubkeys.** NIP-42 permits a sequence of `AUTH` messages, and a shared or multi-account device legitimately holds more than one identity. So both "the sender" and "me" are *sets*, and every check above is set membership rather than equality — "authored by the sender" means the event's pubkey is one the peer authenticated as, and a direct proof satisfies the rule if its `p` tag names any of them. This does not loosen I5: each direct proof still authorises exactly one hop from its author, and holding several identities lets a device sit in several chains rather than extend any of them.

### Why this bounds at two hops

Alice meets Bob and hands over a chunk of her events with one direct proof naming Bob. Bob later meets Carol and forwards some of them, proving he holds that proof without handing it over. Carol accepts. Carol cannot forward them in turn: Dave would need evidence of a direct proof naming Carol, only Alice can mint one, and Alice has never met Carol.

The bound needs no honest-node assumption. A modified client gains nothing by ignoring the rule, because the check runs on the receiver and a direct proof cannot be forged without the author's key.

## Indirect authorship proofs

A direct proof is transferable evidence. Anyone holding one can prove to anyone else that the author signed it, and since the root commits to the events it covers, that is a portable proof of authorship for every one of them. So **a direct proof is only ever sent to the peer it names.** The second hop gets an **indirect proof** instead: evidence that convinces the recipient and nobody else.

B proves to C, designated to C, that B holds a valid direct proof from A naming B and covering this event — without revealing it.

Forwarding one event sends three things:

1. The content event.
2. **An inclusion proof** — the sibling hashes from this event's leaf up to the root, plus the direct proof's `created_at`. C recomputes the root and, with A and B already known, reconstructs the direct proof's event body and therefore its id. C never sees its signature, and learns nothing about the other ids in the chunk beyond their hashes along the path.
3. **A proof of knowledge of that signature**, designated to C — below.

Forwarding several events from one chunk shares almost all of that: a single proof of knowledge covers the direct proof, and the inclusion paths collapse into one multiproof rather than repeating shared internal nodes.

A direct proof's `sig` is a BIP-340 Schnorr signature `(R, s)` over its id, verifying as

```
s·G = R + e·A          e = tagged_hash(R.x ‖ A.x ‖ proof_id)
```

`R`, `e`, and `A` are all public, so C can compute the point `S = s·G` without knowing `s`. Holding the direct proof means knowing `s`, the discrete log of `S`. B proves exactly that, OR-composed with knowledge of C's private key:

```
know dlog(S)   OR   know dlog(C_pubkey)
```

This is a Cramer–Damgård–Schoenmakers OR-proof, made non-interactive with Fiat–Shamir. B proves the branch it can and simulates the other.

Each property falls out of one part of the construction:

- **C is convinced.** C knows it did not produce the proof, so the first branch must be the real one — B holds a signature by A over a direct proof naming B.
- **C cannot transfer it.** The proof is designated to C: verification uses C's pubkey, so D rejects it outright. And C could have produced an identical proof with its own key, so even shown to D it establishes nothing about A.
- **C cannot forward the event.** Convincing D means proving knowledge of a direct proof naming C. None exists, and forging one needs A's key.
- **The direct proof never leaves B.** Transferable evidence of A's authorship exists only on the device A handed the content to.

**Replay binding.** The Fiat–Shamir transcript includes both session identifiers from [`nip-p2p-auth.md`](./nip-p2p-auth.md). Designation already stops C relaying a proof to D, so this is the narrower protection: it stops a proof captured from one session being replayed to the same peer in a later one.

**Cost.** Roughly 200 bytes and a handful of secp256k1 scalar multiplications — no pairings, no trusted setup. One proof per event forwarded, computed at encounter time, inside a background wake.

**Implementation.** The arithmetic needs explicit scalars and points, which the `secp256k1` binding deliberately does not expose, so it uses `k256` from RustCrypto while signing and verification stay on the audited binding. A mistake here is *silent*: a broken OR-proof still produces bytes that verify and prove nothing. It wants known-answer tests and a verifier that rejects malformed input before doing any arithmetic, and it is the construction that most wants a single implementation rather than one per platform — see [`overview.md`](./overview.md#architecture).

**No key access.** B proves knowledge of `s`, which is data B already holds, not B's own private key. Forwarding therefore never reads secure storage, and no part of the proof can be steered into acting as a signing oracle for B's identity. The only signature B produces in an encounter is its own auth event.

## Authorship is detached from the id

Binding the recipient into the event id, by hashing it in, enforces the same two-hop bound. It fails because it gives one logical post a different id per recipient:

- **Reconciliation would not converge.** Both the GCS filter and NIP-77 negentropy diff sets of event ids. Two peers holding the same post would share no ids, so every exchange would report everything as missing in both directions and re-send it.
- **Threading would fragment.** A reply names a parent id. Recipients holding different ids for that parent cannot resolve it.
- **Dedup would fail.** Receiving a post from its author and again from a forwarder would produce two entries, churning the recently-discovered view that `seen_at` exists to keep stable.
