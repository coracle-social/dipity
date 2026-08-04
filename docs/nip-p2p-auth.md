NIP-XX
======

Authentication over non-WebSocket transports
--------------------------------------------

`draft` `optional`

This NIP extends [NIP-42](https://github.com/nostr-protocol/nips/blob/master/42.md) to connections that are not WebSockets, and to sessions where both parties act as relay.

## Motivation

The `relay` tag in kind `22242` stops a relay from replaying a client's `AUTH` event to a different relay to impersonate that client. It holds a URL, which non-WebSocket transports do not have. Such connections also have no distinguished relay — each party serves the other, and both must authenticate.

Every participant in a peer-to-peer network is a relay, so every participant can replay. This NIP retains the binding by naming the relay by its transport public key instead of its URL.

Throughout, "relay" means the party issuing a challenge and "client" the party signing the response. In a peer-to-peer session each peer is both.

## Session identifier

The `relay` tag holds a session identifier: a URI naming the relay on the transport carrying this connection.

```
<transport>://<lowercase-hex public key>
```

| Transport | Scheme | Authority |
| --- | --- | --- |
| WebSocket | `ws` / `wss` | Relay URL, as in NIP-42 |
| iroh | `iroh` | 32-byte `EndpointId` |
| Noise | `noise` | 32-byte static public key |

Other transports MAY define schemes. The key MUST be long-term for the relay and MUST be authenticated by the transport handshake.

Transports that do not authenticate their endpoints to a key MUST NOT be used with this NIP. A plain TCP connection should run Noise or TLS first and use the resulting key.

## Authentication event

Unchanged from NIP-42, except that `relay` holds the session identifier of the party that issued the challenge:

```jsonc
{
  "kind": 22242,
  "created_at": 1740000000,
  "tags": [
    ["relay", "iroh://a3f2c1...9e"],
    ["challenge", "8f14e45fceea167a5a36dedd4bea2543"]
  ],
  "content": ""
}
```

Clients that build the tag from the connection URI need no changes: the transport supplies `iroh://…` where it supplied `wss://…`.

## Challenges

A challenge MUST carry at least 128 bits of CSPRNG entropy, MUST be scoped to one connection, and MUST be single-use. Challenges MUST NOT be derived from the peer's identity, the time, or any other guessable input.

## Verification

A relay receiving kind `22242` MUST check that:

1. the signature is valid;
2. `created_at` is within 600 seconds of its current time;
3. the `challenge` tag matches a challenge it issued on this connection and has not consumed;
4. the `relay` tag equals its own session identifier for this connection, byte-for-byte after lowercasing the hex authority.

WebSocket URL normalisation MUST NOT be applied to non-WebSocket schemes.

On success `event.pubkey` is the peer's identity for this connection, and the relay responds `["OK", <id>, true, ""]`. On failure it responds `["OK", <id>, false, "auth-required: <reason>"]`.

## Mutual authentication

Both parties MAY act as relay. Each MAY send `["AUTH", <challenge>]` as soon as the connection opens, without waiting for the peer.

The two directions are independent. Neither party's status gates the other's, and a party MUST NOT treat its own successful authentication as evidence about the peer.

## Rebinding

A connection has at most one authenticated identity per direction. A relay receiving a second valid authentication event MUST either reject it or replace the bound identity and re-evaluate every authorization decision made under the previous one. Subscriptions and permissions MUST NOT carry over.

## Compatibility

On WebSocket transports nothing changes. An implementation supporting this NIP is a conforming NIP-42 implementation.

## Security considerations

- Omitting check 4, or accepting a wildcard session identifier, lets every peer you authenticate to impersonate you to every other peer.
- Verification proves the peer holds the nostr key *and* is the endpoint of this transport session. The second half is false on unauthenticated transports, where a machine-in-the-middle can relay the whole exchange.
- Authenticating discloses a long-term identity, and on proximity transports also discloses physical presence at a time and place. Implementations SHOULD gate it behind consent or policy and SHOULD NOT authenticate automatically to unknown peers.
- This NIP authenticates; it does not establish keys. Confidentiality is the transport's concern.
- Devices without a network time source drift. Tightening the 600-second window will produce legitimate failures from long-offline peers.
