# Identity

See [`overview.md`](./overview.md).

## Keys

Three keypairs, three jobs. Keeping them distinct is what makes the transport binding in [`nip-p2p-auth.md`](./nip-p2p-auth.md) meaningful.

| Key | Type | Scope | Purpose |
| --- | --- | --- | --- |
| nostr identity | secp256k1 | Long-term, user-facing | Signs events. The identity users see and follow. |
| Noise static | Curve25519 | Long-term, per-install | Authenticates the BLE channel. |
| iroh endpoint | ed25519 | Long-term, per-install | Authenticates the QUIC session. |

The transport keys and the nostr identity are bound at session time by mutual NIP-42 with transport binding. Nothing else binds them — no long-lived mapping is published anywhere, and the binding is scoped to the session.

## Partition signing

Proximity events are signed into the `proximity` partition per [`nip-partition-sig.md`](./nip-partition-sig.md). The partition value is appended to the NIP-01 serialization before hashing, so the event id and signature differ from a default-partition event with identical contents.

This makes invariant I1 cryptographic rather than operational. A proximity event that leaks to a normal relay does not validate as a nostr event at all — it is not a real post that escaped, it is garbage. No convention, tag, or relay policy is relied upon.

### Consequences, accepted deliberately

- **No promotion path.** A proximity post can never be published to the open network. This is the intent, not a limitation to work around. Re-signing into the default partition would change the event id and orphan every reply, reaction, and quote pointing at it.
- **No interoperability.** No existing nostr client can read these events; no relay can store them meaningfully. Nothing in the wider ecosystem is a fallback.
- **No external signers.** NIP-07, NIP-46, and NIP-55 signers compute the event id themselves and cannot be asked for a partitioned signature. This rules them out entirely — see below.
- **Partition support lives in `@welshman/util`** — in `getEventHash` / `verifyEvent` and everything downstream that recomputes an id. It is a library shipped to other people, so the partition parameter defaults to the default partition and is invisible to existing callers.

### Authentication events stay in the default partition

Kind 22242 auth events are **not** partition-signed. Two reasons: the auth NIP is generally useful beyond this app, and the transport binding already prevents a proximity auth event from being replayed anywhere useful — its `relay` tag is `noise://…` or `iroh://…`, which no relay will ever match.

This also means the existing signing path works for auth unchanged.

### The partition identifier

An arbitrary string, serialized like `content`, compared byte-for-byte, with no registry or namespacing. Ours is `proximity`.

The default partition is the *absence* of the seventh serialization element rather than an empty string, so default-partition events serialize exactly as NIP-01 specifies and existing implementations are unaffected. This is what lets `@welshman/util` carry partition support without touching any existing caller.

### The key is not partitioned, only the events

Worth being clear about: partition signing isolates *events*, not identities. The nsec is an ordinary secp256k1 key, so the same pubkey can sign default-partition events from any other nostr client.

This is fine — it is the same person either way — but two consequences follow. Proximity activity and open-network activity under one pubkey are trivially linkable by anyone who sees both. And a user who exports their key can post to the open network as themselves; what they cannot do is move a *proximity event* there, which is the property I1 needs.

## Key custody

Keys are generated in the app and stored in platform secure storage: Keychain on iOS (with an accessibility class that survives backgrounding but not device-unlocked-once), Keystore / EncryptedSharedPreferences on Android.

There are no external signers, by consequence of the partition decision. `@welshman/signer` narrows to the local-key path.

## Login with device

Identity moves between devices over the same proximity stack — BLE or iroh, Noise-encrypted, same session lifecycle. This is the multi-device story, and the first half of the backup story.

Requirements, because this is deliberate key exfiltration:

- **Explicit on both ends.** Initiated by user action on the source and confirmed by user action on the target. Never automatic, never a background capability.
- **Short authentication string.** Both devices display a derived comparison value the user checks by eye before the key moves. Noise XX authenticates the channel to static keys, but the user has no prior knowledge of the target's static key — the SAS is what stops an active attacker posing as the intended device. Same mechanism as Bluetooth numeric comparison or Signal safety numbers.
- **Copy, not move.** The source retains the key; both devices end up holding the same identity. Multi-device is more useful than migration, and a failed "move" that destroys the source key is unrecoverable.
- **Source device unlocked.** The transfer requires the key to be readable from secure storage, which requires the device be unlocked, not merely powered on.

## Backup

Multi-device is the happy path — two devices holding the same key means losing one costs nothing. But most users have one phone, so there is also a file export, following Flotilla's `KeyDownload.svelte`.

A single text file containing prose instructions and the key itself:

- **Optionally password-encrypted.** Unencrypted is `nsecEncode` from `nostr-tools/nip19`; encrypted is `encrypt` from `nostr-tools/nip49`, producing an `ncryptsec`. The user chooses; encryption is opt-in behind a toggle, not a wall.
- **Minimum 12-character password**, stated in the UI, with "write this down" guidance. There is no recovery for a forgotten backup password either.
- **Instructions, not just a key.** The copy explains what a keypair is, why the private half matters, and what to do with the file — following Flotilla's, with one adjustment. Flotilla tells the user to "import into a Nostr Signer app," which is misleading here, because a signer cannot produce partition-signed events. The wording instead says the key restores this app, and separately identifies them on the open network via other clients.
- **Gated flow.** The download must complete before the user can continue past the screen, so nobody skips it by accident.

On device the `<a download>` blob trick is a no-op in native webviews, so the file is written with `@capacitor/filesystem` to `Directory.Cache` and handed to `@capacitor/share`. Android carries a FileProvider entry (`android/app/src/main/res/xml/file_paths.xml`) for the cache directory. Dismissing the share sheet rejects with "Share canceled", which counts as *not downloaded* rather than an error, so the user can retry. Flotilla's `downloadText` in `src/lib/html.ts` is the reference.

## Advertisement identity

The BLE advertisement carries no identity. See [`discovery.md`](./discovery.md) — this is forced by iOS background behaviour rather than chosen.
