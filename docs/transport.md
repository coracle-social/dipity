# Transport

One transport: BLE. Discovery, session establishment, sync and blobs all happen there or not at all, and L2CAP is a bandwidth upgrade within a session BLE already established rather than a second way to reach anyone.

What goes on the air is written up for other implementations as NIP-BE, a draft in [coracle-social/nips](https://github.com/coracle-social/nips/blob/nip-be-dipity/BE.md). This document is why the wire is the way it is; NIP-BE is what another implementation has to match.

## BLE

### Link layer

Dual role: every device runs a GATT peripheral and a GATT central simultaneously. There is one primary service, with one characteristic declaring `write` and `notify`. Discovery is `scanForPeripherals(withServices:)` against our service UUID — see [`discovery.md`](./discovery.md).

Both UUIDs are random, chosen once, and live in `core/dip/src/transport/gatt.rs` rather than in Swift and Kotlin: an iPhone and an Android phone have to name the same service, and two hard-coded copies is one place to disagree. Changing either is a protocol break — a device on the old one is invisible to a device on the new.

A link begins when the dialer subscribes to notifications, which it does before its first write. CoreBluetooth has no call that disconnects a central, so a receiver closes a link by refusing every later write on it with an ATT error, and the dialer closes the link when a write is refused. The receiver refuses a write from any central it has no link with in the same way, which includes a central that subscribed before a Bluetooth relaunch and whose link did not survive it. The characteristic declares only `write` and `notify`, because the dialer sends nothing else. The Android shell drops a write without response, which has no response to carry a refusal, and CoreBluetooth cannot tell one from a write request.

iOS requires both `bluetooth-central` and `bluetooth-peripheral` background modes, plus CoreBluetooth **state restoration** in both roles. Restoration is not optional for an app the system will kill and relaunch; `willRestoreState` is implemented on both managers, following bitchat.

### Channel security

Noise XX — Curve25519 / ChaCha20-Poly1305 / SHA-256 — giving forward secrecy for the live session and a key for each side to be named by. The handshake and the transport state run in the core on `snow`; the shell only moves bytes on and off the characteristic. This keeps both metadata and content synced between two peers confidential.

#### The static key is generated per session

**Every handshake uses a fresh Curve25519 static key.** Nothing about it persists across sessions, and no device has a long-term Noise identity.

The Noise key is not the nostr identity; mutual NIP-42 binds the two for the life of one session.

### Framing

Our own, directly over GATT. The codec is core-side — pure byte manipulation that has to agree exactly with a peer running the other platform's build — while the writes, the MTU, and the ATT queue belong to CoreBluetooth and `android.bluetooth`:

- **Multiplexed.** A channel id in the frame header lets control traffic, event sync, and blob transfer share the one link.
- **Priority-scheduled.** Separate characteristics would not give QoS isolation, because the ATT queue is per-connection. The sender interleaves instead: control frames pre-empt bulk fragments, which keeps the heartbeat alive during a media transfer.
- **Fragmented.** A fragment is at most the ATT MTU minus 3 bytes, which is what one ATT request carries, and never more than 512 bytes, the longest value an attribute may hold. A longer write goes out as a long write, which splits one fragment across several requests, and the receiver refuses it. On iOS the dialer takes its size from `maximumWriteValueLength(for: .withoutResponse)` although it writes with response, because `.withResponse` answers 512 whatever the MTU. The header comes out of each fragment, and so does the AEAD tag once the channel is encrypted, which leaves at most 494 bytes of payload.
- **Reliable.** Ordered, reliable delivery on every channel, with one fragment in flight per pipe. The dialer writes with response, and a write is in flight until the receiver acknowledges it. The receiver notifies, and a notification is in flight until the platform accepts it for sending: until `updateValue` returns true on iOS, and until `onNotificationSent` on Android. CoreBluetooth reports nothing after it accepts a notification, so an iOS receiver releases the next fragment as soon as the queue takes the last one. A notification the full queue refuses is retried from `peripheralManagerIsReady`. Blob fragments travel the same way and differ only in yielding to the other channels, and in moving to L2CAP where a channel opens.
- **Resumable.** Blob transfers survive disconnection and resume by group, each of which is verified as it arrives. See [`sync.md`](./sync.md#blob-sync).

#### The wire format

Every write is one fragment: a two-byte header, then the payload. Once the handshake completes, the two are sealed together, so the radio can neither read which channel a fragment is on nor rewrite it.

| Byte | Is |
| --- | --- |
| 0 | Channel: `0` control, `1` sync, `2` blob |
| 1 | Flags. Bit 0 set means more fragments follow for this frame; the rest are reserved, must be zero, and a fragment that sets one is refused |
| 2.. | Payload |

A frame is the concatenation of its fragments' payloads. Reassembly is per channel, so that an interleaved control frame does not disturb a blob transfer mid-frame. It is also per pipe, because GATT and an [L2CAP channel](#the-l2cap-bandwidth-upgrade) deliver independently. A frame whose fragments exceed 1 MiB is refused rather than buffered, and the link is dropped. A fragment on a channel this build does not know is opened and discarded, because it spent a nonce, so that a later version can add a channel without cutting off this one. A reserved flag still ends the link, because it may change how the fragment reassembles; a later version sets one only once a control frame has shown the peer understands it.

No frame is empty. The heartbeat is a control frame carrying only [its discriminant](#the-control-channels-header).

#### The control channel's header

Channel 0 carries several kinds of application frame, distinguished by a payload byte naming which:

| Byte | Frame | Payload |
| --- | --- | --- |
| `0x01` | Recognition tags | The [tag list](./discovery.md#recognition) |
| `0x02` | Mutual `AUTH` | `["AUTH", <challenge>]`, `["AUTH", <event>]`, or the `["OK", …]` that accepts an event, exactly as NIP-42 writes them |
| `0x03` | Heartbeat | Nothing |
| `0x04` | [L2CAP upgrade](#the-l2cap-bandwidth-upgrade) | A step byte — `0x01` request, `0x02` published, `0x03` unavailable — and, for a publication, the PSM as two big-endian bytes |
| `0x05` | [Pair request](./discovery.md#meeting-somebody) | Nothing. The link already says who is asking, and a request before the link syncs is no request |
| `0x80` | [Identity transfer](./keys.md#login-with-device) | A step byte — `0x01` offer, `0x02` accept, `0x03` key, `0x04` decline — and, for the key, the 32 secret bytes |

A byte this device does not know is skipped and the link carries on, so that a later version can add a frame without cutting off the one before it. `0x80` and above are left for applications, which is where the identity transfer sits, since no other implementation has a use for it.

Both directions of `AUTH` share one byte, because the message names itself. Recognition and `AUTH` share the channel, so the tags always arrive ahead of the challenge that follows them. The handshake travels on channel 0 with no byte: its frames are the only ones to arrive before the channel is encrypted, and they are raw Noise messages read by the peer's handshake state rather than by anything that dispatches on a byte. Channel 1 needs no byte either, since it carries NIP-01 arrays, which name themselves in their first element. Channel 2's [binary messages](./sync.md#blob-sync) open with a kind byte of their own.

**A fragment is sealed as it leaves the queue, not as it is queued.** Each pipe has its own nonce sequence, GATT on the even nonces and an L2CAP channel on the odd ones, because the two deliver independently and a shared sequence would fail to open whichever write overtook the other. A sequence keeps no window, while the scheduler lets a control frame overtake queued bulk; sealing at enqueue would hand the peer ciphertext in an order it cannot open. The handshake is the exception: its messages are queued in the clear and stay that way even though the sender's own session may already have finished, because the peer must read them with its handshake state.

### Throughput

| Path | Throughput |
| --- | --- |
| GATT | 5–15 KB/s, measured |
| L2CAP | 50–150 KB/s expected, not yet measured here |

**Bandwidth is the only thing the upgrade changes.** Latency is identical, because round trips are set by the BLE connection interval, which both paths share. A drive-by still ends before a multi-round negotiation converges.

### The L2CAP bandwidth upgrade

An **L2CAP connection-oriented channel** is a byte stream over the same radio, the same connection and Noise session - all it does is increase the connection's bandwidth.

What opening one takes:

- **The APIs.** iOS 11+: the peripheral calls `publishL2CAPChannel(withEncryption:)` and the central `openL2CAPChannel(_:)`, both ending at a `CBL2CAPChannel` that exposes an `inputStream` / `outputStream` pair. Android 10+: `listenUsingInsecureL2capChannel()` and `createInsecureL2capChannel(psm)`, which puts L2CAP's own floor at API 29. The Android floor is 33, set by the GATT write and notify calls: below that they take their payload through the characteristic's mutable `value` rather than as an argument.
- **Unencrypted at the link layer, deliberately.** The encrypted variants require LE Secure Connections bonding, and a bond is a durable pairing record on both devices. Confidentiality is Noise's job, and the channel is already inside a Noise session.
- **The PSM is assigned at publish time** and is not known in advance. It goes to the peer over the existing GATT channel once there is bulk to move.
- **Opened on demand**, once outstanding blob bytes justify the setup round trip, rather than on connect — a drive-by would not recover the cost.
- **Control frames stay on GATT**, so that the heartbeat keeps defining session lifetime. Blob fragments — channel 2 — move across, and nothing else does: a fragment naming another channel on the bulk pipe is a peer misbehaving and ends the link.
- **Framing is unchanged**, with one addition. The codec already fragments and multiplexes and only the chunk size moves. A bulk write carries its length ahead of it as two big-endian bytes, because L2CAP is a byte stream on both platforms. GATT needs no prefix: one ATT write is one fragment.
- **The two pipes are independent.** Each has one write in flight at a time, acknowledged separately. A slow ATT write does not stall the transfer, and vice versa.
- **The bulk MTU is the shell's to report,** and only one platform is told it. Android reads `getMaxTransmitPacketSize()` off the socket; iOS reports the size it cuts writes to instead, because CoreBluetooth negotiates the channel's MTU and exposes it nowhere. Either way it is the size of one write, length prefix included.

#### Who publishes, and how the PSM crosses

The platform decides, not whoever wants the bandwidth: the GATT peripheral publishes and the central connects. So the receiver publishes and the dialer opens, whichever of the two raised the need.

| Step | Sent by | Means |
| --- | --- | --- |
| `0x01` request | The dialer | It wants bulk and cannot publish |
| `0x02` published | The receiver | A channel is open at this PSM |
| `0x03` unavailable | Either | This end cannot publish or open a channel, or has lost one. Bulk stays on GATT for the rest of the session |

A `published` may arrive with no request behind it, because a receiver that wants bulk publishes without being asked. Every failure path is `unavailable` rather than closing the link: the upgrade is bandwidth, and a link that never gets one still syncs. A `published` too short to hold a PSM is one more failure path, and is answered with `unavailable`. A step this build does not know is skipped, so that a later version can add one without cutting off this one.

**A channel serves only the link it was published for.** Neither platform publishes a channel for one central, so any device connected to this one can open a channel at the PSM. The shell uses a channel only if the central of the link it was published for opened it, and closes any other. A channel from anybody else would be read as the peer's bulk pipe, and its first fragment would fail to open under the session's keys and close the link.

**A frame finishes on the pipe it began on.** The blob channel moves to the bulk pipe once the frame under way on GATT has gone out, because the peer reassembles each pipe on its own. Nothing orders an ATT write against a stream read at the peer, so the first fragments of the next frame can arrive over L2CAP before the last of this one arrives over GATT.

#### Giving up the upgrade

A device gives up the upgrade when it cannot publish or open a channel, when an open channel goes away, or when the peer sends `unavailable`. A device that gives up sends `unavailable` itself whenever the peer may be counting on the channel, because a channel can die at one end while the other's streams never report it.

Giving up moves bulk back to GATT. Whatever had been cut to the larger MTU is dropped, because the fragments would overrun an ATT write, and a frame half received over the channel is discarded, because the rest of it went with the channel. A blob frame under way on GATT finishes there, because the peer is still reassembling it.

**Giving up takes the channel down at this end.** The device closes any channel it has open and withdraws any it published, and a channel the shell reports open after the device gave up is closed rather than used. Otherwise a channel would sit idle for the rest of the session, and a published one would stay open to every device connected to this one. Closing or withdrawing a channel leaves the link up.

**An upgrade that fails after it was asked for starts the fetch in flight over at both ends**, from what the store already holds, whether or not a channel opened at this end. The request in flight, or the answer to it, may have gone over a channel one end opened and the other never read. Nothing would ever answer that request, and a fetch left waiting on it stalls for the session. A second `unavailable` on a link already back on GATT changes nothing.

**A session opens one channel at most.** A second channel could not restart the odd nonces, which would reuse a nonce under the session's key. It could not carry them on either, because fragments sealed for the lost channel may never have reached the peer, whose sequence opens strictly in order. Once the upgrade has failed or the channel has gone, bulk stays on GATT until the session ends, and a later `request` or `published` is answered with `unavailable`.
