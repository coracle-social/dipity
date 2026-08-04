# Discovery

How peers find each other, decide to connect, identify each other, and decide when a session is over. See [`overview.md`](./overview.md).

## The advertisement carries nothing

The BLE advertisement is a bare presence beacon: our service UUID and nothing else. This is forced, not chosen.

When an iOS app advertises in the background, the local name and service data are stripped, and the 128-bit service UUID moves to the "overflow" area — readable only by another iOS device explicitly scanning for that exact UUID. In the background-to-background case, which is the case that matters, there is no payload to read. Any scheme that puts a resolvable identifier in the advertisement works in the foreground and silently stops working in the pocket.

Bluetooth's own answer (Resolvable Private Addresses with a shared IRK) is also unavailable: it operates at the address layer, requires bonding, and CoreBluetooth never exposes peer MAC addresses to apps — you get a per-app `CBPeripheral` identifier instead.

**Consequence: identification is always post-connect.** A stranger and a close friend are indistinguishable until the GATT link is up and the handshake has run.

**Service UUID:** a single UUID, pinned. Debug builds use a separate one so development devices don't join production meshes.

## Making identification cheap

Since every identification costs a connection, the work is in not paying for it twice.

**Peripheral cache.** `CBPeripheral` identifiers are stable per-app-per-device until the peer's BLE address rotates — roughly 15 minutes for non-bonded devices. The identity resolved for a peripheral identifier is cached with a TTL matched to that, so the same person is not re-identified on every rediscovery. bitchat's `BLERecentPeripheralCache` is exactly this, and exists specifically to arm pending background connections when the app leaves the foreground.

**Identification budget.** In a crowd, connecting to everyone is neither possible nor desirable. Attempts are rate-limited, candidates are ordered by RSSI so the nearest stranger is tried first, and peers already identified and declined get a hard backoff.

## Connection scheduling

- RSSI floor around −90 dBm; candidates below it are queued, not dialled.
- Concurrent central links capped at 6.
- Connect rate limiting, roughly one attempt per 0.5 s globally.
- Distinct backoff for "never answered a connect" versus "was connected and walked away." The second recovers fast, because those peers usually come back.

bitchat's `BLEConnectionScheduler.swift` is the reference for this, and is public domain.

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
      │ SECURED │  channel authenticated to Noise static keys
      └────┬────┘
           │ consent gate, then mutual NIP-42 (both directions)
      ┌────▼─────┐
      │IDENTIFIED│  nostr pubkeys bound; policy evaluated
      └────┬─────┘
           │ sync begins over BLE; address exchange
      ┌────▼────┐        iroh dial succeeds
      │ SYNCING ├───────────────────────┐
      └────┬────┘                       │
           │                       ┌────▼─────┐
           │                       │ UPGRADED │  bulk over QUIC,
           │                       └────┬─────┘  control stays on BLE
           │  heartbeat lost            │
      ┌────▼────┐◄──────────────────────┘
      │DRAINING │  no new work; in-flight transfers finish or time out
      └────┬────┘
           │
      ┌────▼────┐
      │ CLOSED  │  links torn down; synced data retained
      └─────────┘
```

### The consent gate

Between SECURED and IDENTIFIED. Authenticating discloses a long-term nostr identity to whoever is nearby, and on a proximity transport also discloses that you were physically present at a time and place. It is not automatic for unknown peers.

- **Known peers** — followed, or previously paired — authenticate silently.
- **Strangers** require an explicit user action, or an opt-in "discoverable" mode with a duration.

## Heartbeat and teardown

The heartbeat is **liveness, not authorization**. Proximity is already guaranteed by transport configuration (see [`transport.md`](./transport.md)), so the heartbeat's only job is cleanup, and it can afford to be lenient.

Interval: 15–30 s when connected, jittered. A 60 s timeout is 2–4 missed beacons, matching bitchat's reachability window.

| Condition | Action |
| --- | --- |
| BLE link healthy | Nothing. |
| Heartbeat missed, session idle | After 60 s → DRAINING → CLOSED. |
| Heartbeat missed, transfer in flight | DRAINING: accept no new work, let in-flight transfers finish. Hard cap 5 min. |
| BLE lost, iroh healthy | DRAINING. Do not kill a working transfer over two missed beacons — radio contention during bulk transfer and iOS background throttling both cause this. |
| iroh lost, BLE healthy | Downgrade to BLE. Session continues. Retry upgrade on address change. |
| Clean BLE disconnect event | Immediate DRAINING, no timeout. Disconnects are reliable when they fire; the timeout is for the ambiguous case. |

**welshman note:** `socketPolicyCloseInactive` (30 s idle close) and `socketPolicyPing` (30 s WebSocket ping) both fight this. Peer adapters opt out of `defaultSocketPolicies` and carry their own.
