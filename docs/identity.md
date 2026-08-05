# Identity

## Keys

Two keypairs, two jobs. Keeping them distinct is what makes the transport binding in [`nip-p2p-auth.md`](./nip-p2p-auth.md) meaningful.

| Key | Type | Scope | Purpose |
| --- | --- | --- | --- |
| nostr identity | secp256k1 | Long-term, user-facing | Signs delivery grants and auth events, both ordinary nostr events. The identity users see and follow. |
| Noise static | Curve25519 | Long-term, per-install | Authenticates the BLE channel. |

The transport key and the nostr identity are bound at session time by mutual NIP-42 with transport binding. Nothing else binds them — no long-lived mapping is published anywhere, and the binding is scoped to the session.

Any transport added later follows the same rule: the identity NIP-42 names has to be one that transport's own handshake authenticated, never one taken from an address book. See [Adding a transport later](./transport.md#adding-a-transport-later).

## Events are not signed, grants are

Content events carry an id and no `sig`. Authenticity comes instead from a **delivery grant**: a small signed nostr event by the author, tagging the content event id and one recipient pubkey, specified in [`sync.md`](./sync.md#delivery-grants).

Ids are plain NIP-01 hashes. Nothing about serialization is app-specific, so an id computed here matches what any nostr implementation would compute for the same content, and `@welshman/util` is used unmodified.

Two implementations compute them: `@welshman/util` in the view and `coracle-lib` in the core. They are checked against shared known-answer vectors, because JSON string escaping is where they would diverge and a divergence is silent — reconciliation would report every event as missing in both directions rather than failing.

This arrangement carries two properties:

- **Authenticity.** A grant covers the id, and only the author's key can produce one, so verifying a grant proves the author produced exactly this content. An event with no valid grant is never stored.
- **Containment.** An event with no `sig` is not a valid nostr event. Proximity content that escapes to a relay is rejected on arrival rather than stored, and no existing client can render it — without relying on any tag, convention, or relay policy.

### Consequences, accepted deliberately

- **The author can promote their own posts, and only their own.** Holding the key, they can sign one of their events normally and publish it to the open network. Because ids are canonical, a post promoted that way keeps its id and its replies still resolve. Nobody can do this for anyone else's content.
- **No interoperability in the other direction.** Unsigned events cannot be read or stored by relays or existing clients, so nothing in the wider ecosystem is a fallback.
- **The key has to be one the device holds.** Both signed kinds are produced at encounter time, in the background. That rules out every signer the app cannot reach then — see [Key custody](#key-custody).

### Auth events are ordinary signed events

Kind 22242 auth events are real nostr events, signed normally, because NIP-42 requires a verifiable signature and the peer checks it as one. They are the exception to "content events are unsigned".

Transport binding keeps them from being replayed anywhere useful — the `relay` tag is `noise://…`, which no relay will ever match. See [`nip-p2p-auth.md`](./nip-p2p-auth.md).

### The key is not confined

This isolates *events*, not identities. The nsec is an ordinary secp256k1 key, so the same pubkey can sign and post on the open network from any other nostr client.

That is fine — it is the same person either way — but proximity activity and open-network activity under one pubkey are trivially linkable by anyone who sees both.

## Key custody

**One model: the app holds the key.** Generated on device or imported, kept in platform secure storage, and read by the core when a signature is needed. There is no external signer path. This is forced rather than chosen.

### Signing happens at encounter time, in the background

Exactly two kinds are signed, and both are produced when a peer appears:

| Signed | When | Frequency |
| --- | --- | --- |
| Kind 22242 auth event | Session establishment, both directions | Once per session per direction |
| [Delivery grant](./sync.md#delivery-grants) | Handover | Once per chunk handed to a peer |

Both fall inside a CoreBluetooth background wake, with the view suspended and possibly with no network. A signer that cannot produce a signature *there* does not reduce the app's capability at the edges; it removes the core loop, because a peer that cannot authenticate can neither send nor receive ([`nip-p2p-auth.md`](./nip-p2p-auth.md)).

Everything else is key-free. Content events carry no signature. [Grant proofs](./sync.md#grant-proofs) prove knowledge of a grant's signature — data we already hold — not knowledge of our key. Verification is public.

### Why external signers are out

| | Blocked by |
| --- | --- |
| **NIP-46 bunker** | Every signature is a relay round trip. In a crowd with no signal there is no signature, so no session. The core would also have to own a WebSocket and NIP-44. |
| **NIP-55 signer** | Android-only to begin with, so it could never be the model on iOS. Its ContentResolver API can sign without an Activity once the user has approved us, so a background signature is not strictly impossible — but it is an IPC round trip into another app, per encounter, on one platform, on a path where the failure mode is silent unreachability. |

[Backup](#backup) and [login with device](#login-with-device) are both gated on holding the nsec, so both are always available.

### The key is readable while the device is locked

Background signing means the core reads the key during a CoreBluetooth wake, which happens whether or not the screen is on. An accessibility class that requires the device be *currently* unlocked would mean a phone in a pocket cannot authenticate, cannot receive, and cannot send — leaving the app working only when someone is looking at it, which inverts what it is for.

- **iOS: `kSecAttrAccessibleAfterFirstUnlockThisDeviceOnly`.** Readable from the moment the user first unlocks after boot, including while locked afterwards. `ThisDeviceOnly` keeps it out of iCloud Keychain and encrypted device backups — moving the identity to another device is [an explicit flow](#login-with-device), not an implicit sync.
- **Android: a Keystore-wrapped key with no `setUserAuthenticationRequired`.** The default already behaves this way; the thing to avoid is an auth-gated key, which would fail in the background for the same reason.
- **No biometric gate on the signing path.** There is no user present to satisfy it. A biometric gate belongs on [export](#backup) and [transfer](#login-with-device), where there is.

**What this costs.** A device seized while locked but powered on since its last unlock has the key recoverable by anyone who can execute code as the app. The data-protection class is not a barrier there; the barriers left are the OS's own — app sandboxing, and the exploit needed to get inside it. `privacy.md` states it in [What we do not defend against](./privacy.md#what-we-do-not-defend-against).

### No hardware-backed key

**The identity key cannot live in the Secure Enclave or in hardware-backed Keystore, on either platform.** Both do NIST P-256; nostr is secp256k1, and Android Keystore dropped secp256k1 support years ago.

So what secure storage holds is the key *bytes*, protected at rest by the platform, and signing happens in the core with `secp256k1`. The shell owns Keychain and Keystore and hands the bytes to the core through a uniffi callback, which zeroizes after use.

## Login with device

Identity moves between devices over the same proximity stack — BLE, Noise-encrypted, same session lifecycle, driven by the same core. This is the multi-device story, and the first half of the backup story. It is always available, since the app always holds the key.

Requirements, because this is deliberate key exfiltration:

- **Explicit on both ends.** Initiated by user action on the source and confirmed by user action on the target. Never automatic, never a background capability — this is the one flow the core will not run during a background wake, whatever the session state says.
- **Short authentication string.** Both devices display a derived comparison value the user checks by eye before the key moves. Noise XX authenticates the channel to static keys, but the user has no prior knowledge of the target's static key — the SAS is what stops an active attacker posing as the intended device. Same mechanism as Bluetooth numeric comparison or Signal safety numbers.
- **Copy, not move.** The source retains the key; both devices end up holding the same identity. Multi-device is more useful than migration, and a failed "move" that destroys the source key is unrecoverable.
- **Foreground and unlocked, by policy.** The key is [readable while the device is locked](#the-key-is-readable-while-the-device-is-locked), so this is an app-level gate rather than a storage-class one. The SAS comparison enforces it in practice by needing someone to look at both screens, and a biometric or passcode prompt belongs here, where a user is present to answer it.

## Backup

Multi-device is the happy path — two devices holding the same key means losing one costs nothing. But most users have one phone, so there is also a file export, following Flotilla's `KeyDownload.svelte`.

A single text file containing prose instructions and the key itself:

- **Optionally password-encrypted.** NIP-19 `nsec` encoding and NIP-49 `ncryptsec` encryption both happen in the core, which hands the view a finished string rather than key bytes. The user chooses; encryption is opt-in behind a toggle, not a wall. In the unencrypted case that string *is* the key, so it does cross the bridge — deliberately, since exporting it is the whole point of the flow. In the encrypted case nothing usable does.
- **Minimum 12-character password**, stated in the UI, with "write this down" guidance. There is no recovery for a forgotten backup password either.
- **Instructions, not just a key.** The copy explains what a keypair is, why the private half matters, and what to do with the file — following Flotilla's, with one adjustment. Flotilla tells the user to "import into a Nostr Signer app," which is misleading here, because this app holds the key itself. The wording instead says the key restores this app, and separately identifies them on the open network via other clients.
- **Gated flow.** The download must complete before the user can continue past the screen, so nobody skips it by accident.

On device the `<a download>` blob trick is a no-op in native webviews, so the file is written with `@capacitor/filesystem` to `Directory.Cache` and handed to `@capacitor/share`. Android carries a FileProvider entry (`android/app/src/main/res/xml/file_paths.xml`) for the cache directory. Dismissing the share sheet rejects with "Share canceled", which counts as *not downloaded* rather than an error, so the user can retry. Flotilla's `downloadText` in `src/lib/html.ts` is the reference.

## Advertisement identity

The BLE advertisement carries no identity. See [`discovery.md`](./discovery.md) — this is forced by iOS background behaviour rather than chosen.
