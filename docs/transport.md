# Transport

One transport: BLE.

## BLE

### Link layer

Dual role: every device runs a GATT peripheral and a GATT central simultaneously. One primary service, one characteristic supporting `notify`, `write`, and `writeWithoutResponse`. Discovery is `scanForPeripherals(withServices:)` against our service UUID — see [`discovery.md`](./discovery.md).

iOS requires both `bluetooth-central` and `bluetooth-peripheral` background modes, plus CoreBluetooth **state restoration** in both roles. Restoration is not optional for an app the system will kill and relaunch; `willRestoreState` is implemented on both managers, following bitchat.

### Channel security

Noise XX — Curve25519 / ChaCha20-Poly1305 / SHA-256 — giving forward secrecy for the live session and a key for each side to be named by. The handshake and the transport state run in the core on `snow`; the shell only moves bytes on and off the characteristic. This keeps both metadata and content synced between two peers confidential.

#### The static key is generated per session

**Every handshake uses a fresh Curve25519 static key.** Nothing about it persists across sessions, and no device has a long-term Noise identity.

This is forced by where the key is disclosed. Noise XX hands the responder's static key to the initiator in msg2, and the initiator's to the responder in msg3 — both before the [consent gate](./discovery.md#the-consent-gate), which sits after SECURED. There is no way to withhold it: the handshake is what produces the channel the gate is evaluated on, so an unauthenticated peer that completes it holds the key by construction. A long-term key in that position is a stable 32-byte device identifier that anything in radio range can demand at will ([`privacy.md`](./privacy.md#what-an-active-radio-attacker-learns)).

A fresh key costs a base-point multiplication, on the order of 50 µs. Nothing depends on the key being stable:

- **Recognising a paired peer** happens in the [recognition exchange](./discovery.md#recognition), inside the encrypted channel, keyed on a per-pair secret rather than on a global name.
- **Backing off a peer that never completes** is keyed on the `CBPeripheral` identifier, at BLE-address granularity ([`discovery.md`](./discovery.md#making-identification-cheap)).
- **Transport binding** requires its authority to be unique and established by the handshake, not long-lived ([`nip-p2p-auth.md`](./nip-p2p-auth.md#transport-identities)). A per-session key binds an auth event to exactly one channel.
- **Login with device** compares a short authentication string derived from the transcript ([`keys.md`](./keys.md#login-with-device)), never prior knowledge of the peer's key.

What it does give up is Noise-layer authentication. A handshake against a key nobody has seen before authenticates no one, so **SECURED means encrypted, not identified**, and a machine-in-the-middle is caught one step later by the transport binding in mutual NIP-42 — see [`privacy.md`](./privacy.md#what-a-machine-in-the-middle-can-do).

The Noise key is not the nostr identity ([`keys.md`](./keys.md#the-nostr-identity)); mutual NIP-42 binds the two for the life of one session ([`sync.md`](./sync.md#authentication)), and that binding expires with the channel it names.

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
- **Originals do not, on GATT.** A 2 MB photo is 3–7 minutes of link time. They are fetched anyway, behind the previews: transfers resume by offset, so an original accumulates across encounters instead of needing one long enough to hold it — [`media.md`](./media.md#every-referenced-blob-is-fetched). L2CAP is what turns several meetings into one.

### The L2CAP bandwidth upgrade

More bandwidth stays inside Bluetooth: an **L2CAP connection-oriented channel** — `CBL2CAPChannel` on iOS 11+, `createL2capChannel` / `listenUsingL2capChannel` on Android 10+. It is a stream over the same radio, the same connection and the same Noise session, so it needs no scheme, no discovery mechanism and no pairing story. Range is still range, so I1 is untouched.

It is not free. Real throughput has to be measured on device rather than taken from spec numbers, Android's implementation has device-specific history, and it raises the Android floor to API 29. What makes it cheap next to a second network stack is that the channel carries frames the core already produces, under a session the core already holds open. `iroh-ble-transport` reaches for L2CAP-with-GATT-fallback for the same reason; we passed on that library because it is AGPL-3.0 and this app ships to app stores, not because of its technique.

**It moves byte budgets, not round trips.** Round trips are set by the BLE connection interval, which an L2CAP channel shares, so the reconciliation strategy is unchanged — a drive-by still ends before a multi-round negotiation converges ([`sync.md`](./sync.md#one-shot-first-negentropy-if-the-session-lasts)). Control frames stay on GATT so the heartbeat keeps defining session lifetime ([`discovery.md`](./discovery.md#session-lifecycle)); bulk `EVENT` streams and blob fragments move across.

**Blob transfer is what justifies it**, being the one workload that is purely byte-bound. Every blob a stored event references is fetched ([`media.md`](./media.md#every-referenced-blob-is-fetched)), so the outstanding byte count is usually large. The channel opens once that count justifies the setup round trip, rather than on connect, which a drive-by would not recover. Where it fails to open the same wants are worked over GATT and originals take more encounters, so nothing depends on it ([I2](./overview.md#invariants)).
