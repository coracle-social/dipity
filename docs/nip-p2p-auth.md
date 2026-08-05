# Peer authentication

Sessions authenticate with [NIP-42](https://github.com/nostr-protocol/nips/blob/master/42.md), used unmodified. The two things this app needs from it — naming a peer that has no URL, and running the exchange in both directions — were added to NIP-42 itself rather than specified separately here. NIP-42 is the specification; where it and this document disagree, it wins.

## What was added upstream

NIP-42's `relay` tag names the party that issued the challenge, and on WebSocket that name is a URL. A BLE link has no URL, so there was nothing to put in the tag. Leaving it out is not an option: without it an auth event is a bearer token for the pubkey that signed it, and here every participant is a relay, so every participant that receives one is in a position to replay it.

### Transport identities

Transports with no URL may define an identity scheme of the form `<transport>://<authority>`:

| Transport | Scheme | Authority |
| --- | --- | --- |
| WebSocket | `ws` / `wss` | Relay URL |
| iroh | `iroh` | 32-byte `EndpointId`, lowercase hex |
| Noise | `noise` | 32-byte static public key, lowercase hex |

The authority must be a long-term key of the challenging party, and the transport handshake must authenticate the peer to that key. A scheme is usable only if completing its handshake proves the peer holds the key the authority names, which rules out plain TCP unless something that does authenticate — Noise or TLS — is layered underneath and the key it establishes is the one named.

### The check runs on both sides

The verifier checks that the `relay` tag names it. The signer checks that the handshake authenticated the identity it is about to name, taking that identity from the completed handshake rather than from an address book or an advertisement.

Both are needed, and the second is the one that is easy to skip. An attacker posing as a third party collects an event naming that party and replays it to them, where the tag check passes — because the tag does name the recipient. The verifier cannot catch this on its own; the event is well-formed for it.

### Challenges

A challenge carries at least 128 bits from a CSPRNG, is scoped to a single connection, and is accepted once. A predictable or reused challenge lets a captured auth event be replayed to the same relay on a later connection.

### Mutual authentication

Both parties may act as relay. Each may send `["AUTH", <challenge>]` as soon as the connection opens without waiting for the peer, the two directions are independent, and neither party may treat its own success as evidence about the other.

### Verification

Two of NIP-42's existing checks were extended. A challenge must also not have been consumed already, and the `relay` tag comparison splits by scheme: `ws` and `wss` keep the existing tolerance for URL normalization, while every other scheme compares byte-exactly with normalization forbidden, each scheme defining a single canonical encoding for its authority.

Transports should also provide confidentiality. Not against replay, which the rules above cover, but because authenticating discloses a long-term identity to anyone listening, and on a proximity transport it discloses physical presence at a time and place.

## What it means here

- **The session identifier is `noise://<hex static key>`.** It names a key the BLE handshake has already authenticated, so the tag is checked against something the handshake established rather than something the peer claimed. This app registers no other scheme; the table above is the general registry. See [`transport.md`](./transport.md).
- **A captured auth event is useless against a relay.** On these transports the `relay` tag never holds a URL, so no relay will ever match it. See [`identity.md`](./identity.md#auth-events-are-ordinary-signed-events).
- **A peer may authenticate as several pubkeys**, since NIP-42 allows a sequence of `AUTH` messages. Ingest therefore tests set membership rather than equality — see [`sync.md`](./sync.md#delivery-grants).
- **Authentication sits behind the consent gate**, because it discloses presence. Sessions are never established automatically with unknown peers, and since the gate is usually evaluated with nobody looking at the screen, the decision comes from stored preferences rather than a prompt — see [`discovery.md`](./discovery.md#the-consent-gate).
- **The signature is produced in the background, by us.** This exchange is the reason custody is limited to a key the device holds: it runs during a CoreBluetooth wake, with the view suspended and possibly with no network, and a peer that cannot complete it can neither send nor receive. It is also why the identity key must be readable while the device is locked — see [`identity.md`](./identity.md#key-custody).
- **The `created_at` window is wider than NIP-42 suggests.** Its ~10 minutes assumes a network time source. Devices that have been offline for days drift, and rejecting them would fail exactly the case this app is built for.
