# Transport

One transport: BLE. Discovery, session establishment, sync and blobs all happen there or not at all, and L2CAP is a bandwidth upgrade within a session BLE already established rather than a second way to reach anyone.

## BLE

### Link layer

Dual role: every device runs a GATT peripheral and a GATT central simultaneously. One primary service, one characteristic supporting `notify`, `write`, and `writeWithoutResponse`. Discovery is `scanForPeripherals(withServices:)` against our service UUID — see [`discovery.md`](./discovery.md).

Both UUIDs are random, chosen once, and live in `core/dip/src/transport/gatt.rs` rather than in Swift and Kotlin: an iPhone and an Android phone have to name the same service, and two hard-coded copies is one place to disagree. Changing either is a protocol break — a device on the old one is invisible to a device on the new.

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
- **Fragmented.** Chunked to `maximumWriteValueLength(for:)` minus the header, and minus the AEAD tag once the channel is encrypted — roughly 480 bytes usable at a 512-byte MTU, often less.
- **Reliable.** Acknowledged ATT writes give ordered reliable delivery on the control and sync channels. The blob channel uses `writeWithoutResponse` with application-level acking and pacing, at 25–30 ms between fragments to avoid loss.
- **Resumable.** Blob transfers survive disconnection and resume by group, each of which is verified as it arrives. See [`sync.md`](./sync.md#blob-sync).

#### The wire format

Every write is one fragment: a two-byte header, then the payload.

| Byte | Is |
| --- | --- |
| 0 | Channel: `0` control, `1` sync, `2` blob |
| 1 | Flags. Bit 0 set means more fragments follow for this frame; the rest are reserved, must be zero, and a fragment that sets one is refused |
| 2.. | Payload, sealed once the channel is encrypted |

A frame is the concatenation of its fragments' payloads. Reassembly is per channel, so an interleaved control frame does not disturb a blob transfer mid-frame. A frame whose fragments exceed 1 MiB is refused rather than buffered, and the link is dropped.

No frame is empty. The heartbeat is a control frame carrying only [its discriminant](#the-control-channels-header).

#### The control channel's header

Channel 0 carries several kinds of application frame, distinguished by a payload byte naming which:

| Byte | Frame | Payload |
| --- | --- | --- |
| `0x01` | Recognition tags | The [tag list](./discovery.md#recognition) |
| `0x02` | Mutual `AUTH` | `["AUTH", <challenge>]` or `["AUTH", <event>]`, exactly as NIP-42 writes them |
| `0x03` | Heartbeat | Nothing |
| `0x04` | [Identity transfer](./keys.md#login-with-device) | A step byte — `0x01` offer, `0x02` accept, `0x03` key, `0x04` decline — and, for the key, the 32 secret bytes |
| `0x05` | [L2CAP upgrade](#the-l2cap-bandwidth-upgrade) | A step byte — `0x01` request, `0x02` published, `0x03` unavailable — and, for a publication, the PSM as two big-endian bytes |

Both directions of `AUTH` share one byte, because the message names itself. The handshake travels on the same channel with no byte: its frames are the only ones to arrive before the channel is encrypted, and they are raw Noise messages read by the peer's handshake state rather than by anything that dispatches on a byte. Channels 1 and 2 need no byte either, since both carry NIP-01 arrays, which name themselves in their first element.

**The payload is sealed as it leaves the queue, not as it is queued.** Each pipe has its own nonce sequence, GATT on the even nonces and an L2CAP channel on the odd ones, because the two deliver independently and a shared sequence would fail to open whichever write overtook the other. A sequence keeps no window, while the scheduler lets a control frame overtake queued bulk; sealing at enqueue would hand the peer ciphertext in an order it cannot open. The handshake is the exception: its messages are queued in the clear and stay that way even though the sender's own session may already have finished, because the peer must read them with its handshake state.

### Throughput

| Path | Throughput |
| --- | --- |
| GATT | 5–15 KB/s, measured |
| L2CAP | 50–150 KB/s expected, not yet measured here |

**Bandwidth is the only thing the upgrade changes.** Round trips are set by the BLE connection interval, which both paths share, so latency is identical and a drive-by still ends before a multi-round negotiation converges.

### The L2CAP bandwidth upgrade

An **L2CAP connection-oriented channel** is a byte stream over the same radio, the same connection and Noise session - all it does is increase the connection's bandwidth.

What opening one takes:

- **The APIs.** iOS 11+: the peripheral calls `publishL2CAPChannel(withEncryption:)` and the central `openL2CAPChannel(_:)`, both ending at a `CBL2CAPChannel` that exposes an `inputStream` / `outputStream` pair. Android 10+: `listenUsingInsecureL2capChannel()` and `createInsecureL2capChannel(psm)`, which puts L2CAP's own floor at API 29. The Android floor is 33, set by the GATT write and notify calls: below that they take their payload through the characteristic's mutable `value` rather than as an argument.
- **Unencrypted at the link layer, deliberately.** The encrypted variants require LE Secure Connections bonding, and a bond is a durable pairing record on both devices. Confidentiality is Noise's job, and the channel is already inside a Noise session.
- **The PSM is assigned at publish time**, so it is not known in advance and goes to the peer over the existing GATT channel once there is bulk to move.
- **Opened on demand**, once outstanding blob bytes justify the setup round trip, rather than on connect — a drive-by would not recover the cost.
- **Control frames stay on GATT**, so the heartbeat keeps defining session lifetime. Blob fragments — channel 2 — move across, and nothing else does: a fragment naming another channel on the bulk pipe is a peer misbehaving and ends the link.
- **Framing is unchanged**, with one addition. The codec already fragments and multiplexes and only the chunk size moves, but L2CAP is a byte stream on both platforms, so a bulk write carries its length ahead of it as two big-endian bytes. GATT needs no prefix: one ATT write is one fragment.
- **The two pipes are independent.** Each has one write in flight at a time, acknowledged separately, so a slow ATT write does not stall the transfer and vice versa.
- **The bulk MTU is the shell's to report,** and only one platform is told it. Android reads `getMaxTransmitPacketSize()` off the socket; CoreBluetooth negotiates the channel's MTU and exposes it nowhere, so iOS reports the size it cuts writes to instead. Either way it is the size of one write, length prefix included.

#### Who publishes, and how the PSM crosses

The platform decides, not whoever wants the bandwidth: the GATT peripheral publishes and the central connects. So the receiver publishes and the dialer opens, whichever of the two raised the need.

| Step | Sent by | Means |
| --- | --- | --- |
| `0x01` request | The dialer | It wants bulk and cannot publish |
| `0x02` published | The receiver | A channel is open at this PSM |
| `0x03` unavailable | Either | This end cannot, so the link stays on GATT |

A receiver that wants bulk publishes without being asked, so a `published` may arrive with no request behind it. Every failure path is `unavailable` rather than a teardown: the upgrade is bandwidth, and a link that never gets one still syncs. If an open channel goes away, bulk returns to GATT and whatever had been cut to the larger MTU is dropped — the fragments would overrun an ATT write, and the transfer resumes from what the store already holds.
