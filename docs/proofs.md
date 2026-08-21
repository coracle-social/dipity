# Authorship proofs

Content events carry no signature. How a recipient learns who wrote one anyway, what lets it travel a second hop, and what stops it going further.

## Events are not signed

A content event carries an id and no `sig`. Authorship is established by the channel it arrived on, not by anything travelling with the event.

Three things follow:

- **Reach is capped by construction.** A signed event is self-authenticating, so anyone holding one can convince anyone else and the hop count is whatever the network's topology allows. An unsigned event is worth nothing to a party that cannot be shown authorship some other way, and the only two ways are a session and a designated-verifier proof, neither of which survives a second forwarding.
- **Nothing leaks to the open network.** A bug that publishes proximity content to a public relay produces something the relay rejects and no existing client renders.
- **The author keeps deniability by default.** Signing every event would hand every recipient permanent, transferable attribution. Here the author chooses, per recipient, whether to hand over that evidence at all.

**The id is the only thing an authorization commits to**, so the id has to be the hash of the event. Both registers name an event by id — the session says the peer authored the event with this id, and a recipient signature is over `event_id ‖ recipient_pubkey`. An id that named content it was not the hash of would let one genuine proof authorize anything a forwarder cared to attach to it, so an inbound event whose id is not its own NIP-01 hash is refused before authorization runs.

## Session AUTH carries the first hop

Mutual NIP-42 proves the party on the other end of the Noise channel holds a given key. An event arriving over that channel whose `pubkey` is one the peer authenticated is proof of authorship to that peer that can't be forwarded.

## Recipient signatures

For B to convince anyone, A has to have committed to that specific event in a form B can carry. That commitment is the author's signature over the event id and the recipient:

```
sig_A( m )          m = tagged_hash("dip/authorship-signature", event_id ‖ recipient_pubkey)
```

The pair is hashed rather than signed directly because BIP-340 takes a message of any length and the audited binding takes 32 bytes. A tagged hash binds the same two values, and the alternative is hand-rolling the signing.

### The author's signature stays with the peer it names

This signature must never leave B's device, since it is verifiable by anyone. It names B, and it attributes the event to A permanently and to everyone — the one artifact in the protocol that defeats the author's deniability outright.

Two rules follow. It is never sent to anyone but the peer it names — a second-hop recipient gets a [proof](#authorship-proofs) built from it instead. And only peers who can be trusted not to leak a signature should receive one: the author chooses, per recipient rather than per event.

Where that guarantee stops is in [`privacy.md`](./privacy.md#what-deniability-covers).

## Authorship proofs

To prove A's authorship, B constructs an **authorship proof** instead of sharing the signature: B proves to C, designated to C, that B holds A's signature over this event and B's own pubkey.

The signature is BIP-340 Schnorr `(R, s)` over that message, verifying as

```
s·G = R + e·A          e = tagged_hash("BIP0340/challenge", R.x ‖ A.x ‖ m)
```

The challenge tag is BIP-340's own and not ours to pick: `e` has to be the value the binding computed, or `S` is not the point the signature is over.

`e` and `A` are public, `R` travels with the proof — half a signature, and inert without the other half — and C reconstructs `m` itself from the event id and the pubkeys B authenticated as. So C can compute the point `S = s·G` without knowing `s`. Holding the signature means knowing `s`, the discrete log of `S`. B proves exactly that, OR-composed with knowledge of C's private key:

```
know dlog(S)   OR   know dlog(C_pubkey)
```

This is a Cramer–Damgård–Schoenmakers OR-proof, made non-interactive with Fiat–Shamir. B proves the branch it can and simulates the other. Forwarding one event sends the event and one proof alongside it.

Each property falls out of one part of the construction:

- **C is convinced.** C knows it did not produce the proof, so the first branch must be the real one — B holds a signature by A over this event, naming B.
- **C cannot pass that conviction on.** A transcript proving B's branch is distributed identically to one C could produce from its own private key. D can run the verification and watch it pass, and still learn nothing, because nothing in the transcript rules out C having fabricated it.
- **C cannot forward the event.** Convincing D means producing a proof designated to D, which takes knowledge of `s` or of D's private key. C has neither, and knowledge does not survive a zero-knowledge proof.

## Implementation

The authorship proof arithmetic needs explicit scalars and points, which the `secp256k1` binding deliberately does not expose, so the OR-proof uses `k256` from RustCrypto. Signing and signature verification stay on the audited binding.

### Nonce derivation

Two nonces matter, and reusing either is catastrophic in a different way. **Neither may come from a bare RNG call.**

**The proof nonce.** Inside the OR-proof is a Schnorr proof of knowledge: commitment `t = r·G`, challenge `c`, response `z = r + c·s`. Two proofs of the same statement under the same `r` with different `c` give

```
s = (z₁ − z₂) / (c₁ − c₂)
```

and `s` is the whole capability. This design proves the same statement constantly — B forwards one event to every peer it meets, and to the same peer again in a later session.

Only the combination is fatal. A repeated `r` with a repeated `c` is a byte-identical proof and reveals nothing, and a fresh `r` is safe whatever `c` does. The rule relates the two:

- **Everything that feeds the challenge must also feed `r`.** Derive `r` from the witness, the statement, and every value entering the Fiat–Shamir transcript, optionally mixed with auxiliary randomness. Then `r` cannot hold still while `c` moves — the construction BIP-340 uses for signatures, for the same reason.
- **Nothing the verifier supplies per session may feed `r`.** A verifier able to choose a nonce input can replay it to force a collision while another transcript value moves, and recover `s`. The NIP-42 AUTH challenge is the obvious candidate, and it stays out: it secures that handshake and nothing else.

The transcript holds the statement and the commitments, and nothing in it varies between two sessions with the same peer. Two proofs of the same statement therefore share every challenge input, and a deterministic `r` makes them the same proof. The rules above hold if a session-unique value is ever added.

Leaking `s` exposes nobody's private key. It costs the two-hop bound and the author's deniability for that event: the recipient can forward, and holds transferable evidence.

**The signing nonce.** A BIP-340 signature is `s = k + e·a`. Two signatures over different messages under the same `k` give

```
a = (s₁ − s₂) / (e₁ − e₂)
```

which is the author's identity key, recoverable by anyone holding both signatures — and recipients hold them by design. Signing volume here is high: one signature per event per recipient, so handing 500 events to three peers is 1500 signatures inside a background wake, on a phone whose entropy pool may be thin shortly after boot. BIP-340's deterministic derivation makes reuse impossible across distinct messages, and the messages are distinct, so the rule is simply that signing goes through the audited binding's own nonce derivation and never a hand-rolled one.

A bad proof nonce costs one event. A bad signing nonce costs an identity permanently, since nostr has no revocation ([`privacy.md`](./privacy.md#what-we-do-not-defend-against)).

### Testing has to cover both directions

Soundness and zero-knowledge fail independently, and known-answer vectors only catch the first.

- **Soundness.** A proof that verifies but establishes nothing. Covered by known-answer vectors, plus negative cases: a proof designated to one verifier rejected by another, a proof over the wrong recipient pubkey rejected, both branches simulated rejected.
- **Zero-knowledge.** A proof that verifies every time while leaking the witness. Test the simulator property directly — real and simulated proofs drawn from the same distribution — and assert that no two proofs of the same statement ever share a commitment.

### Hygiene

- **No key access when forwarding.** B proves knowledge of `s`, which B already holds, not B's own private key. Forwarding never reads secure storage, and no part of the proof can be steered into acting as a signing oracle for B's identity. The only signature B produces in an encounter is its own auth event.
- **One key read per batch when signing.** 1500 Keychain or Keystore reads in a background wake is both slow and more exposure than necessary. Read once, sign the batch, zeroize.
- **Reject before computing.** The verifier rejects malformed input — points not on the curve, out-of-range scalars — before any arithmetic runs. secp256k1's prime order and cofactor of 1 rule out small-subgroup attacks, but not malformed input.
- **Constant time.** Scalar multiplications touching `s` or a nonce use constant-time paths. `k256`'s standard operations are; ad-hoc `Scalar` arithmetic assembled by hand may not be.
