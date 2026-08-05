# Transport

One transport: BLE. The seam for a second one stays open — see [Adding a transport later](#adding-a-transport-later).

## BLE

### Link layer

Dual role: every device runs a GATT peripheral and a GATT central simultaneously. One primary service, one characteristic supporting `notify`, `write`, and `writeWithoutResponse`. Discovery is `scanForPeripherals(withServices:)` against our service UUID — see [`discovery.md`](./discovery.md).

iOS requires both `bluetooth-central` and `bluetooth-peripheral` background modes, plus CoreBluetooth **state restoration** in both roles. Restoration is not optional for an app the system will kill and relaunch; `willRestoreState` is implemented on both managers, following bitchat.

### Channel security

Noise XX — Curve25519 / ChaCha20-Poly1305 / SHA-256 — giving mutual authentication of static keys and forward secrecy for the live session. The handshake and the transport state run in the core on `snow`; the shell only moves bytes on and off the characteristic.

Events are mostly public, so content confidentiality matters less than metadata. Without channel encryption a passive listener learns which event ids two people are reconciling, which leaks the social graph directly.

### Framing

Our own, directly over GATT. The codec is core-side — pure byte manipulation that has to agree exactly with a peer running the other platform's build — while the writes, the MTU, and the ATT queue belong to CoreBluetooth and `android.bluetooth`:

- **Multiplexed.** A channel id in the frame header lets control traffic, event sync, and blob transfer share the one link.
- **Priority-scheduled.** The ATT queue is per-connection, so separate characteristics would not give QoS isolation. The sender interleaves instead: control frames pre-empt bulk fragments, which keeps the heartbeat alive during a media transfer.
- **Fragmented.** Chunked to `maximumWriteValueLength(for:)` minus header — roughly 500 bytes usable at a 512-byte MTU, often less.
- **Reliable.** Acknowledged ATT writes give ordered reliable delivery on the control and sync channels. The blob channel uses `writeWithoutResponse` with application-level acking and pacing, at 25–30 ms between fragments to avoid loss.
- **Resumable.** Blob transfers survive disconnection and resume by offset. See [`media.md`](./media.md).

### Throughput

5–15 KB/s measured, rather than theoretical PHY rates. This is the number that drives media tiering and the reconciliation strategy.

## What fits in an encounter

At 5–15 KB/s an encounter is measured by what fits in it:

- **Event sync fits.** A one-shot GCS filter is 4–8 KB and covers thousands of events, and events themselves are a few hundred bytes. Someone walking past for eight seconds still exchanges useful data — [`sync.md`](./sync.md#one-shot-first-negentropy-if-the-session-lasts).
- **Previews fit.** A 32 KB tier-1 preview is 3–7 s.
- **Originals do not.** A 2 MB photo is 3–7 minutes of link time. Tier 2 is never fetched automatically and may accumulate across several encounters — [`media.md`](./media.md#three-tiers).

### The L2CAP bandwidth upgrade

More bandwidth stays inside Bluetooth: an **L2CAP connection-oriented channel** — `CBL2CAPChannel` on iOS 11+, `createL2capChannel` / `listenUsingL2capChannel` on Android 10+. It is a stream over the same radio, the same connection and the same Noise session, so it needs no scheme, no discovery mechanism, no pairing story and none of the machinery in [Adding a transport later](#adding-a-transport-later). Range is still range, so I1 is untouched.

It is not free. Real throughput has to be measured on device rather than taken from spec numbers, Android's implementation has device-specific history, and it raises the Android floor to API 29. What makes it cheap next to a second network stack is that the channel carries frames the core already produces, under a session the core already holds open. `iroh-ble-transport` reaches for L2CAP-with-GATT-fallback for the same reason; we passed on that library over its licence, not its technique.

**It moves byte budgets, not round trips.** Round trips are set by the BLE connection interval, which an L2CAP channel shares, so the reconciliation strategy is unchanged — a drive-by still ends before a multi-round negotiation converges ([`sync.md`](./sync.md#one-shot-first-negentropy-if-the-session-lasts)). What grows is every budget that is a byte count: the GCS filter, the chunk one Merkle tree and one grant cover, how much of a negentropy exchange completes before the link drops. Control frames stay on GATT so the heartbeat keeps defining session lifetime ([`discovery.md`](./discovery.md#session-lifecycle)); bulk `EVENT` streams and blob fragments move across. Blob transfer is what justifies it, and event sync is a beneficiary.

### Wi-Fi has no cross-platform path

iOS has no Wi-Fi Direct — peer-to-peer Wi-Fi there means AWDL via Network.framework or MultipeerConnectivity. Android has Wi-Fi Aware, and the pairing story between the two is bad.

That leaves peers who happen to share an access point, which is not the common case for two people who have just walked past each other. A LAN transport optimizes the encounter that was already easy.

## Adding a transport later

I2 makes anything above the floor an optimization. These rules hold whatever arrives:

- **BLE is the control plane, always.** A second transport is a data plane only. Session state, heartbeat and teardown stay on BLE, so a session's lifetime is defined by radio proximity rather than by what the fast link believes — [`discovery.md`](./discovery.md#session-lifecycle).
- **Reachability information is exchanged over the authenticated BLE channel, or not at all.** Whatever a future transport needs in order to dial — an address, an endpoint id, a token — is handed over *after* the Noise handshake and mutual auth, carries a monotonic generation counter so stale entries are detectable, and is never obtained from a discovery service. This is the mechanism that makes I1 structural.
- **Transports are URL schemes.** Peers are `ble://…`; a second transport is a second scheme resolved by the same `getAdapter` override, speaking the same relay protocol over the same session — [`sync.md`](./sync.md#client-half). Adding a transport is registering a scheme, not integrating a protocol.
- **Blob transfer stays offset-resumable and content-addressed** — [`media.md`](./media.md#transfer).

A transport that cannot satisfy the second rule — one needing a discovery service, a rendezvous server or a relay to reach its peer — is not a candidate, whatever its throughput.

## What we are not using

**iroh.** A QUIC endpoint over the LAN, for the bandwidth upgrade. Rejected for three reasons that compound:

- **Its window is small.** It only helps when both devices sit on the same access point. Two people who pass on the street, or share a room on cellular, share no LAN.
- **It cannot serve the main case.** The core loop is two backgrounded phones in two pockets, where execution is short CoreBluetooth wakes rather than sustained runtime ([`storage.md`](./storage.md#background-execution)). A bulk QUIC transfer is therefore a foreground-to-foreground feature.
- **It works against I1.** iroh exists to connect peers who are *not* co-present — pkarr, Mainline DHT, mDNS, relays — so using it means turning all of that off and keeping it off across every version bump. Leaving it out converts I1 from a configuration property into an absence: there is no code that could reach a distant peer, so there is none to audit or to regress.

**`iroh-ble-transport`.** AGPL-3.0 with a commercial option. This app ships to app stores, where GPL-family licensing has a long history of conflict with store terms, and resolving that would require legal review. Own framing also removes the 1200-byte minimum datagram requirement and the L2CAP-with-GATT-fallback complexity it forces.

bitchat remains available as a reference — it is public domain.

**QUIC over BLE**, for the same reasons. The application protocol is identical whatever the framing, so a transport swap never reaches the app layer — see [`sync.md`](./sync.md).
