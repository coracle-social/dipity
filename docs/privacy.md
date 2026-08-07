# Privacy and threat model

## What a passive radio observer learns

That a device running this app is present, and nothing else. The advertisement carries no identity, no payload, and — in the iOS background case — not even a readable service UUID except to another device scanning for it specifically. See [`discovery.md`](./discovery.md).

bitchat's stable 8-byte peer ID rides in the advertisement, so a passive sniffer tracks every device in range (its whitepaper says so in §3 and §9). Nothing here is readable that way.

## What an active radio attacker learns

An active attacker connects, or advertises our service UUID and waits to be connected to: a phone running the app, or a small board speaking the handshake. The second costs nothing to operate, because our own [identification budget](./discovery.md#making-identification-cheap) dials strangers on its own initiative — a beacon on a café windowsill is dialled by passers-by.

What such an attacker gets:

- **A completed Noise handshake, always.** There is no gate in front of it, and there cannot be: the handshake is what produces the channel the gate is then evaluated on.
- **A Curve25519 static key that is fresh every session** and links nothing to anything. Noise XX discloses it to an unauthenticated peer before any consent has been established, which is why it is not long-term ([`transport.md`](./transport.md#the-static-key-is-generated-per-session)).
- **A padded list of recognition tags**, each a MAC over this session's handshake hash, resolvable only by a peer holding the matching pair secret ([`discovery.md`](./discovery.md#recognition)).
- **Nothing else, outside a discoverable window.** The session stops at SECURED and closes; no pubkey moves in either direction, and nothing the attacker holds will be seen again.
- **A pubkey, inside a discoverable window** — but only after disclosing one of its own, if it dialled ([`discovery.md`](./discovery.md#the-dialler-authenticates-first)), and only until the [disclosure budget](./policy.md#discoverability) for that window is spent.

**No identifier survives a session**, so tracking costs continuous observation or dense sensor coverage rather than a single sighting. That is not untrackability; the residual is in [What we do not defend against](#what-we-do-not-defend-against).

## What a peer who completes a session learns

- Your nostr pubkey.
- That you were physically present at a time and place.
- Whatever the gossip scope serves them.

The consent gate sits before authentication for this reason, so strangers do not get it automatically.

### The auth event is portable evidence

Kind 22242 is signed by the nostr identity and names the channel in its `relay` tag, so a peer that authenticates you keeps a third-party-verifiable statement that your pubkey held that channel's key. Unlike an [authorship proof](./proofs.md#authorship-proofs), it is not designated-verifier: whoever holds one can convince anyone.

This is the second reason the Noise key is per-session. Against a long-term key, one such event — leaked, sold, or seized from any peer you ever authenticated to — converts every past and future sighting of that key into a named person. Against a per-session key it attests to a channel that no longer exists.

## What a machine-in-the-middle can do

Nothing, and the reason is entirely NIP-42. A handshake against a per-session static key authenticates nobody, so SECURED means the channel is encrypted, not that anyone is who they claim. The transport binding closes the attack: an auth event names the key of the party that issued the challenge, so an event signed for a middle's channel does not verify on the far side, and a middle can only ever appear as itself. See [`nip-p2p-auth.md`](./nip-p2p-auth.md#the-check-runs-on-both-sides).

The one place this needs care is **login with device**, where the target does not hold the identity key yet and NIP-42 has nothing to check. A short authentication string compared by eye is the whole defence — see [`keys.md`](./keys.md#login-with-device).

## Replay across peers

Prevented by the transport binding in [`nip-p2p-auth.md`](./nip-p2p-auth.md). Without it, any peer you authenticate to could replay your `AUTH` event to impersonate you to every other peer — an attack that is far cheaper here than with relays, because every participant is a verifier.

## `seen_at` never leaves the device

`seen_at` records when you encountered an event, which is a record of where you were and who you were near. It is local-only by construction: not part of the event, never transmitted, never served to a peer. See [`storage.md`](./storage.md#event-provenance).

Provenance — which peers an event arrived from — is the same class of data and is subject to the same rule ([`storage.md`](./storage.md#event-provenance)). It is more sensitive than `seen_at` alone: `seen_at` says the user was somewhere, provenance says who they were with, and it accumulates into a connectivity graph. Exposing any of it ("discovered near X") is a deliberate product decision with its own consent story, not a free consequence of having the rows.

## Unsigned events as leak protection

Content events carry no signature, so a bug that publishes proximity content to the open network produces nothing a relay will keep or a client will render. This is defence in depth behind invariant I1, which is already enforced by transport configuration. See [`sync.md`](./proofs.md#events-are-not-signed).

## What an authorship proof discloses

The author's signature is transferable evidence, so it never leaves the peer it names. The second hop receives an [authorship proof](./proofs.md#authorship-proofs) instead — designated to that recipient and simulatable by them, so it convinces them and no one else.

What a second-hop recipient learns is therefore bounded. They learn the forwarder holds the author's signature, which implies the two met; they already observe the forwarder has the event, so per event this adds little. Accumulated it does not: a device's arrival log is a record of other people's co-presence as well as its owner's, since every forwarder that hands over an event is one who met that event's author. They cannot carry any of it further — neither the author's authorship nor the encounter.

A proof covers one event and nothing else, so a second-hop recipient learns nothing about anything else the forwarder was given.

One device does hold portable proof that the author wrote the content and handed it over: the peer the signature names. That is unavoidable, since it is the same object that authorises forwarding. It stays one hop from the author and is never transmitted.

## Key custody

Keys are generated on device and held in platform secure storage. There is no bunker, no relay-side account, and no external signer — none of them can produce a signature during a background wake with no network, which is when every signature this app needs is produced ([`keys.md`](./keys.md#key-custody)). Redundancy is therefore the user's responsibility: a second device holding the same key, or a backup file they export themselves. See [`keys.md`](./keys.md#backup).

**The key is [readable while the device is locked](./keys.md#signing-happens-in-the-background)**, because a phone that cannot sign cannot authenticate. The cost is in [What we do not defend against](#what-we-do-not-defend-against) below.

The backup file is the weak point in an otherwise device-bound design. Unencrypted it is a plaintext key in whatever the user's share sheet sent it to; encrypted it is only as strong as a password they chose once and may never type again. Both are better than permanent identity loss, and the UI presents the tradeoff rather than picking silently.

## What we do not defend against

- **Relayed or extended radio links.** I1 rests on Bluetooth range, and range is the adversary's to choose. A directional antenna reaches hundreds of meters; two radios with an internet link between them are a range extender, and to both endpoints the result is indistinguishable from co-presence. There is no distance bounding here, and BLE offers no practical way to add one. I1 holds against honest implementations and against this app's own code paths, which is what prevents accidental global reach, but not against someone who builds the extender.
- **Traffic analysis of payload sizes.** Frames are not padded. A determined observer learns roughly how much is being exchanged and when.
- **Correlating sessions to each other** by radio fingerprint, timing, co-presence pattern, or simply leaving a receiver in one place. Nothing the protocol discloses outlives a session, so this is what tracking costs here.
- **A harvester inside a discoverable window.** Whoever the window admits gets your pubkey, and a burner pubkey defeats any per-identity limit, so there is no blocklist that works. The [disclosure budget](./policy.md#discoverability) caps the yield per window; nothing caps the number of distinct attackers. Narrowing the window is the only real control.
- **An authorised peer leaking.** Anyone entitled to receive your events can do whatever they like with them outside the protocol — screenshot, retype, republish. The two-hop cap bounds what the *protocol* will carry, not what a person will.
- **Media at rest.** Blobs are written to disk unsealed ([`storage.md`](./storage.md#blobs)). The privacy policy states this plainly.
- **A compromised device.** Secure storage protects keys from other apps, not from an attacker who controls the OS.
- **A locked device that has been unlocked since boot.** The identity key is readable to the app from first unlock onward, so the data-protection class is not a barrier to an attacker who can execute code as the app on a seized-but-locked phone. What is left protecting it is the sandbox and whatever exploit getting inside it costs. There is no good recovery: nostr has no revocation, so a stolen identity key stays valid forever and the only remedy is abandoning the pubkey and rebuilding the social graph under a new one. A device passcode and remote wipe are the real mitigations, and both are the platform's rather than ours.

## Things users will assume that are false

The UI has to actively correct these:

- **"My posts only reach people nearby."** False. Events propagate transitively through people who move — that is invariant I4 and the basis of offline gossip. Proximity constrains *connections*, not *information*. What is true, and what the UI should say instead, is that reach is bounded at two hops by I5: your posts reach people you meet, and people they meet. See [`sync.md`](./sync.md#bounded-propagation).
- **"Deleting a post takes it back."** A kind 5 is an ordinary content event, propagating forward from where it is published to the people you meet next. The content already reached hop 2 through a forwarder under no obligation to carry the deletion after it, and nothing routes a deletion along the paths the content took.
- **"Nobody knows I'm here unless I connect."** A device advertising is detectable as *a* device running this app, and connecting is not the user's decision — anything nearby can dial the device and complete a handshake without being asked. What that yields is bounded by the [consent gate](./discovery.md#the-consent-gate), not by the user's intent to connect.
- **"Muting someone hides them."** It hides them here and nowhere else: this device still accepts, stores and relays their events. [Blocking](./sync.md#trust-block-and-mute-do-different-jobs) is what stops that, and purges what is already stored.
- **"People are who they say they are."** There is no mechanism for preventing impersonation. A trust graph, explicit pairing, or forcing generated identities may be used to mitigate this.
