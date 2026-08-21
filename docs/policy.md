# Policy

User and app-defined settings which determine what content gets replicated where.

## Hard limits

Content can never be propagated more than two hops away due to how [authorship proofs](./proofs.md) and [transport](./transport.md) are structured. Data always flows over a local bluetooth connection, and is never valid unless authorship is established — by the authenticated session at the first hop, by a designated-verifier proof at the second. No policy setting extends that.

## Discoverability

Any peer that authenticates learns the user's nostr identity, and on a proximity transport also learns that the user was physically present at a time and place. The [consent gate](./discovery.md#the-consent-gate) decides who gets that far.

Peers that have already been paired are stored against a **pair secret** derived from the authenticated session, and pass silently. They are [recognized](./discovery.md#recognition) without either side disclosing anything durable — not a pubkey, which is unavailable before authentication, and not a Noise static key, which does not survive between sessions.

Otherwise, the user has a few preferences they can set for controlling background connections:

- Cool-off window - when the app is foregrounded, it begins accepting connections. This cool-off period determines how long the app will continue accepting unknown connections after the app is backgrounded. 10 minutes by default.
- Discoverable times - times of day, in the device's local timezone, when the user is willing to be passively discoverable. Empty by default.
- Disclosure budget - the number of new pubkeys the device will disclose to per discoverable window. 10 by default. Bounds what a harvester camped in a busy place collects, without needing to know who anyone is; nothing else can, since a burner pubkey defeats any per-identity limit. See [the consent gate](./discovery.md#the-consent-gate).

An unknown peer is admitted if either of the first two preferences allows it and the budget has not been spent.

The cool-off window carries most of the gate: at its default, checking the app on a bus leaves the device open to that bus for ten minutes.

## Social graph

There are certain classifications that users may wish to use to tag other users:

- Trust - the user trusts this person (kind xxxxx)
- Block - the user has blocked this person (kind xxxxx)
- Mute - the user has muted this person (kind 10000)

Every setting below is expressed on the same tiers, applied either to the peer on the other end of a session or to the author of an event:

- **Trusted** - people the user explicitly trusts.
- **Network** - people the user transitively trusts, two hops out.
- **Lenient** - anyone who connects, except blocked pubkeys.
- **Public** - anyone; [proofs are generated](./proofs.md) for whatever the setting covers. Visibility settings only.

## A device, not a pubkey

A peer may prove several pubkeys over one session, since [NIP-42](./nips/p2p-auth.md#mutual-authentication) allows a sequence of exchanges. What the settings below say about it is nonetheless one answer, because the tiers name **people** and a session is with a **device**.

The set reduces to one standing, and the two directions reduce differently:

- **Block is a veto.** Any blocked pubkey blocks the device. Blocking is a decision about a person, and a device holding that key is theirs whatever else it also signs with.
- **Access takes the best.** Any trusted pubkey makes the device trusted. Proving an extra key is a claim to more, never less, and the peer could have made the better claim on its own.

Everything downstream reads that one standing, so a session syncs one superset once rather than once per key. What does *not* reduce is the cryptography: a recipient signature names one recipient and an [authorship proof](./proofs.md#authorship-proofs) designates one verifier, so those are minted once per identity the peer proved.

The same holds in the other direction, for a device carrying several of the user's own identities. Presenting two identities over one session tells the peer they are one device — the linkage happens at authentication, not at serving — so keeping two identities apart from a given peer takes separate sessions.

## Accept and gossip

How much of other people's content the device takes in, and how much of it goes on to the next peer:

| Setting | Governs | Default |
| --- | --- | --- |
| Accept | what the device stores from a peer | `lenient` |
| Gossip | what the device relays onward | `network` |
| Forward | which peers may carry the user's own events one more hop | `trusted` |

Gossip takes one extra value, **Nothing**, which shares only the user's own content. How all three compile onto the wire is in [`sync.md`](./sync.md#how-policy-reaches-the-wire).

### Forwarding

Gossip decides who may *read* an event. Forward decides who may *attribute* it.

A peer carries an event its second hop by presenting an [authorship proof](./proofs.md#authorship-proofs), which it can only build from the author's signature over the event id and its own pubkey. That signature is verifiable by anyone, so handing it over is irreversible: a peer who leaks it [ends the author's deniability](./proofs.md#the-authors-signature-stays-with-the-peer-it-names) for that event permanently and for everyone.

It is therefore its own setting, and a narrow one. At the default a peer outside the user's trust graph is still served the event — it simply stops with them. The cost is reach; the alternative is minting permanent attribution for whoever happens to dial, which on a proximity transport includes a beacon someone left on a windowsill.

## Visibility

Who can see what the user publishes. By default social graph metadata is only shared with `trusted` peers, while everything else a user publishes is `public`.
