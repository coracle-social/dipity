# Discovery

How peers find each other, decide to connect, identify each other, and decide when a session is over.

## The advertisement carries nothing

The BLE advertisement is a bare presence beacon: our service UUID and nothing else. iOS imposes this: there is no payload to put an identifier in once the app is backgrounded.

When an iOS app advertises in the background, the local name and service data are stripped, and the 128-bit service UUID moves to the "overflow" area — readable only by another iOS device explicitly scanning for that exact UUID. In the background-to-background case, which is the case that matters, there is no payload to read. Any scheme that puts a resolvable identifier in the advertisement works in the foreground and silently stops working in the pocket.

Bluetooth's own answer (Resolvable Private Addresses with a shared IRK) is also unavailable: it operates at the address layer, requires bonding, and CoreBluetooth never exposes peer MAC addresses to apps — you get a per-app `CBPeripheral` identifier instead.

**Consequence: identification is always post-connect.** A stranger and a close friend are indistinguishable until the GATT link is up and the handshake has run.

**Service UUID:** a single UUID, pinned. Debug builds use a separate one so development devices don't join production meshes.

## Making identification cheap

Every identification costs a connection.

**Peripheral cache.** `CBPeripheral` identifiers are stable per-app-per-device until the peer's BLE address rotates — roughly 15 minutes for non-bonded devices. What was resolved for a peripheral identifier is cached with a TTL matched to that, so the same person is not re-identified on every rediscovery. bitchat's `BLERecentPeripheralCache` is exactly this.

It is also the only handle the backoff below has: nothing the handshake establishes outlives the session ([`transport.md`](./transport.md#the-static-key-is-generated-per-session)), so rate limiting works at BLE-address granularity or not at all.

**Identification budget.** In a busy place, connecting to everyone is neither possible nor desirable. Attempts are rate-limited, candidates are ordered by RSSI so the nearest stranger is tried first, and peers already identified and declined get a hard backoff.

## Connection scheduling

- RSSI floor around −90 dBm; candidates below it are queued, not dialled.
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
      │ SECURED │  channel encrypted; nobody identified
      └────┬────┘
           │ recognition, consent gate, then mutual NIP-42 (dialler first)
      ┌────▼─────┐
      │IDENTIFIED│  nostr pubkeys bound; policy evaluated
      └────┬─────┘
           │ sync begins
      ┌────▼────┐
      │ SYNCING │  events, then blobs, over BLE
      └────┬────┘
           │ heartbeat lost
      ┌────▼────┐
      │DRAINING │  no new work; in-flight transfers finish or time out
      └────┬────┘
           │
      ┌────▼────┐
      │ CLOSED  │  links torn down; synced data retained
      └─────────┘
```

### Recognition

The first frames on the secured channel are a recognition exchange, and the consent gate reads its result. There is no durable identity in the handshake to read instead: the Noise static key is [generated per session](./transport.md#the-static-key-is-generated-per-session).

At pairing, both sides derive a **pair secret** from the authenticated session and store it against the peer. On a later encounter, each proves it holds one without naming it:

```
tag = HMAC(pair_secret, h)      # h is this session's Noise handshake hash
```

The sender emits one tag per pair secret it holds; the receiver trial-MACs its own secrets against the list, a few microseconds per entry. A match identifies the relationship, and with it the policy stored against that peer.

**The dialler sends first**, and the peer answers only if a tag resolves or it is inside a discoverable window. A harvester that dials gets a list of random-looking bytes.

**The list is padded to a fixed count**, so its length does not disclose how many peers the device has paired with, and a long history does not put more on the wire. Resolution stays cheap against the full set.

**Recognition is voluntary, so it cannot deny anything.** A peer who would rather not be recognised omits their tag and arrives as a stranger. Involuntary recognition and involuntary trackability are the same property: a key that identifies a peer against their wishes is one any stranger can also demand. So there is no pre-authentication blocklist; what bounds a hostile peer instead is [ordering the authentication](#the-dialler-authenticates-first).

### The consent gate

Between SECURED and IDENTIFIED. Authenticating discloses a long-term nostr identity to whoever is nearby, and on a proximity transport also discloses that you were physically present at a time and place. It is not automatic for unknown peers.

**The decision is stored, not prompted.** Most encounters happen with both phones in pockets, so the gate is evaluated by the core with no view to raise a dialog in. A prompt that cannot be shown has to resolve to a decision the user already made, which the core reads from [stored preferences](./policy.md):

- **Recognised peers** — those whose [tag resolved](#recognition) — get whatever the pair record says: authenticate silently, or close without disclosing anything.
- **Everyone else** is admitted only inside an open discoverable window: a mode the user turned on in the foreground, stored with an expiry. Outside one, an unrecognised peer gets no authentication; the session stops at SECURED and closes, having learned nothing that outlives it.

Two criteria that look available are not. "Followed" requires knowing who the peer is, which requires the authentication being gated. "Explicit user action" only exists when someone is looking at the screen; the foreground can still offer it live, but the design cannot depend on it. The durable form of consent is a window the user opened earlier, so meeting new people is something the user switches on rather than something that happens continuously.

**Inside a discoverable window you disclose to strangers.** The two mechanisms below bound that rather than prevent it.

#### The dialler authenticates first

Mutual NIP-42 runs in both directions, and NIP-42 does not require them to be simultaneous ([`nip-p2p-auth.md`](./nip-p2p-auth.md#mutual-authentication)). So the side that dialled sends its auth event first, and the side that was dialled evaluates policy against a known pubkey before deciding whether to reciprocate.

A [blocked](./sync.md#trust-block-and-mute-do-different-jobs) pubkey that dials us is closed on before we disclose anything. That does not survive a fresh pubkey — nothing here does.

The direction is what makes it work. If the dialled side went first, a harvester could sweep a street and make every discoverable device in it disclose for free; making the dialler pay means it learns nothing unless its target is discoverable, and burns a pubkey per attempt. A beacon that waits to be dialled still collects from devices that dial it during a discoverable window, but its yield is bounded by our identification budget rather than by its radio.

#### The disclosure budget

Inside a discoverable window there is no way to deny a repeat visitor, so the yield is bounded instead: **a cap on new pubkeys disclosed per window** ([`policy.md`](./policy.md#discoverability)). It fails closed and needs no notion of who anyone is, which bounds what a beacon camped in a busy place collects over an afternoon.

## Heartbeat and teardown

The heartbeat is **liveness, not authorization**. Proximity is already guaranteed by transport configuration (see [`transport.md`](./transport.md)), so the heartbeat's only job is cleanup, and it can afford to be lenient.

Interval: 15–30 s when connected, jittered. A 60 s timeout is 2–4 missed beacons, matching bitchat's reachability window.

| Condition | Action |
| --- | --- |
| BLE link healthy | Nothing. |
| Heartbeat missed, session idle | After 60 s → DRAINING → CLOSED. |
| Heartbeat missed, transfer in flight | DRAINING: accept no new work, let in-flight transfers finish. Hard cap 5 min. Do not kill a working transfer over two missed beacons — radio contention during bulk transfer and iOS background throttling both cause them. |
| Clean BLE disconnect event | Immediate DRAINING, no timeout. Disconnects are reliable when they fire; the timeout is for the ambiguous case. |
