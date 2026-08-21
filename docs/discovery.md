# Discovery

How peers find each other, decide to connect, identify each other, and decide when a session is over.

## The advertisement carries nothing

The BLE advertisement is a bare presence beacon: our service UUID and nothing else. iOS imposes this: there is no payload to put an identifier in once the app is backgrounded.

When an iOS app advertises in the background, the local name and service data are stripped, and the 128-bit service UUID moves to the "overflow" area — readable only by another iOS device explicitly scanning for that exact UUID. In the background-to-background case, which is the case that matters, there is no payload to read. Any scheme that puts a resolvable identifier in the advertisement works in the foreground and silently stops working in the pocket.

Bluetooth's own answer (Resolvable Private Addresses with a shared IRK) is also unavailable: it operates at the address layer, requires bonding, and CoreBluetooth never exposes peer MAC addresses to apps — you get a per-app `CBPeripheral` identifier instead.

## Making identification cheap

Every identification costs a connection.

`CBPeripheral` identifiers are stable per-app-per-device until the peer's BLE address rotates — roughly 15 minutes for non-bonded devices. What was resolved for a peripheral identifier is cached with a TTL matched to that, so the same person is not re-identified on every rediscovery. bitchat's `BLERecentPeripheralCache` is exactly this.

Nothing the handshake establishes outlives the session ([`transport.md`](./transport.md#the-static-key-is-generated-per-session)), so rate limiting works at BLE-address granularity or not at all.

Attempts are rate-limited, candidates are ordered by RSSI so the nearest stranger is tried first, and peers already identified and declined get a hard backoff.

## Connection scheduling

- RSSI floor around −90 dBm; candidates below it are queued, not dialed.
- Concurrent central links capped at 6.
- Connect rate limiting, roughly one attempt per 0.5 s globally.
- Distinct backoff for "never answered a connect" versus "was connected and walked away." The second recovers fast, because those peers usually come back.

bitchat's `BLEConnectionScheduler.swift` is the reference for this.

## Session lifecycle

```
                ┌─────────┐
                │  IDLE   │  advertising + duty-cycled scanning
                └────┬────┘
                     │ RSSI + scheduler + budget admit
                ┌────▼────┐
                │ LINKED  │  GATT connected, no security
                └────┬────┘
                     │ Noise XX completes
                ┌────▼────┐
  ┌─────────────┤ SECURED │  channel encrypted; nobody identified
  │             └────┬────┘
  │                  │ recognition, dialer's gate, dialer's AUTH
  │        ┌─────────▼─────────┐
  ├────────┤ DIALER_IDENTIFIED │  dialer named; receiver has disclosed nothing
  │        └─────────┬─────────┘
  │                  │ receiver's gate, receiver's AUTH
  │             ┌────▼─────┐
  ├─────────────┤IDENTIFIED│  both pubkeys bound; policy evaluated on both sides
  │             └────┬─────┘
  │                  │ sync begins
  │             ┌────▼────┐
  │             │ SYNCING │  events, then blobs, over BLE
  │             └────┬────┘
  │                  │ heartbeat lost
  │             ┌────▼────┐
  │             │DRAINING │  no new work; in-flight transfers finish or time out
  │             └────┬────┘
  │                  │
  │             ┌────▼────┐
  └── refused ─►│ CLOSED  │  links torn down; synced data retained
                └─────────┘
```

### Recognition

The first frames on the secured channel are a recognition exchange, and the consent gate reads its result. At pairing, both sides derive a **pair secret** from the authenticated session and store it against the peer. On a later encounter, each proves it holds one without naming it:

```
tag = HMAC(pair_secret, h)      # h is this session's Noise handshake hash
```

The sender emits one tag per pair secret it holds; the receiver trial-MACs its own secrets against the list. A match identifies the relationship.

The dialer sends first, and the peer answers only if a tag resolves or it is inside a discoverable window. A harvester that dials gets a list of random-looking bytes. A peer who would rather not be recognized omits their tag and arrives as a stranger.

The list is padded to a fixed count, so its length does not disclose how many peers the device has paired with, and a long history does not put more on the wire. Resolution stays cheap against the full set.

### The consent gate

What the gate protects is the disclosure of a nostr pubkey, which names a long-term identity. On a proximity transport it also places that identity somewhere at a given time.

Before either party identifies itself, a recognition tag may resolve allowing the peer to be mapped to a pubkey. This allows policy to be used to decide whether to connect. Blocked peers retain their tags so we can keep dropping their connections.

If a peer isn't recognized, the app may refuse to connect depending on the user's [discoverability policy settings](./policy.md#discoverability). If this happens, the user should be notified so they can manually approve the connection. If the user doesn't respond, hang on to the connection for up to 5 minutes. The next time the peer connects (and it should retry for this reason), the user's decision gates the connection.

If neither party drops the connection, the dialer identifies itself first via [NIP 42 AUTH](./nips/p2p-auth.md#mutual-authentication). This gives the receiver the chance to drop the connection without identifying itself.

If the receiver wishes to continue, it then identifies itself to the dialer, which can choose to drop the connection as well based on the disclosed nostr identity. If neither peer drops, they enter SYNCING state.

## Heartbeat and teardown

The heartbeat is **liveness, not authorization**. Proximity is already guaranteed by transport configuration (see [`transport.md`](./transport.md)), so the heartbeat's only job is cleanup, and it can afford to be lenient.

Interval: 15–30 s when connected, jittered. A 60 s timeout is 2–4 missed beacons, matching bitchat's reachability window.

| Condition | Action |
| --- | --- |
| BLE link healthy | Nothing. |
| Heartbeat missed, session idle | After 60 s → DRAINING → CLOSED. |
| Heartbeat missed, transfer in flight | DRAINING: accept no new work, let in-flight transfers finish. Hard cap 5 min. Do not kill a working transfer over two missed beacons — radio contention during bulk transfer and iOS background throttling both cause them. |
| Clean BLE disconnect event | Immediate close. Disconnects are reliable when they fire, and nothing in flight can finish on a dead link, so there is no drain to wait out; the timeout is for the ambiguous case. |
