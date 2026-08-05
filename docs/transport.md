# Transport

One transport: BLE.

## BLE

### Link layer

Dual role: every device runs a GATT peripheral and a GATT central simultaneously. One primary service, one characteristic supporting `notify`, `write`, and `writeWithoutResponse`. Discovery is `scanForPeripherals(withServices:)` against our service UUID — see [`discovery.md`](./discovery.md).

iOS requires both `bluetooth-central` and `bluetooth-peripheral` background modes, plus CoreBluetooth **state restoration** in both roles. Restoration is not optional for an app the system will kill and relaunch; `willRestoreState` is implemented on both managers, following bitchat.

### Channel security

Noise XX — Curve25519 / ChaCha20-Poly1305 / SHA-256 — giving mutual authentication of static keys and forward secrecy for the live session. The handshake and the transport state run in the core on `snow`; the shell only moves bytes on and off the characteristic.

The **Noise static key** is Curve25519, long-term and per-install. It authenticates the channel and nothing else — it is not the nostr identity ([`keys.md`](./keys.md#the-nostr-identity)), and the two are bound only for the life of a session, by mutual NIP-42 ([`sync.md`](./sync.md#authentication)). Keeping them distinct is what makes that binding worth anything.

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

More bandwidth stays inside Bluetooth: an **L2CAP connection-oriented channel** — `CBL2CAPChannel` on iOS 11+, `createL2capChannel` / `listenUsingL2capChannel` on Android 10+. It is a stream over the same radio, the same connection and the same Noise session, so it needs no scheme, no discovery mechanism and no pairing story. Range is still range, so I1 is untouched.

It is not free. Real throughput has to be measured on device rather than taken from spec numbers, Android's implementation has device-specific history, and it raises the Android floor to API 29. What makes it cheap next to a second network stack is that the channel carries frames the core already produces, under a session the core already holds open. `iroh-ble-transport` reaches for L2CAP-with-GATT-fallback for the same reason; we passed on that library because it is AGPL-3.0 and this app ships to app stores, not because of its technique.

**It moves byte budgets, not round trips.** Round trips are set by the BLE connection interval, which an L2CAP channel shares, so the reconciliation strategy is unchanged — a drive-by still ends before a multi-round negotiation converges ([`sync.md`](./sync.md#one-shot-first-negentropy-if-the-session-lasts)). What grows is every budget that is a byte count: the GCS filter, the chunk one Merkle tree and one grant cover, how much of a negentropy exchange completes before the link drops. Control frames stay on GATT so the heartbeat keeps defining session lifetime ([`discovery.md`](./discovery.md#session-lifecycle)); bulk `EVENT` streams and blob fragments move across. Blob transfer is what justifies it, and event sync is a beneficiary.

### Wi-Fi has no cross-platform path

iOS has no Wi-Fi Direct — peer-to-peer Wi-Fi there means AWDL via Network.framework or MultipeerConnectivity. Android has Wi-Fi Aware, and the pairing story between the two is bad.

That leaves peers who happen to share an access point, which is not the common case for two people who have just walked past each other. A LAN transport optimizes the encounter that was already easy.
