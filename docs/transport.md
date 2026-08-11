# Transport

One transport: BLE. Discovery, session establishment, sync and blobs all happen there or not at all, and L2CAP is a bandwidth upgrade within a session BLE already established rather than a second way to reach anyone.

## BLE

### Link layer

Dual role: every device runs a GATT peripheral and a GATT central simultaneously. One primary service, one characteristic supporting `notify`, `write`, and `writeWithoutResponse`. Discovery is `scanForPeripherals(withServices:)` against our service UUID — see [`discovery.md`](./discovery.md).

iOS requires both `bluetooth-central` and `bluetooth-peripheral` background modes, plus CoreBluetooth **state restoration** in both roles. Restoration is not optional for an app the system will kill and relaunch; `willRestoreState` is implemented on both managers, following bitchat.

### Channel security

Noise XX — Curve25519 / ChaCha20-Poly1305 / SHA-256 — giving forward secrecy for the live session and a key for each side to be named by. The handshake and the transport state run in the core on `snow`; the shell only moves bytes on and off the characteristic. This keeps both metadata and content synced between two peers confidential.

#### The static key is generated per session

**Every handshake uses a fresh Curve25519 static key.** Nothing about it persists across sessions, and no device has a long-term Noise identity.

The Noise key is not the nostr identity; mutual NIP-42 binds the two for the life of one session.

### Framing

Our own, directly over GATT. The codec is core-side — pure byte manipulation that has to agree exactly with a peer running the other platform's build — while the writes, the MTU, and the ATT queue belong to CoreBluetooth and `android.bluetooth`:

- **Multiplexed.** A channel id in the frame header lets control traffic, event sync, and blob transfer share the one link.
- **Priority-scheduled.** The ATT queue is per-connection, so separate characteristics would not give QoS isolation. The sender interleaves instead: control frames pre-empt bulk fragments, which keeps the heartbeat alive during a media transfer.
- **Fragmented.** Chunked to `maximumWriteValueLength(for:)` minus header — roughly 500 bytes usable at a 512-byte MTU, often less.
- **Reliable.** Acknowledged ATT writes give ordered reliable delivery on the control and sync channels. The blob channel uses `writeWithoutResponse` with application-level acking and pacing, at 25–30 ms between fragments to avoid loss.
- **Resumable.** Blob transfers survive disconnection and resume by chunk. See [`sync.md`](./sync.md#blob-sync).

### Throughput

| Path | Throughput |
| --- | --- |
| GATT | 5–15 KB/s, measured |
| L2CAP | 50–150 KB/s expected, not yet measured here |

**Bandwidth is the only thing the upgrade changes.** Round trips are set by the BLE connection interval, which both paths share, so latency is identical and a drive-by still ends before a multi-round negotiation converges.

### The L2CAP bandwidth upgrade

An **L2CAP connection-oriented channel** is a byte stream over the same radio, the same connection and Noise session - all it does is increase the connection's bandwidth.

What opening one takes:

- **The APIs.** iOS 11+: the peripheral calls `publishL2CAPChannel(withEncryption:)` and the central `openL2CAPChannel(_:)`, both ending at a `CBL2CAPChannel` that exposes an `inputStream` / `outputStream` pair. Android 10+: `listenUsingInsecureL2capChannel()` and `createInsecureL2capChannel(psm)`, which puts the Android floor at API 29.
- **Unencrypted at the link layer, deliberately.** The encrypted variants require LE Secure Connections bonding, and a bond is a durable pairing record on both devices. Confidentiality is Noise's job, and the channel is already inside a Noise session.
- **The PSM is assigned at publish time**, so it is not known in advance and goes to the peer over the existing GATT channel once there is bulk to move.
- **Opened on demand**, once outstanding blob bytes justify the setup round trip, rather than on connect — a drive-by would not recover the cost.
- **Control frames stay on GATT**, so the heartbeat keeps defining session lifetime. Bulk `EVENT` streams and blob fragments move across.
- **Framing is unchanged.** The codec already fragments and multiplexes; only the chunk size moves.
