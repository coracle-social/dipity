# Discovery

How peers find each other, decide to connect, identify each other, and decide when a session is over.

## The advertisement carries nothing

The BLE advertisement is a bare presence beacon: our service UUID and nothing else. iOS imposes this: there is no payload to put an identifier in once the app is backgrounded.

When an iOS app advertises in the background, the local name and service data are stripped, and the 128-bit service UUID moves to the "overflow" area: Apple manufacturer data of type `0x01`, a bitmask with one bit per backgrounded service. iOS matches it against a scan for that exact UUID. Android cannot, because which bit is ours is undocumented. An Android scan matches every overflow advertisement, and a dial that finds no service of ours sets that device aside until its address rotates. In the background-to-background case, which is the case that matters, there is no payload to read. Any scheme that puts a resolvable identifier in the advertisement works in the foreground and silently stops working in the pocket.

Bluetooth's own answer (Resolvable Private Addresses with a shared IRK) is also unavailable: it operates at the address layer, requires bonding, and CoreBluetooth never exposes peer MAC addresses to apps — you get a per-app `CBPeripheral` identifier instead.

## Making identification cheap

Every identification costs a connection.

`CBPeripheral` identifiers are stable per-app-per-device until the peer's BLE address rotates — roughly 15 minutes for non-bonded devices. What was resolved for a peripheral identifier is cached with a TTL matched to that, so that the same person is not re-identified on every rediscovery. bitchat's `BLERecentPeripheralCache` is exactly this.

Rate limiting works at BLE-address granularity or not at all, because nothing the handshake establishes outlives the session ([`transport.md`](./transport.md#the-static-key-is-generated-per-session)).

Attempts are rate-limited, candidates are ordered by RSSI so the nearest stranger is tried first, and peers already identified and declined get a hard backoff.

## Connection scheduling

- RSSI floor around −90 dBm; candidates below it are queued, not dialed.
- Concurrent central links capped at 6.
- Connect rate limiting, one attempt per second globally.
- Distinct backoff for "never answered a connect" versus "was connected and walked away." The second recovers fast, because those peers usually come back.
- A dial that fails before a link comes up is retried within a few seconds, jittered, because the usual cause is two phones dialing each other at the same moment.
- A dropped link is redialed when its backoff lapses, without waiting for the radio to report the peer again, because a peer stays queued after it is dialed.
- Two phones that dial each other end up with two links to one person. Both keep the one the lower pubkey dialed and close the other as soon as both have named the peer, which needs no message between them. Each link has its own transcript and so its own six digits, and a pairing request that arrives over both before the duplicate closes is shown once, under the kept link's digits.
- A peripheral that took a disclosure and left before syncing is not redialed until its address rotates, so that a harvester cannot drain the [disclosure bucket](./policy.md#discoverability) by being dialed again.
- A peer that synced and then walked away is waited for: a standing connect that does not time out and survives the app being suspended, `connect` on iOS and an auto-connect on Android, given up once the peer's address has rotated. A suspended iPhone runs no timer. Without the standing connect, a peer coming back is reached only if somebody happens to scan. A link that never synced gets no wait, since a peer that refused us would be reconnected the moment it was back.
- A walk-away in the background restarts the scan, because iOS reports each peripheral once per scan there and a peer it already reported would otherwise never be seen again.
- A Bluetooth relaunch keeps the connects still pending and tears down only the connected peripherals, whose sessions did not survive.
- A teardown this device decided is not a walk-away and does not recover fast. Policy blocking the peer, a consent gate lapsing, a frame the wire cannot carry: each waits a minute, because redialing in fifteen seconds only reaches the same refusal. A gate the user refused outright waits longer still.

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

One state sits off this line. **GATE_PENDING** is where an unadmitted stranger waits while the user is asked, entered from SECURED when the tags resolve to nobody and the [disclosure bucket](./policy.md#discoverability) does not admit them; approving returns the link to SECURED and the exchange resumes, refusing or the five-minute hold lapsing closes it.

[Login with device](./keys.md#login-with-device) sits off it in the other direction: it is an exchange on the control channel rather than a state, available only from SYNCING and only while the app is in the foreground, and the session goes on syncing throughout. A refusal at either end ends the exchange and leaves the link alone.

Every state above IDENTIFIED has a deadline of its own as well, independent of the heartbeat: a link that has not named anybody within a minute of connecting is closed, or a peer that completes the handshake and then sends nothing but heartbeats would hold one of the six slots for as long as it keeps beating.

### Recognition

The first frames on the secured channel are a recognition exchange, and the consent gate reads its result. At pairing, both sides derive a **pair secret** from the authenticated session and store it against the peer. On a later encounter, each proves it holds one without naming it, by sending a tag that is an HMAC over the session's handshake hash, keyed on the pair secret. The sender emits one tag per pair secret it holds; the receiver trial-MACs its own secrets against the list. A match identifies the relationship.

The dialer sends first, and the peer answers only if a tag resolves or its disclosure bucket admits a stranger. A harvester that dials gets a list of random-looking bytes. A peer who would rather not be recognized omits their tag and arrives as a stranger. Removing somebody from contacts drops the pair secret held for them. The two then meet as strangers until a session completes and pairs them again.

The list is padded to a fixed count so that its length does not disclose how many peers the device has paired with, and a long history does not put more on the wire. Resolution stays cheap against the full set.

#### On the wire

The list is 32 tags of 32 bytes concatenated, which is 1024 bytes from every device on every encounter. Raw bytes rather than JSON, because the MTU is what binds this link and an array of decimal integers costs three times as much.

A device holding fewer than 32 pair secrets fills the rest with bytes from the CSPRNG. One holding more sends a random sample, redrawn each session, so that a peer left out of one list is in the running for the next rather than permanently invisible. The list is shuffled either way, so that a peer that finds its own tag learns nothing from where it sat.

A list of any other length is refused and the link dropped: a short one is malformed, and a long one is an invitation to trial-MAC against an unbounded set.

The secret each tag is keyed on comes from the session that established it:

```
pair_secret = SHA256("dip/pair-secret" ‖ h)      # h at pairing time
tag         = HMAC-SHA256(pair_secret, h)        # h at this encounter
```

Domain-separated so that the secret cannot collide with any other use of the handshake hash. A session that recognized the peer leaves the secret alone. One that authenticated the peer without recognizing it replaces the secret on both sides, so that a pairing that one device did not finish is repaired the next time the two meet.

### The consent gate

What the gate protects is the disclosure of a nostr pubkey, which names a long-term identity. On a proximity transport it also places that identity somewhere at a given time.

Before either party identifies itself, a recognition tag may resolve allowing the peer to be mapped to a pubkey. This allows policy to be used to decide whether to connect. Refused peers retain their tags so we can keep dropping their connections.

If a peer isn't recognized, the app may hold the connection depending on the user's [discoverability policy settings](./policy.md#discoverability). A held stranger is not put in front of the user: the hold waits for the app to come to the front, where every stranger is admitted, and lapses after 5 minutes otherwise. Nothing about a held stranger is worth a prompt, because nobody is named yet and the user could only be asked whether to let an unknown phone in.

A receiver that would hold a stranger first lets the exchange run until the dialer has proved its pubkey, which the dialer discloses first. A pubkey the user has named, which is a [contact](./policy.md#social-graph), passes without a hold and without spending a disclosure, which repairs a pair secret only one of the two phones kept. Anybody else is held.

Every link carries a comparison value, derived from the session's handshake hash under its own domain label the way [login with device](./keys.md#login-with-device) derives its six digits. That value is the only thing the two users have to check when they pair, because Noise XX authenticates nobody. A device in the middle completes two handshakes and the two screens then disagree. It is six decimal digits, as login with device shows, under its own label so that one ritual's value cannot stand in for the other's.

The pubkey each side proves is announced to the shell as it is proved, which is how the view knows who is in range.

If neither party drops the connection, the dialer identifies itself first via [NIP 42 AUTH](./nips/p2p-auth.md#mutual-authentication). This gives the receiver the chance to drop the connection without identifying itself.

If the receiver wishes to continue, it then identifies itself to the dialer, which can choose to drop the connection as well based on the disclosed nostr identity. If neither peer drops, they enter SYNCING state.

### Meeting somebody

The gate decides whether a link proceeds, and naming decides whether the person on it becomes a contact. A stranger the gate admits syncs without anybody being asked, and has no name on this device. Every [policy](./policy.md) the user can set about a person starts from naming them.

**A pairing request is made by a person, not noticed by a phone.** Pressing add contact sends a request over every link to somebody the user has not named, as a control frame carrying nothing, because the link already says who is asking. The other phone puts the request in front of its user with the link's six digits and a name field, and the first phone waits. Opening the request sends one back over that link, so that the first phone shows the same digits while the second user compares them. Either user names the other once the digits match, which writes their [contact card](./policy.md#social-graph) for the pubkey the session proved. Declining leaves the person unnamed and not asked about again until they have left and come back.

A request from a contact the phone recognized is nothing new and is not shown. One from a contact the phone did not recognize is somebody whose phone has forgotten the user, and is shown under the name the user already gave them. Somebody who has not been asked cannot be named: there is no screen for a stranger in range, only a count of phones in range, because being in the room is not an introduction.

## Heartbeat and teardown

The heartbeat is **liveness, not authorization**. The heartbeat's only job is cleanup, because proximity is already guaranteed by transport configuration (see [`transport.md`](./transport.md)), and it can afford to be lenient.

Interval: 15–30 s when connected, jittered. A 60 s timeout is 2–4 missed beacons, matching bitchat's reachability window.

| Condition | Action |
| --- | --- |
| BLE link healthy | Nothing. |
| Heartbeat missed, session idle | After 60 s → DRAINING → CLOSED. |
| Heartbeat missed, transfer in flight | DRAINING: accept no new work, let in-flight transfers finish. Hard cap 5 min. Do not kill a working transfer over two missed beacons — radio contention during bulk transfer and iOS background throttling both cause them. |
| Clean BLE disconnect event | Immediate close. There is no drain to wait out, because disconnects are reliable when they fire and nothing in flight can finish on a dead link. The timeout is for the ambiguous case. |
