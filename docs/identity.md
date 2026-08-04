# Identity

See [`overview.md`](./overview.md).

## Keys

Three keypairs, three jobs. Keeping them distinct is what makes the transport binding in [`nip-p2p-auth.md`](./nip-p2p-auth.md) meaningful.

| Key | Type | Scope | Purpose |
| --- | --- | --- | --- |
| nostr identity | secp256k1 | Long-term, user-facing | Signs delivery grants and auth events, both ordinary nostr events. The identity users see and follow. |
| Noise static | Curve25519 | Long-term, per-install | Authenticates the BLE channel. |
| iroh endpoint | ed25519 | Long-term, per-install | Authenticates the QUIC session. |

The transport keys and the nostr identity are bound at session time by mutual NIP-42 with transport binding. Nothing else binds them — no long-lived mapping is published anywhere, and the binding is scoped to the session.

## Events are not signed, grants are

Content events carry an id and no `sig`. Authenticity comes instead from a **delivery grant**: a small signed nostr event by the author, tagging the content event id and one recipient pubkey, specified in [`sync.md`](./sync.md#delivery-grants).

Ids are plain NIP-01 hashes. Nothing about serialization is app-specific, so an id computed here matches what any nostr implementation would compute for the same content, and `@welshman/util` is used unmodified.

This arrangement carries two properties that used to need separate mechanisms:

- **Authenticity.** A grant covers the id, and only the author's key can produce one, so verifying a grant proves the author produced exactly this content. An event with no valid grant is never stored.
- **Containment.** An event with no `sig` is not a valid nostr event. Proximity content that escapes to a relay is rejected on arrival rather than stored, and no existing client can render it — without relying on any tag, convention, or relay policy.

### Consequences, accepted deliberately

- **The author can promote their own posts, and only their own.** Holding the key, they can sign one of their events normally and publish it to the open network. Because ids are canonical, a post promoted that way keeps its id and its replies still resolve. Nobody can do this for anyone else's content, which is the property that matters.
- **No interoperability in the other direction.** Unsigned events cannot be read or stored by relays or existing clients, so nothing in the wider ecosystem is a fallback.
- **External signers work, with reduced capability.** Grants and auth events are ordinary events, so a NIP-55 signer produces both without modification. What it will not do is surrender the key, so backup files and login with device are unavailable on that path. See [Key custody](#key-custody).

### Auth events are ordinary signed events

Kind 22242 auth events are real nostr events, signed normally, because NIP-42 requires a verifiable signature and the peer checks it as one. They are the exception to "content events are unsigned", and they are the one place `@welshman/signer` is still used as-is.

Transport binding keeps them from being replayed anywhere useful — the `relay` tag is `noise://…` or `iroh://…`, which no relay will ever match. See [`nip-p2p-auth.md`](./nip-p2p-auth.md).

### The key is not confined, only the content

Worth being clear about: this isolates *events*, not identities. The nsec is an ordinary secp256k1 key, so the same pubkey can sign and post on the open network from any other nostr client.

That is fine — it is the same person either way — but proximity activity and open-network activity under one pubkey are trivially linkable by anyone who sees both.

## Key custody

Two login paths, and which one a user takes decides what else the app can offer them.

**In-app key.** Generated in the app or imported, held in platform secure storage: Keychain on iOS (with an accessibility class that survives backgrounding but not device-unlocked-once), Keystore / EncryptedSharedPreferences on Android.

**NIP-55 external signer.** On Android, signing is delegated to a signer app and the key never reaches us. Grants and auth events are ordinary nostr events, so the signer handles both unmodified — this is the practical payoff of grants being events rather than bare signatures.

**NIP-46 bunker.** Same shape, except the signer is remote and each signature is a round trip over a relay.

| | In-app key | NIP-55 signer | NIP-46 bunker |
| --- | --- | --- | --- |
| Signing grants and auth events | yes | yes | yes, over the network |
| Producing grant proofs | yes | yes | yes — proves knowledge of a grant we hold, not of our key |
| [Backup file](#backup) | yes | no — nothing to export | no — nothing to export |
| [Login with device](#login-with-device) | yes | no — nothing to transfer | no — nothing to transfer |
| Peering with no internet | yes | yes | **no** |

The backup and login-with-device gaps are structural rather than policy: those features move a key the app does not have. The UI omits them on those paths rather than failing when they are used.

The last row is the one that bites. Establishing a session needs a signed kind 22242 in both directions ([`nip-p2p-auth.md`](./nip-p2p-auth.md)), and issuing grants needs a signature per chunk. A bunker user with no connectivity can do neither, so they cannot peer at all — not send, not receive. Forwarding is the exception, because a [grant proof](./sync.md#grant-proofs) demonstrates knowledge of a grant already held rather than producing a signature; but with no session there is nobody to forward to. Offer NIP-46 with that stated plainly, since a crowd with no signal is exactly where this app is meant to work.

I3 scopes offline-first to the gossip protocol, not to the device, so a login method that needs network is allowed. It just has to be honest about when it stops working — hence the last row above.

## Login with device

Identity moves between devices over the same proximity stack — BLE or iroh, Noise-encrypted, same session lifecycle. This is the multi-device story, and the first half of the backup story.

**Offered only when the app holds the key.** With a NIP-55 signer there is nothing to transfer; the user installs their signer on the second device instead, and the app never sees the key on either.

Requirements, because this is deliberate key exfiltration:

- **Explicit on both ends.** Initiated by user action on the source and confirmed by user action on the target. Never automatic, never a background capability.
- **Short authentication string.** Both devices display a derived comparison value the user checks by eye before the key moves. Noise XX authenticates the channel to static keys, but the user has no prior knowledge of the target's static key — the SAS is what stops an active attacker posing as the intended device. Same mechanism as Bluetooth numeric comparison or Signal safety numbers.
- **Copy, not move.** The source retains the key; both devices end up holding the same identity. Multi-device is more useful than migration, and a failed "move" that destroys the source key is unrecoverable.
- **Source device unlocked.** The transfer requires the key to be readable from secure storage, which requires the device be unlocked, not merely powered on.

## Backup

**Offered only when the app holds the key.** With a NIP-55 signer there is nothing to export and the signer app owns backup; the flow below does not appear.

Multi-device is the happy path — two devices holding the same key means losing one costs nothing. But most users have one phone, so there is also a file export, following Flotilla's `KeyDownload.svelte`.

A single text file containing prose instructions and the key itself:

- **Optionally password-encrypted.** Unencrypted is `nsecEncode` from `nostr-tools/nip19`; encrypted is `encrypt` from `nostr-tools/nip49`, producing an `ncryptsec`. The user chooses; encryption is opt-in behind a toggle, not a wall.
- **Minimum 12-character password**, stated in the UI, with "write this down" guidance. There is no recovery for a forgotten backup password either.
- **Instructions, not just a key.** The copy explains what a keypair is, why the private half matters, and what to do with the file — following Flotilla's, with one adjustment. Flotilla tells the user to "import into a Nostr Signer app," which is misleading here, because on this path the app holds the key itself. The wording instead says the key restores this app, and separately identifies them on the open network via other clients.
- **Gated flow.** The download must complete before the user can continue past the screen, so nobody skips it by accident.

On device the `<a download>` blob trick is a no-op in native webviews, so the file is written with `@capacitor/filesystem` to `Directory.Cache` and handed to `@capacitor/share`. Android carries a FileProvider entry (`android/app/src/main/res/xml/file_paths.xml`) for the cache directory. Dismissing the share sheet rejects with "Share canceled", which counts as *not downloaded* rather than an error, so the user can retry. Flotilla's `downloadText` in `src/lib/html.ts` is the reference.

## Advertisement identity

The BLE advertisement carries no identity. See [`discovery.md`](./discovery.md) — this is forced by iOS background behaviour rather than chosen.
