# Keys

The nostr identity key: what holds it, what reads it, and how it moves between devices.

## The nostr identity

One secp256k1 keypair, long-term and user-facing. It signs kind 22242 auth events and [authorship signatures](./proofs.md#the-authors-signature-stays-with-the-peer-it-names), and it is the identity users see and follow.

The Noise static key that encrypts the BLE channel is a separate key with a separate job, described in [`transport.md`](./transport.md#channel-security). Mutual NIP-42 binds the two for the life of one session.

### The key is not confined

The gossip protocol isolates *events*, not identities. The nsec is an ordinary secp256k1 key. The same pubkey can sign and post on the open network from any other nostr client.

That is fine — it is the same person either way — but proximity activity and open-network activity under one pubkey are trivially linkable by anyone who sees both.

## Key custody

Whether generated on device or imported, the nostr key is kept in platform secure storage, and read by the core when a signature is needed. There is no external signer path.

### Signing happens in the background

The key signs during encounters — auth events and authorship signatures — which may happen while the app is backgrounded or the device is locked. What this implies for the accessibility class:

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
- **Short authentication string.** Both devices display a comparison value derived from the transcript, which the user checks by eye before the key moves. Nothing else authenticates this flow: the Noise handshake authenticates nobody ([`transport.md`](./transport.md#the-static-key-is-generated-per-session)), and NIP-42 cannot help, because the target authenticates as the identity it made at first run, which says nothing about whether it is the user's own phone. Same mechanism as Bluetooth numeric comparison or Signal safety numbers. The value is six decimal digits, the first four bytes of `SHA-256("dip/login-with-device/sas" ‖ handshake_hash)`, read big-endian and taken modulo a million. Both users are asked the same question — does the other device show this number — and either may answer first; the key moves once both have said yes.
- **Only an unused phone receives.** First run offers to make a stand-in key and wait for the user's other phone, because the target needs a key of its own to authenticate the session. The core turns an offer away without asking its user once the store holds anything the target's own identity wrote, because adopting another key would orphan it — names, lists and bookmarks published under the old key would read as nobody's, and retention would stop sparing the old posts. A phone that has been used logs out first.
- **The stand-in identity goes.** The target adopts the transferred key in place of the stand-in, and empties its store, since whatever it gathered belongs to an identity nobody uses again.

Logging out deletes the key and empties the store in the same way, and returns the phone to first run.

## Backup

Multi-device is the happy path — two devices holding the same key means losing one costs nothing. There is also a file export, following Flotilla's `KeyDownload.svelte`, because most users have one phone.

A single text file containing prose instructions and the key itself:

- **Optionally password-encrypted.** NIP-19 `nsec` encoding and NIP-49 `ncryptsec` encryption both happen in the core. The user chooses; encryption is opt-in behind a toggle, not a wall.
- **The view never holds the key, encrypted or not.** It starts the flow and learns only *shared* or *canceled*, the second counting as not downloaded rather than an error, so the user can retry.
- **Minimum 12-character password**, stated in the UI, with "write this down" guidance. There is no recovery for a forgotten backup password either.
- **Instructions, not just a key.** The copy explains what a keypair is, why the private half matters, and what to do with the file — following Flotilla's, with one adjustment. Flotilla tells the user to "import into a Nostr Signer app," which is misleading here, because this app holds the key itself. The wording instead says the key restores this app, and separately identifies them on the open network via other clients.

The key never crosses the bridge, because the core writes the file to the cache directory and the shell presents the share sheet. Android carries a FileProvider entry (`android/app/src/main/res/xml/file_paths.xml`) for the cache directory. Returning a path instead would not help, since `Capacitor.convertFileSrc` lets the view fetch any path back.
