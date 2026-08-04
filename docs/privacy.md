# Privacy and threat model

See [`overview.md`](./overview.md).

## What a passive radio observer learns

That a device running this app is present, and nothing else. The advertisement carries no identity, no payload, and — in the iOS background case — not even a readable service UUID except to another device scanning for it specifically. See [`discovery.md`](./discovery.md).

This is a stronger position than bitchat's, whose stable 8-byte peer ID makes every device passively trackable by anyone in radio range (its whitepaper says so in §3 and §9). For an app where identities are real, long-lived and socially meaningful, that trade would be much worse than it is for pseudonymous chat.

## What a peer who completes a session learns

- Your nostr pubkey.
- That you were physically present at a time and place.
- Whatever the gossip scope serves them.

This is the real disclosure in the design, and it is why the consent gate sits before authentication rather than after. Strangers do not get it automatically.

## What a machine-in-the-middle can do

Nothing, on either transport. Noise XX authenticates BLE to static keys, iroh authenticates QUIC to endpoint keys, and mutual NIP-42 binds the nostr identity to whichever channel is in use.

The one place this needs care is **login with device**, where the user has no prior knowledge of the target device's static key. That flow requires a short authentication string compared by eye — see [`identity.md`](./identity.md#login-with-device).

## Replay across peers

Prevented by the transport binding in [`nip-p2p-auth.md`](./nip-p2p-auth.md). Without it, any peer you authenticate to could replay your `AUTH` event to impersonate you to every other peer — an attack that is far cheaper here than with relays, because every participant is a verifier.

## `seen_at` never leaves the device

`seen_at` records when you encountered an event, which is a record of where you were and who you were near. It is local-only by construction: not part of the event, never transmitted, never served to a peer. See [`storage.md`](./storage.md#seen_at).

Provenance — which peer an event arrived from — is the same class of data and is subject to the same rule. It is not surfaced in the UI. Exposing it ("discovered near X") is a deliberate product decision with its own consent story, not a free consequence of having the column.

## Unsigned events as leak protection

Content events carry no signature, so an event that escapes to a normal relay is rejected on arrival rather than stored. A bug that publishes proximity content to the open network produces nothing a relay will keep or a client will render.

This is defence in depth behind invariant I1, which is already enforced by transport configuration. See [`identity.md`](./identity.md#events-are-not-signed-grants-are).

## What a delivery grant discloses

A grant is transferable evidence of authorship, so it never leaves the peer it names. The second hop receives a [grant proof](./sync.md#grant-proofs) instead — designated to that recipient and simulatable by them, so it convinces them and no one else.

What a second-hop recipient learns is therefore bounded. They learn the forwarder holds a grant from the author, which implies the two met; they already observe the forwarder has the event, so this adds little. What they cannot do is carry any of it further — not the author's authorship, and not the encounter.

One detail the chunking adds: a grant covers a whole encounter, and verifying an inclusion proof reveals its root and timestamp. A second-hop recipient can therefore tell that several events reached the forwarder in the same handover, and see the sibling hashes along each path. The other event ids stay hidden, but the grouping does not.

One device does hold portable proof that the author wrote the content and handed it over: the peer the grant names. That is unavoidable, since it is the same object that authorises forwarding. It stays one hop from the author and is never transmitted.

## Key custody

Keys are generated on device and held in platform secure storage. There is no bunker, no relay-side account, and no external signer, so redundancy is the user's responsibility: a second device holding the same key, or a backup file they export themselves. See [`identity.md`](./identity.md#backup).

The backup file is the weak point in an otherwise device-bound design. Unencrypted it is a plaintext key in whatever the user's share sheet sent it to; encrypted it is only as strong as a password they chose once and may never type again. Both are better than permanent identity loss, and the UI presents the tradeoff rather than picking silently.

## What we do not defend against

- **Traffic analysis of payload sizes.** Frames are not padded. A determined observer learns roughly how much is being exchanged and when.
- **Correlating rotating identifiers** by radio fingerprint, timing, or co-presence patterns.
- **An authorised peer leaking.** Anyone entitled to receive your events can do whatever they like with them outside the protocol — screenshot, retype, republish. The two-hop cap bounds what the *protocol* will carry, not what a person will.
- **Media at rest.** Blobs are written to disk unsealed, protected by the platform's data-protection class rather than app-layer encryption. The privacy policy states this plainly.
- **A compromised device.** Secure storage protects keys from other apps, not from an attacker who controls the OS.

## Things users will assume that are false

Worth naming because the UI has to actively correct them:

- **"My posts only reach people nearby."** False. Events propagate transitively through people who move — that is invariant I4 and the basis of offline gossip. Proximity constrains *connections*, not *information*. What is true, and what the UI should say instead, is that reach is bounded at two hops by I5: your posts reach people you meet, and people they meet. See [`sync.md`](./sync.md#bounded-propagation).
- **"Nobody knows I'm here unless I connect."** Mostly true, but a device advertising is detectable as *a* device running this app.
- **"Muting someone hides them."** It does more: it stops this device carrying their events for anyone, and purges what is already stored.
