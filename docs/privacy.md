# Privacy and threat model

## What a passive radio observer learns

That a device running this app is present, and nothing else. The advertisement carries no identity, no payload, and — in the iOS background case — not even a readable service UUID except to another device scanning for it specifically. See [`discovery.md`](./discovery.md).

bitchat's stable 8-byte peer ID rides in the advertisement. A passive sniffer tracks every device in range by it (its whitepaper says so in §3 and §9). Nothing here is readable that way.

## What an active radio attacker learns

An active attacker connects, or advertises our service UUID and waits to be connected to: a phone running the app, or a small board speaking the handshake. The second costs nothing to operate, because our own [identification budget](./discovery.md#making-identification-cheap) dials strangers on its own initiative — a beacon on a café windowsill is dialed by passers-by.

What such an attacker gets:

- **A completed Noise handshake, always.** There is no gate in front of it, and there cannot be: the handshake is what produces the channel the gate is then evaluated on.
- **A Curve25519 static key that is fresh every session** and links nothing to anything. Noise XX discloses it to an unauthenticated peer before any consent has been established, which is why it is not long-term ([`transport.md`](./transport.md#the-static-key-is-generated-per-session)).
- **A padded list of recognition tags**, each a MAC over this session's handshake hash, resolvable only by a peer holding the matching pair secret ([`discovery.md`](./discovery.md#recognition)).
- **Nothing else, in a quiet time, or with the app closed while the disclosure bucket is empty or background discovery is off.** The session stops at SECURED and closes; no pubkey moves in either direction, and nothing the attacker holds will be seen again.
- **A pubkey, whenever the app is open, and otherwise while the [disclosure bucket](./policy.md#discoverability) has room** — but only after disclosing one of its own, if it dialed ([`discovery.md`](./discovery.md#the-consent-gate)).

**No identifier survives a session.** Tracking therefore costs continuous observation or dense sensor coverage rather than a single sighting. That is not untrackability; the residual is in [What we do not defend against](#what-we-do-not-defend-against).

## What a peer who completes a session learns

- Your nostr pubkey.
- That you were physically present at a time and place.
- Whatever [Sharing](./policy.md#sharing) and [relaying](./policy.md#relaying) serve them.

The consent gate sits before authentication for this reason, so that strangers do not get it automatically.

### The auth event is portable evidence

Kind 22242 is signed by the nostr identity and names the channel in its `relay` tag. A peer that authenticates you therefore keeps a third-party-verifiable statement that your pubkey held that channel's key. Unlike an [authorship proof](./proofs.md#authorship-proofs), it is not designated-verifier: whoever holds one can convince anyone.

This is the second reason the Noise key is per-session. Against a long-term key, one such event — leaked, sold, or seized from any peer you ever authenticated to — converts every past and future sighting of that key into a named person. Against a per-session key it attests to a channel that no longer exists.

## Unsigned events as leak protection

A bug that publishes proximity content to the open network produces nothing a relay will keep or a client will render, because content events carry no signature. This is defense in depth behind proximity, which is already enforced by transport configuration. See [`proofs.md`](./proofs.md#events-are-not-signed).

## What deniability covers

An [authorship proof](./proofs.md#authorship-proofs) is designated-verifier: it convinces the peer it was made for and is worthless to anyone else. A second-hop recipient knows where an event came from and cannot prove it. Stated exactly: **a second-hop recipient, acting alone on what the protocol handed it, cannot attribute the event to its author.**

A first-hop recipient does however hold the author's raw signature over the event id and their own pubkey. That signature is transferable evidence, and nothing stops them publishing it, or minting proofs designated to whoever asks. Deniability protects you from the stranger two hops out, not from the person you chose to talk to.

## Key custody

Keys are generated on device and held in platform secure storage. There is no bunker, no relay-side account, and no external signer — none of them can produce a signature during a background wake with no network, which is when every signature this app needs is produced ([`keys.md`](./keys.md#key-custody)). Redundancy is therefore the user's responsibility: a second device holding the same key, or a backup file they export themselves. See [`keys.md`](./keys.md#backup).

**The key is [readable while the device is locked](./keys.md#signing-happens-in-the-background)**, because a phone that cannot sign cannot authenticate. The cost is in [What we do not defend against](#what-we-do-not-defend-against) below.

The backup file is the weak point in an otherwise device-bound design. Unencrypted it is a plaintext key in whatever the user's share sheet sent it to; encrypted it is only as strong as a password they chose once and may never type again. Both are better than permanent identity loss, and the UI presents the tradeoff rather than picking silently.

## Social layer

People can set up their profile any way they like, allowing them to impersonate others. There are two categories of mitigation to this: approve-on-first connect prevents peering when the user isn't paying attention, keeping people who peer at weird times from getting into the social graph. The other is web-of-trust analysis; if we have access to others' contact cards, we can have more confidence in the reputation of a given key.

## What we do not defend against

- **Relayed or extended radio links.** Proximity rests on Bluetooth range, and range is the adversary's to choose. A directional antenna reaches hundreds of meters; two radios with an internet link between them are a range extender, and to both endpoints the result is indistinguishable from co-presence. There is no distance bounding here, and BLE offers no practical way to add one. Proximity holds against honest implementations and against this app's own code paths, which is what prevents accidental global reach, but not against someone who builds the extender.
- **Traffic analysis of payload sizes.** Frames are not padded. A determined observer learns roughly how much is being exchanged and when.
- **Correlating sessions to each other** by radio fingerprint, timing, co-presence pattern, or simply leaving a receiver in one place. This is what tracking costs here, because nothing the protocol discloses outlives a session.
- **A harvester while the app is open, or while the bucket has room.** Whoever the gate admits gets the user's pubkey, and a burner pubkey defeats any blocklist. With the app open nothing caps the yield, because the user is there to be met. With it closed the [disclosure bucket](./policy.md#discoverability) caps the yield across every attacker together. An attacker can empty it, which stops the user meeting strangers until it refills but yields no more sightings than the cap.
- **An authorized peer leaking.** Anyone entitled to receive your events can do whatever they like with them outside the protocol — screenshot, retype, republish. A first-hop recipient can go further and leak the author's signature, which attributes the event to you permanently and to everyone; see [What deniability covers](#what-deniability-covers). The two-hop cap bounds what the *protocol* will carry, not what a person will.
- **Media at rest.** Blobs are written to disk unsealed ([`storage.md`](./storage.md#blob-store)). The privacy policy states this plainly.
- **A compromised device.** Secure storage protects keys from other apps, not from an attacker who controls the OS. A seized device yields the co-presence record that nothing on the radio discloses, because `event_seen` and `event_shared` name who handed the user each thing and who the user handed it to.
- **A locked device that has been unlocked since boot.** The data-protection class is not a barrier to an attacker who can execute code as the app on a seized-but-locked phone, because the identity key is readable to the app from first unlock onward. What is left protecting it is the sandbox and whatever exploit getting inside it costs. There is no good recovery, because nostr has no revocation. A stolen identity key stays valid forever, and the only remedy is abandoning the pubkey and rebuilding the social graph under a new one. A device passcode and remote wipe are the real mitigations, and both are the platform's rather than ours.
