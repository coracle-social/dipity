# Authorship proofs

Content events carry no signature. This document describes how to prove authorship to peers, and what stops it travelling more than two hops.

## Events are not signed

Content events carry no `sig` which keeps proximity content off the open network ([`privacy.md`](./privacy.md#unsigned-events-as-leak-protection)) and deniable beyond the range of authorship proofs.

## Session AUTH

Mutual NIP-42 proves the party on the other end of the Noise channel holds a given key. An event received over this channel whose `pubkey` is one the peer authenticated is proof of authorship.

## Authorship proofs

Session authentication cannot be handed onward: B knows A wrote an event because B was there, and there is nothing to show C. For B to convince anyone, A has to have committed to that specific event in a form B can carry. That commitment is the author's signature over the event id and the recipient:

```
sig_A( event_id ‖ recipient_pubkey )
```

Sharing this signature with a peer provides them independent proof of authorship, however it is transferable evidence: anyone holding it can prove to anyone that A signed it. Users should only provide proofs to peers they trust not to leak them. The two-hop limit provided by system assumes that this proof is never forwarded to a third party.

Instead of sharing the signature itself, relayers should construct an **authorship proof** in order to prove the signature without revealing it. In this scenario, B proves to C, designated to C, that B holds A's signature over this event and B's own pubkey, without revealing it.

The signature is BIP-340 Schnorr `(R, s)` over the message `event_id ‖ B_pubkey`, verifying as

```
s·G = R + e·A          e = tagged_hash(R.x ‖ A.x ‖ event_id ‖ B_pubkey)
```

`R`, `e` and `A` are public, and C reconstructs the signed message itself from the event id and the pubkeys B authenticated as. So C can compute the point `S = s·G` without knowing `s`. Holding the signature means knowing `s`, the discrete log of `S`. B proves exactly that, OR-composed with knowledge of C's private key:

```
know dlog(S)   OR   know dlog(C_pubkey)
```

This is a Cramer–Damgård–Schoenmakers OR-proof, made non-interactive with Fiat–Shamir. B proves the branch it can and simulates the other. Forwarding one event sends the event and one proof alongside it.

Each property falls out of one part of the construction:

- **C is convinced.** C knows it did not produce the proof, so the first branch must be the real one — B holds a signature by A over this event, naming B.
- **C cannot transfer it.** The proof is designated to C: verification uses C's pubkey, so D rejects it outright.
- **C cannot forward the event.** Convincing D means proving knowledge of A's signature. C never learned it, and knowledge does not survive a zero-knowledge proof.
- **The signature never leaves B.** Transferable evidence of A's authorship exists only on the device A handed the content to.

**Cost.** Roughly 200 bytes and a handful of secp256k1 scalar multiplications — no pairings, no trusted setup. One proof per event forwarded, computed at encounter time, inside a background wake.

## Implementation

The authorship proof arithmetic needs explicit scalars and points, which the `secp256k1` binding deliberately does not expose, so the OR-proof uses `k256` from RustCrypto. Signing and signature verification stay on the audited binding.

### Nonce derivation

Two nonces matter, and reusing either is catastrophic in a different way. **Neither may come from a bare RNG call.**

**The proof nonce.** Inside the OR-proof is a Schnorr proof of knowledge: commitment `t = r·G`, challenge `c`, response `z = r + c·s`. Two proofs of the same statement under the same `r` with different `c` give

```
s = (z₁ − z₂) / (c₁ − c₂)
```

and `s` is the whole capability. This design proves the same statement constantly — B forwards one event to every peer it meets, and to the same peer again in a later session.

Only the combination is fatal. A repeated `r` with a repeated `c` is a byte-identical proof and reveals nothing, and a fresh `r` is safe whatever `c` does. The rule is a relation between the two rather than a property of either:

- **Everything that feeds the challenge must also feed `r`.** Derive `r` from the witness, the statement, and every value entering the Fiat–Shamir transcript, optionally mixed with auxiliary randomness. Then `r` cannot hold still while `c` moves — the construction BIP-340 uses for signatures, for the same reason.
- **Nothing the verifier supplies per session may feed `r`.** A verifier able to choose a nonce input can replay it to force a collision while another transcript value moves, and recover `s`. The NIP-42 AUTH challenge is the obvious candidate, and it stays out: it secures that handshake and nothing else.

The transcript holds the statement and the commitments, and nothing in it varies between two sessions with the same peer. Two proofs of the same statement therefore share every challenge input, and a deterministic `r` makes them the same proof. The rules above are what keeps that safe if a session-unique value is ever added.

Leaking `s` does not expose anyone's private key. It costs the two-hop bound and the author's deniability for that event: the recipient can forward, and holds transferable evidence.

**The signing nonce.** A BIP-340 signature is `s = k + e·a`. Two signatures over different messages under the same `k` give

```
a = (s₁ − s₂) / (e₁ − e₂)
```

which is the author's identity key, recoverable by anyone holding both signatures — and recipients hold them by design. Signing volume here is high: one signature per event per recipient, so handing 500 events to three peers is 1500 signatures inside a background wake, on a phone whose entropy pool may be thin shortly after boot. BIP-340's deterministic derivation makes reuse impossible across distinct messages, and the messages are distinct, so the rule is simply that signing goes through the audited binding's own nonce derivation and never a hand-rolled one.

The asymmetry is worth holding onto: a bad proof nonce costs one event, a bad signing nonce costs an identity permanently, since nostr has no revocation ([`privacy.md`](./privacy.md#what-we-do-not-defend-against)).

### Testing has to cover both directions

Soundness and zero-knowledge fail independently, and known-answer vectors only catch the first.

- **Soundness.** A proof that verifies but establishes nothing. Covered by known-answer vectors, plus negative cases: a proof designated to one verifier rejected by another, a proof over the wrong recipient pubkey rejected, both branches simulated rejected.
- **Zero-knowledge.** A proof that verifies every time while leaking the witness. Test the simulator property directly — real and simulated proofs drawn from the same distribution — and assert that no two proofs of the same statement ever share a commitment.

### Hygiene

- **No key access when forwarding.** B proves knowledge of `s`, which B already holds, not B's own private key. Forwarding never reads secure storage, and no part of the proof can be steered into acting as a signing oracle for B's identity. The only signature B produces in an encounter is its own auth event.
- **One key read per batch when signing.** 1500 Keychain or Keystore reads in a background wake is both slow and more exposure than necessary. Read once, sign the batch, zeroize.
- **Reject before computing.** The verifier rejects malformed input — points not on the curve, out-of-range scalars — before any arithmetic runs. secp256k1's prime order and cofactor of 1 rule out small-subgroup attacks, but not malformed input.
- **Constant time.** Scalar multiplications touching `s` or a nonce use constant-time paths. `k256`'s standard operations are; ad-hoc `Scalar` arithmetic assembled by hand may not be.
