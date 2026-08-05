# Keys

The nostr identity key: what holds it, what reads it, and how it moves between devices.

## The nostr identity

One secp256k1 keypair, long-term and user-facing. It signs delivery grants and auth events — the only two signed kinds ([`sync.md`](./sync.md#delivery-grants)) — and it is the identity users see and follow.

The Noise static key that authenticates the BLE channel is a separate key with a separate job, described in [`transport.md`](./transport.md#channel-security). The two are bound only per session, by mutual NIP-42 ([`sync.md`](./sync.md#authentication)).

### The key is not confined

The gossip protocol isolates *events*, not identities. The nsec is an ordinary secp256k1 key, so the same pubkey can sign and post on the open network from any other nostr client.

That is fine — it is the same person either way — but proximity activity and open-network activity under one pubkey are trivially linkable by anyone who sees both.

## Key custody

**One model: the app holds the key.** Generated on device or imported, kept in platform secure storage, and read by the core when a signature is needed. There is no external signer path.

### Signing happens at encounter time, in the background

Two event kinds are signed (22242 auth events and delivery grants), and both may occur when the app is backgrounded or the device is locked. What this implies for the accessibility class:

- **iOS: `kSecAttrAccessibleAfterFirstUnlockThisDeviceOnly`.** Readable from the moment the user first unlocks after boot, including while locked afterwards. `ThisDeviceOnly` keeps it out of iCloud Keychain and encrypted device backups — moving the identity to another device is [an explicit flow](#login-with-device), not an implicit sync.
- **Android: a Keystore-wrapped key with no `setUserAuthenticationRequired`.** The default already behaves this way; the thing to avoid is an auth-gated key, which would fail in the background for the same reason.
- **No biometric gate on the signing path.** There is no user present to satisfy it. A biometric gate belongs on [export](#backup) and [transfer](#login-with-device), where there is.

### No hardware-backed key

**The identity key cannot live in the Secure Enclave or in hardware-backed Keystore, on either platform.** Both do NIST P-256; nostr is secp256k1, and Android Keystore dropped secp256k1 support years ago.

So what secure storage holds is the key *bytes*, protected at rest by the platform, and signing happens in the core with `secp256k1`. The shell owns Keychain and Keystore and hands the bytes to the core through a uniffi callback, which zeroizes after use.

## Login with device

Identity moves between devices over the same proximity stack — BLE, Noise-encrypted, same session lifecycle, driven by the same core. This is the multi-device story, and the first half of the backup story. It is always available, since the app always holds the key.

Requirements, because this is deliberate key exfiltration:

- **Explicit on both ends.** Initiated by user action on the source and confirmed by user action on the target. Never automatic, never a background capability — this is the one flow the core will not run during a background wake, whatever the session state says.
- **Short authentication string.** Both devices display a derived comparison value the user checks by eye before the key moves. Noise XX authenticates the channel to static keys, but the user has no prior knowledge of the target's static key — the SAS is what stops an active attacker posing as the intended device. Same mechanism as Bluetooth numeric comparison or Signal safety numbers.

## Backup

Multi-device is the happy path — two devices holding the same key means losing one costs nothing. But most users have one phone, so there is also a file export, following Flotilla's `KeyDownload.svelte`.

A single text file containing prose instructions and the key itself:

- **Optionally password-encrypted.** NIP-19 `nsec` encoding and NIP-49 `ncryptsec` encryption both happen in the core, which hands the view a finished string rather than key bytes. The user chooses; encryption is opt-in behind a toggle, not a wall. In the unencrypted case that string *is* the key, so it does cross the bridge — deliberately, since exporting it is the whole point of the flow. In the encrypted case nothing usable does.
- **Minimum 12-character password**, stated in the UI, with "write this down" guidance. There is no recovery for a forgotten backup password either.
- **Instructions, not just a key.** The copy explains what a keypair is, why the private half matters, and what to do with the file — following Flotilla's, with one adjustment. Flotilla tells the user to "import into a Nostr Signer app," which is misleading here, because this app holds the key itself. The wording instead says the key restores this app, and separately identifies them on the open network via other clients.
- **Gated flow.** The download must complete before the user can continue past the screen, so nobody skips it by accident.

On device the `<a download>` blob trick is a no-op in native webviews, so the file is written with `@capacitor/filesystem` to `Directory.Cache` and handed to `@capacitor/share`. Android carries a FileProvider entry (`android/app/src/main/res/xml/file_paths.xml`) for the cache directory. Dismissing the share sheet rejects with "Share canceled", which counts as *not downloaded* rather than an error, so the user can retry. Flotilla's `downloadText` in `src/lib/html.ts` is the reference.
