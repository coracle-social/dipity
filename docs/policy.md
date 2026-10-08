# Policy

User and app-defined settings which determine what content gets replicated where.

## Hard limits

Content can never be propagated more than two hops away due to how [authorship proofs](./proofs.md) and [transport](./transport.md) are structured. Data always flows over a local bluetooth connection, and is never valid unless authorship is established — by the authenticated session at the first hop, by a designated-verifier proof at the second. No policy setting extends that.

## Discoverability

Any peer that authenticates learns the user's nostr identity, and on a proximity transport also learns that the user was physically present at a time and place. The [consent gate](./discovery.md#the-consent-gate) decides who gets that far.

Peers that have already been paired are stored against a **pair secret** derived from the authenticated session, and pass silently. They are [recognized](./discovery.md#recognition) without either side disclosing anything durable — not a pubkey, which is unavailable before authentication, and not a Noise static key, which does not survive between sessions.

Otherwise, a stranger is admitted without a prompt in one of two ways. While the app is open, every stranger is: a user looking at the screen is there to meet whoever is nearby, and being sighted while holding the phone up is what they came for. While it is not, a stranger is admitted while the **disclosure bucket** has room, if the user allows discovery in the background at all, which `policy.discover_in_background` says and which is on by default. Being found does not depend on having opened the app recently, because gossip in the background is what the app is for.

Each disclosure gives a stranger the user's pubkey at a place and a time. The rate of disclosures sets what tracking the user costs, because nothing else the protocol discloses outlives a session. Every stranger met in the background draws on the one bucket, because a burner pubkey defeats any limit per identity. The bucket holds three disclosures and refills at 12 a day, fixed rather than a preference. An observer gets at most a burst of three and then one sighting every two hours on average. It limits how often the user can be sighted in their pocket. It does not make them untrackable, and with the app open it does not apply at all.

A disclosure in the background is spent when the device sends its `AUTH` response, not when the exchange completes. The dialer identifies first and does not learn the peer's pubkey until afterwards. A harvester that collects an auth event and walks away therefore costs a disclosure all the same. Three peers cost nothing: one the user has paired with, which is recognized before anyone discloses; one the user approves by hand; and a dialer who is a [contact](#social-graph), whose pubkey the receiver has seen before it answers. [Connection scheduling](./discovery.md#connection-scheduling) keeps a harvester from draining the bucket by being redialed.

When the bucket is empty, a stranger is held for the user to approve. **Quiet times** are times of day, in the device's local timezone, when every stranger is held that way. There are none by default.

## Social graph

There are certain classifications that users may wish to use to tag other users:

- Block - the user has blocked this person (kind 16018)
- Mute - the user has muted this person (kind 10000)

Each is a replaceable event whose `p` tags name people. The current list is one lookup at `<kind>:<pubkey>:`, and an edit supersedes what came before. The block kind is ours rather than NIP-51's.

The mute list names **topics** as well as people, as NIP-51's `t` tags. A post carries at most one topic, and muting one keeps its posts off the board and out of the new-activity notification, and does nothing else. The device still stores them and still hands them on. A muted topic is as visible to contacts as a muted person, because [Sharing](#sharing) hands them the mute list.

**Neither is encrypted.** NIP-51 keeps private entries as ciphertext in `content`, which would put them beyond the peers who need them, since contacts read these lists by design. [Sharing](#sharing) fixes which peers this device hands them to.

A third kind says what the user calls somebody: a contact card, kind 36017, addressed to the person it names at `36017:<author>:<subject>`. A card is the only way anybody has a name, because nobody publishes a profile here. The name is the card's content.

There is one card per contact rather than one list naming everybody the user has met, so that a name travels with that contact's own events.

A **contact** is somebody the user paired with and named: the subject of one of the user's own contact cards whose name is not empty, unless they are blocked. Pairing, which compares shapes in person and names the other person, is the explicit act that makes somebody a contact. Forgetting somebody ends it by emptying the user's card for them.

Every setting below is expressed on the same tiers, applied either to the peer on the other end of a session or to the author of an event:

- **Nothing** - nobody.
- **Contacts** - people the user paired with and named.
- **Network** - contacts, and the people the user's contacts have named.
- **Lenient** - anyone who connects, except blocked pubkeys.
- **Public** - anyone.

Only the fixed visibility rules use Nothing and Public.

Network adds the subjects of every non-empty contact card authored by a contact, except blocked pubkeys, and stops there, two hops from the user. It is derived rather than stored, from whichever of those cards the device holds. A contact whose cards have not arrived yet contributes nobody.

## A device, not a pubkey

A peer may prove several pubkeys over one session, since [NIP-42](./nips/p2p-auth.md#mutual-authentication) allows a sequence of exchanges. What the settings below say about it is nonetheless one answer, because the tiers name **people** and a session is with a **device**.

The set reduces to one standing, and the two directions reduce differently:

- **Block is a veto.** Any blocked pubkey blocks the device. Blocking is a decision about a person, and a device holding that key is theirs whatever else it also signs with.
- **Access takes the best.** Any contact's pubkey makes the device a contact. Proving an extra key is a claim to more, never less, and the peer could have made the better claim on its own.

A session syncs one superset once rather than once per key, because everything downstream reads that one standing. What does *not* reduce is the cryptography: recipient signatures and [authorship proofs](./proofs.md#authorship-proofs) are minted once per identity the peer proved, because a signature names one recipient and a proof designates one verifier.

The same holds in the other direction, for a device carrying several of the user's own identities. Presenting two identities over one session tells the peer they are one device — the linkage happens at authentication, not at serving — so keeping two identities apart from a given peer takes separate sessions.

## Accept

What the device stores from a peer, measured against each event's author: `policy.accept`, `lenient` by default. It takes Contacts, Network or Lenient, and the user's own events are always accepted. A stored `trusted` reads as `contacts`.

## Sharing

Who is handed the user's own activity, and who may carry it a hop further: `policy.sharing`, shown as "Who can see your activity".

| Setting | Handed the user's events | Handed the signature | Reaches |
| --- | --- | --- | --- |
| `contacts` | contacts | nobody | contacts |
| `network` | contacts | contacts | contacts, and their contacts |
| `anyone` (default) | anyone not blocked | contacts | anyone the user meets, and their contacts' contacts |

A peer carries an event its second hop by presenting an [authorship proof](./proofs.md#authorship-proofs), which it can only build from the author's signature over the event id and its own pubkey. Handing that signature over [ends the author's deniability](./proofs.md#the-authors-signature-stays-with-the-peer-it-names) for that event permanently and for everyone, because anyone can verify it. A signature therefore only ever goes to a [contact](#social-graph), and `anyone` widens who sees an event first-hand rather than who can attribute it. A signature handed to a stranger would mint permanent attribution for whoever happens to dial, which on a proximity transport includes a beacon left on a windowsill.

Three of the user's events follow fixed rules whatever the setting. The block and mute lists go only to contacts. The bookmark list goes to nobody, including the user's own second phone, because it cannot be encrypted on a path with no signer. Contact cards follow the setting.

### Relaying

What the device carries for other people is not a setting. It hands a contact every event it holds for somebody else that it may forward, which is an event it holds the author's signature for, unless the event is in the trash or its author is blocked. A stranger is handed none of it. An event that arrived second-hand, under a proof, is never handed on. A contact therefore passes the user's events to their own contacts and no further, which is what `network` says.

### How the screen maps to the core

| Screen | Preference | Core |
| --- | --- | --- |
| What you accept | `policy.accept`, a scope | `PeerPolicy::should_accept`, at ingest |
| Who can see your activity | `policy.sharing` | `Sharing::audience` for who is handed the user's events, `Sharing::signs` for who gets a signature, and `Visibility::for_sharing` for the fixed rules |
| (none) | relaying | `PeerPolicy::may_share` for somebody else's event, and the same rule in the query answering a peer |

How each compiles onto the wire is in [`sync.md`](./sync.md#how-policy-reaches-the-wire).

## Retention

How long an event carried for someone else stays after it first reaches this device is a preference too: `policy.retention_days`, 90 by default. What the sweep spares, and why it measures arrival rather than age or circulation, is [`storage.md`](./storage.md#retention).
