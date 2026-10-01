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
- Connect rate limiting, one attempt per second globally.
- Distinct backoff for "never answered a connect" versus "was connected and walked away." The second recovers fast, because those peers usually come back.
- A teardown this device decided is not a walk-away and does not recover fast. Policy blocking the peer, a consent gate lapsing, a frame the wire cannot carry: redialing in fifteen seconds only reaches the same refusal, so it waits a minute. A gate the user refused outright waits longer still.

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

One state sits off this line. **GATE_PENDING** is where an unadmitted stranger waits while the user is asked, entered from SECURED when the tags resolve to nobody and the discoverability preferences do not admit them; approving returns the link to SECURED and the exchange resumes, refusing or the five-minute hold lapsing closes it.

[Login with device](./keys.md#login-with-device) sits off it in the other direction: it is an exchange on the control channel rather than a state, available only from SYNCING and only while the app is in the foreground, and the session goes on syncing throughout. A refusal at either end ends the exchange and leaves the link alone.

Every state above IDENTIFIED has a deadline of its own as well, independent of the heartbeat: a link that has not named anybody within a minute of connecting is closed, or a peer that completes the handshake and then sends nothing but heartbeats would hold one of the six slots for as long as it keeps beating.

### Recognition

The first frames on the secured channel are a recognition exchange, and the consent gate reads its result. At pairing, both sides derive a **pair secret** from the authenticated session and store it against the peer. On a later encounter, each proves it holds one without naming it, by sending a tag that is an HMAC over the session's handshake hash, keyed on the pair secret. The sender emits one tag per pair secret it holds; the receiver trial-MACs its own secrets against the list. A match identifies the relationship.

The dialer sends first, and the peer answers only if a tag resolves or it is inside a discoverable window. A harvester that dials gets a list of random-looking bytes. A peer who would rather not be recognized omits their tag and arrives as a stranger.

The list is padded to a fixed count, so its length does not disclose how many peers the device has paired with, and a long history does not put more on the wire. Resolution stays cheap against the full set.

#### On the wire

The list is 32 tags of 32 bytes concatenated, so every device sends 1024 bytes on every encounter. Raw bytes rather than JSON, because the MTU is what binds this link and an array of decimal integers costs three times as much.

A device holding fewer than 32 pair secrets fills the rest with bytes from the CSPRNG. One holding more sends a random sample, redrawn each session, so a peer left out of one list is in the running for the next rather than permanently invisible. The list is shuffled either way, so a peer that finds its own tag learns nothing from where it sat.

A list of any other length is refused and the link dropped: a short one is malformed, and a long one is an invitation to trial-MAC against an unbounded set.

The secret each tag is keyed on comes from the session that established it:

```
pair_secret = SHA256("dip/pair-secret" ‖ h)      # h at pairing time
tag         = HMAC-SHA256(pair_secret, h)        # h at this encounter
```

Domain-separated, so the secret cannot collide with any other use of the handshake hash. A session that recognized the peer leaves the secret alone. One that authenticated the peer without recognizing it replaces the secret on both sides, so a pairing one device missed the end of is repaired the next time the two meet.

### The consent gate

What the gate protects is the disclosure of a nostr pubkey, which names a long-term identity. On a proximity transport it also places that identity somewhere at a given time.

Before either party identifies itself, a recognition tag may resolve allowing the peer to be mapped to a pubkey. This allows policy to be used to decide whether to connect. Blocked peers retain their tags so we can keep dropping their connections.

If a peer isn't recognized, the app may refuse to connect depending on the user's [discoverability policy settings](./policy.md#discoverability). If this happens, the user should be notified so they can manually approve the connection. If the user doesn't respond, hang on to the connection for up to 5 minutes. The next time the peer connects (and it should retry for this reason), the user's decision gates the connection.

The prompt carries a comparison value, derived from the session's handshake hash under its own domain label the way [login with device](./keys.md#login-with-device) derives its six digits. Noise XX authenticates nobody, and the gate runs before either side has named a pubkey, so this is the only thing the two users have to check: a device in the middle completes two handshakes and the two screens then disagree. A gate that cannot derive one asks nothing and stays held.

The value the gate shows is five shapes drawn from an alphabet of eight in three tints, which is 24^5 and a little over 23 bits. The screen draws the whole space rather than a prefix of it.

The pubkey each side proves afterwards is announced to the shell as it is proved. The gate runs first, so a pet name the user typed there is for the person in front of them, and this is what says which key that person holds.

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
