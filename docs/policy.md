# Policy

User and app-defined settings which determine what content gets replicated where.

## Hard limits

Content can never be propagated more than two hops away due to how [authorship proofs](./proofs.md) and [transport](./transport.md) are structured. Data always flows over a local bluetooth connection, and is never valid unless authorship is established — by the authenticated session at the first hop, by a designated-verifier proof at the second. No policy setting extends that.

## Discoverability

Any peer that authenticates learns the user's nostr identity, and on a proximity transport also learns that the user was physically present at a time and place. The [consent gate](./discovery.md#the-consent-gate) decides who gets that far.

Peers that have already been paired are stored against a **pair secret** derived from the authenticated session, and pass silently. They are [recognized](./discovery.md#recognition) without either side disclosing anything durable — not a pubkey, which is unavailable before authentication, and not a Noise static key, which does not survive between sessions.

Otherwise, a stranger is admitted without a prompt while the **disclosure bucket** has room, whether the app is open or in a pocket. Gossip in the background is what the app is for, so being found does not depend on having opened it recently.

Each disclosure gives a stranger the user's pubkey at a place and a time. Nothing else the protocol discloses outlives a session, so the rate of disclosures sets what tracking the user costs. Every stranger draws on the one bucket, because a burner pubkey defeats any limit per identity. The bucket holds three disclosures and refills at the user's number per day, 12 by default, so an observer gets at most a burst of three and then one sighting every two hours on average. It limits how often the user can be sighted. It does not make them untrackable.

A disclosure is spent when the device sends its `AUTH` response, not when the exchange completes. The dialer identifies first and does not learn the peer's pubkey until afterwards, so a harvester that collects an auth event and walks away costs a disclosure all the same. Three peers cost nothing: one the user has paired with, which is recognized before anyone discloses; one the user approves by hand; and a dialer the user trusts, whose pubkey the receiver has seen before it answers. [Connection scheduling](./discovery.md#connection-scheduling) keeps a harvester from draining the bucket by being redialed.

When the bucket is empty, a stranger is held for the user to approve. **Quiet times** are times of day, in the device's local timezone, when every stranger is held that way. There are none by default.

## Social graph

There are certain classifications that users may wish to use to tag other users:

- Trust - the user trusts this person (kind 16017)
- Block - the user has blocked this person (kind 16018)
- Mute - the user has muted this person (kind 10000)

Each is a replaceable event whose `p` tags name people, so the current list is one lookup at `<kind>:<pubkey>:` and an edit supersedes what came before. **The first two kinds are ours rather than NIP-51's**, because a trust list here is not a curation of people to read — it decides who is handed the author's signature, which is permanent transferable attribution. A generic list editor in another client must not be able to grant that without knowing it has.

**Neither is encrypted.** NIP-51 keeps private entries as ciphertext in `content`, which would put them beyond the peers who need them — trusted peers read these lists by design. [Visibility](#visibility) decides which peers this device hands them to, and [Forward](#forwarding) decides whether those peers can carry them further.

A fourth kind says what the user calls somebody: a contact card, kind 36017, addressed to the person it names at `36017:<author>:<subject>`. Nobody publishes a profile here, so a card is the only way anybody has a name. The name is the card's content.

There is one card per contact rather than one list naming everybody the user has met, so a name travels with that contact's own events, and a [visibility](#visibility) rule matching a single `d` tag withholds one person's card and leaves the rest.

Every setting below is expressed on the same tiers, applied either to the peer on the other end of a session or to the author of an event:

- **Trusted** - people the user explicitly trusts.
- **Network** - people the user transitively trusts, two hops out.
- **Lenient** - anyone who connects, except blocked pubkeys.
- **Public** - anyone; [proofs are generated](./proofs.md) for whatever the setting covers. Visibility settings only.

Network is the union of the trust lists published by everyone in Trusted. It is derived rather than stored, from whichever of those lists the device holds, so a trusted person whose list has not arrived yet contributes nobody.

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

Which peers this device hands the user's own events to. By default the trust, block and mute lists go to `trusted` peers, the bookmark list goes to nobody, and everything else is `lenient`.

Visibility governs the first hop only. A peer holding the author's signature over an event can forward it to anyone its own gossip setting allows, so the signature decides whether an event travels further, and [Forward](#forwarding) decides who receives one. At the defaults a trusted peer receives the trust list with a signature and may pass it on.

Nobody else reads a bookmark list. It cannot be encrypted, because there is no signer on this path, so it is served to no peer, including the user's own second phone.

## Retention

How long an event carried for someone else outlives the last peer to hand it over is a preference too: `policy.retention_days`, 30 by default. What the sweep spares, and why it measures circulation rather than age, is in [`storage.md`](./storage.md#retention).
