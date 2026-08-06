# Privacy and threat model

## What a passive radio observer learns

That a device running this app is present, and nothing else. The advertisement carries no identity, no payload, and — in the iOS background case — not even a readable service UUID except to another device scanning for it specifically. See [`discovery.md`](./discovery.md).

This is a stronger position than bitchat's, whose stable 8-byte peer ID makes every device passively trackable by anyone in radio range (its whitepaper says so in §3 and §9). Identities here are real, long-lived and socially meaningful, so that trade would cost more than it does for pseudonymous chat.

## What a peer who completes a session learns

- Your nostr pubkey.
- That you were physically present at a time and place.
- Whatever the gossip scope serves them.

The consent gate sits before authentication for this reason, so strangers do not get it automatically.

## What a machine-in-the-middle can do

Nothing. Noise XX authenticates the BLE channel to static keys, and mutual NIP-42 binds the nostr identity to that channel.

The one place this needs care is **login with device**, where the user has no prior knowledge of the target device's static key. That flow requires a short authentication string compared by eye — see [`keys.md`](./keys.md#login-with-device).

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

- **Traffic analysis of payload sizes.** Frames are not padded. A determined observer learns roughly how much is being exchanged and when.
- **Correlating rotating identifiers** by radio fingerprint, timing, or co-presence patterns.
- **An authorised peer leaking.** Anyone entitled to receive your events can do whatever they like with them outside the protocol — screenshot, retype, republish. The two-hop cap bounds what the *protocol* will carry, not what a person will.
- **Media at rest.** Blobs are written to disk unsealed ([`storage.md`](./storage.md#blobs)). The privacy policy states this plainly.
- **A compromised device.** Secure storage protects keys from other apps, not from an attacker who controls the OS.
- **A locked device that has been unlocked since boot.** The identity key is readable to the app from first unlock onward, so the data-protection class is not a barrier to an attacker who can execute code as the app on a seized-but-locked phone. What is left protecting it is the sandbox and whatever exploit getting inside it costs. There is no good recovery: nostr has no revocation, so a stolen identity key stays valid forever and the only remedy is abandoning the pubkey and rebuilding the social graph under a new one. A device passcode and remote wipe are the real mitigations, and both are the platform's rather than ours.

## Things users will assume that are false

The UI has to actively correct these:

- **"My posts only reach people nearby."** False. Events propagate transitively through people who move — that is invariant I4 and the basis of offline gossip. Proximity constrains *connections*, not *information*. What is true, and what the UI should say instead, is that reach is bounded at two hops by I5: your posts reach people you meet, and people they meet. See [`sync.md`](./sync.md#bounded-propagation).
- **"Nobody knows I'm here unless I connect."** Mostly true, but a device advertising is detectable as *a* device running this app.
- **"Muting someone hides them."** It does more: it stops this device carrying their events for anyone, and purges what is already stored.
- **"People are who they say they are."** There is no mechanism for preventing impersonation. Web of trust, explicit pairing, or forcing generated identities may be used to mitigate this.
