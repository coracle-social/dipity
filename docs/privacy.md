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

`seen_at` records when you encountered an event, which is a record of where you were and who you were near. It is local-only by construction: not part of the signed event, never transmitted, never served to a peer. See [`storage.md`](./storage.md#seen_at).

Provenance — which peer an event arrived from — is the same class of data and is subject to the same rule. It is not surfaced in the UI. Exposing it ("discovered near X") is a deliberate product decision with its own consent story, not a free consequence of having the column.

## Partition signing as leak protection

Proximity events are signed into the `proximity` partition, so an event that escapes to a normal relay does not validate as a nostr event at all. A bug that publishes proximity content to the open network produces garbage rather than a real post.

This is defence in depth behind invariant I1, which is already enforced by transport configuration. See [`identity.md`](./identity.md#partition-signing).

## Key custody

Keys are generated on device and held in platform secure storage. There is no bunker, no relay-side account, and no external signer, so redundancy is the user's responsibility: a second device holding the same key, or a backup file they export themselves. See [`identity.md`](./identity.md#backup).

The backup file is the weak point in an otherwise device-bound design. Unencrypted it is a plaintext key in whatever the user's share sheet sent it to; encrypted it is only as strong as a password they chose once and may never type again. Both are better than permanent identity loss, and the UI presents the tradeoff rather than picking silently.

## What we do not defend against

- **Traffic analysis of payload sizes.** Frames are not padded. A determined observer learns roughly how much is being exchanged and when.
- **Correlating rotating identifiers** by radio fingerprint, timing, or co-presence patterns.
- **An authorised peer leaking.** Anyone entitled to receive your events can do whatever they like with them. Scope limits reach; it does not limit recipients' behaviour.
- **Media at rest.** Blobs are written to disk unsealed, protected by the platform's data-protection class rather than app-layer encryption. The privacy policy states this plainly.
- **A compromised device.** Secure storage protects keys from other apps, not from an attacker who controls the OS.

## Things users will assume that are false

Worth naming because the UI has to actively correct them:

- **"My posts only reach people nearby."** False. Events propagate transitively through people who move — that is invariant I4 and the whole basis of offline gossip. Proximity constrains *connections*, not *information*. The gossip scope is the only control over reach and must be presented as such. See [`sync.md`](./sync.md#transitive-propagation).
- **"Nobody knows I'm here unless I connect."** Mostly true, but a device advertising is detectable as *a* device running this app.
- **"Muting someone hides them."** It does more: it stops this device carrying their events for anyone, and purges what is already stored.
