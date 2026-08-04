# Transport

Two transports. BLE is the floor and always works; iroh is a bandwidth upgrade for peers who are already co-present. See [`overview.md`](./overview.md).

## BLE — the floor

### Link layer

Dual role: every device runs a GATT peripheral and a GATT central simultaneously. One primary service, one characteristic supporting `notify`, `write`, and `writeWithoutResponse`. Discovery is `scanForPeripherals(withServices:)` against our service UUID — see [`discovery.md`](./discovery.md).

iOS requires both `bluetooth-central` and `bluetooth-peripheral` background modes, plus CoreBluetooth **state restoration** in both roles. Restoration is not optional for an app the system will kill and relaunch; `willRestoreState` is implemented on both managers, following bitchat.

### Channel security

Noise XX — Curve25519 / ChaCha20-Poly1305 / SHA-256 — giving mutual authentication of static keys and forward secrecy for the live session.

Events are signed and mostly public, so content confidentiality matters less than metadata. Without channel encryption a passive listener learns which event ids two people are reconciling, which leaks the social graph directly.

### Framing

Our own, directly over GATT:

- **Multiplexed.** A channel id in the frame header lets control traffic, event sync, and blob transfer share the one link.
- **Priority-scheduled.** The ATT queue is per-connection, so separate characteristics would not give QoS isolation. The sender interleaves instead: control frames pre-empt bulk fragments. This is what keeps the heartbeat alive during a media transfer.
- **Fragmented.** Chunked to `maximumWriteValueLength(for:)` minus header — roughly 500 bytes usable at a 512-byte MTU, often less.
- **Reliable.** Acknowledged ATT writes give ordered reliable delivery on the control and sync channels. The blob channel uses `writeWithoutResponse` with application-level acking and pacing, at 25–30 ms between fragments to avoid loss.
- **Resumable.** Blob transfers survive disconnection and resume by offset. See [`media.md`](./media.md).

### Throughput

5–15 KB/s in practice. This is the number that drives the media tiering and the reconciliation strategy, not theoretical PHY rates.

## iroh — the upgrade

Used for exactly one thing: a high-bandwidth link between two peers who are already co-present.

### Configuration is the security boundary

iroh is designed to defeat the constraint invariant I1 imposes, so it must be configured not to:

- **No pkarr / DNS address lookup** — do not publish, do not resolve.
- **No Mainline DHT lookup.**
- **No mDNS.** Not needed; the BLE control channel replaces it.
- **No relay URL configured.** A connection that cannot be made directly must fail rather than fall back.
- **`AddrFilter`** restricting published addresses to link-local and RFC1918 ranges.

With all discovery paths off and no relay, iroh cannot reach a peer whose address was not handed to it directly. That turns I1 from a runtime check into a configuration property, which is the entire point.

### Dial policy

Only dial an `EndpointAddr` assembled from addresses that are

1. received over an authenticated BLE session,
2. in a private or link-local range, and
3. fresher than the heartbeat timeout.

Reject everything else.

### Failure is normal

Two people in a park on cellular share no LAN. The upgrade fails and BLE carries the session. Correct behaviour, not an error to surface.

## BLE is the address-lookup service

The mechanism behind I1. After the Noise handshake and mutual auth, each peer sends over the BLE control channel:

- its iroh `EndpointId`
- its current LAN socket addresses
- a monotonic generation counter, so stale addresses are detectable

The peer dials that `EndpointAddr` directly. No discovery service is involved at any point.

Address changes — joining Wi-Fi, switching networks — are re-sent with an incremented generation. **The BLE link is the control plane for the whole session; iroh is only ever a data plane.** If BLE drops, the session is draining regardless of iroh's health.

This also sidesteps a practical problem: iroh's Swift/Kotlin FFI exposes only QUIC streams, with mDNS and custom transports still Rust-only because those APIs are unstable. None of the unshipped parts are needed here.

### Integration

Rust core wrapped with uniffi, called from the Capacitor plugin. Only iroh's endpoint and stream APIs are used.

## What we are not using

**`iroh-ble-transport`.** AGPL-3.0 with a commercial option. This app ships to app stores, where GPL-family licensing has a long history of conflict with store terms, and resolving that would require legal review. Own framing also removes iroh's 1200-byte minimum datagram requirement and the L2CAP-with-GATT-fallback complexity it forces.

bitchat remains available as a reference — it is public domain.

**QUIC over BLE**, for the same reasons. The application protocol is identical on both transports (see [`sync.md`](./sync.md)); only the framing differs.

## The capability gap

There is no fast local Wi-Fi path when peers are not on the same access point. iOS has no Wi-Fi Direct; peer-to-peer Wi-Fi means AWDL via Network.framework or MultipeerConnectivity, and no iroh transport exists for either. Android has Wi-Fi Aware but the cross-platform pairing story is bad.

This bites hardest in exactly the situations this app is for — crowds, festivals, protests, anywhere without shared infrastructure. Closing it would mean a Network.framework-backed iroh custom transport. The gap stands: peers without a shared access point run on BLE alone, which invariant I2 already requires them to tolerate.
