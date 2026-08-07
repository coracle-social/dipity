# Blog post outline — Dip

**Title:** *Encryption that enforces localism*

**Thesis:** Every messaging app uses cryptography to collapse distance. Dip uses it to preserve distance — so a social network can be made of the people actually around you.

---

## 1. The inversion

- Phones in a crowded room, all talking to datacenters, none to each other.
- Distance is solved. What's missing is the shared context of being somewhere together.
- Complement to Signal, not a replacement: this is for the people you keep almost meeting.
- **The crypto here keeps data local instead of moving it further.**

## 2. Why it must be enforced

- Shared identity comes from repeated physical co-presence; no global feed manufactures it. Accountability and mutual understanding are cheap when you'll see the person tomorrow.
- Any system that *can* reach everyone eventually does. A local network with a global escape hatch is a global network with extra steps.
- Design test used throughout: *"what stops this from going global?"* must have a mechanical answer, never a policy one.
- Relatives: **Manyverse/SSB** (offline gossip right, bridges over pubs), **bitchat** (local mesh right, bridges over public relays). We take SSB's sync model, bitchat's transport engineering, nostr's data model, neither one's bridging.

## 3. Data that decays as it travels

*Centerpiece — most of the post's weight.*

- Naive version: a hop counter in the event. Anyone can edit a number; a bound enforced by well-behaved software is a request.
- Invert what carries the signature. **Content events have no `sig`.** The only signature is a **delivery grant** — a small signed nostr event by the author, committing to a batch of events and naming one recipient:

  ```jsonc
  {"kind": 20666, "pubkey": "<author>",
   "tags": [["root", "<merkle root>"], ["p", "<recipient>"]],
   "content": "", "sig": "<author's signature>"}
  ```

- Two jobs at once: proves the author made exactly this content, and names exactly one person allowed to hold it.
- Why a Merkle root rather than one event: an encounter hands over whatever reconciliation turned up, sometimes thousands of events. One signature per *encounter* rather than per *event* is what keeps the design usable for people whose key lives in a separate signer app — otherwise it is thousands of round trips in the eight seconds someone walks past.
- Ingest is one comparison against the NIP-42-authenticated peer: event from the sender needs a grant naming **me** (first hop); from anyone else, proof that the sender holds a grant naming *them* (second hop). Everything else drops.
- Alice → Bob → Carol → Dave: Dave needs a grant naming Carol, only Alice can mint one, and **Alice has never met Carol**. *The wall isn't a rule, it's a missing signature.*
- No honest-node assumption — the check runs on the receiver, and grants can't be forged.
- Second property: an unsigned event isn't a valid nostr event, so leaked proximity content is *rejected* by relays rather than stored. Containment needing nobody else's cooperation — unlike a tag, a convention, or a relay policy.

### The second twist: a grant is too good at its job

*This is the strongest idea in the post; give it room.*

- A grant proves authorship to whoever holds it. Hand it to the second hop and you've handed them a portable, publishable receipt: *"A wrote this."* The mechanism that bounds reach would become the thing that makes attribution permanent.
- So **a grant is never sent to anyone but the peer it names.** Bob proves he holds one, without showing it.
- Sketch the shape without the algebra: a grant's signature is a Schnorr `(R, s)`; anyone can compute the point `s·G` from public values, but only a holder knows `s`. Bob proves he knows it — OR-composed with *"or I know Carol's private key."* Alongside it, a Merkle path showing the event really is in the batch that grant covers.
- Carol reasons: *I didn't make this proof, so Bob's branch must be the real one.* She's convinced. Show it to Dave and it proves nothing, because **Carol could have produced an identical proof herself.**
- **Decay is literal, and now it's about evidence rather than data.** Carol can read the post, trust it, and say who wrote it. She cannot prove it to anyone. The knowledge propagates; the receipt does not.
- Worth naming the deliberate limit: this is deniability against *third parties*, not against Carol. She knows. She just can't make anyone else know.
- Same primitive, one line: this is a designated-verifier proof — the trick behind deniable authentication in messaging, pointed at forwarding instead.
- Costs, stated up front: no inbound interop at all. One escape hatch, author-only — they can sign their own post normally and publish it, and since ids are plain NIP-01 hashes it keeps its replies.
- Non-obvious trap worth a paragraph: the grant is **detached, not folded into the id**. Hashing the recipient in gives one post a different id per recipient, so reconciliation (which diffs id sets) never converges.

## 4. Radio: borrowed engineering, refused shortcuts

- BLE is the floor: dual GATT role, own framing (multiplexed, priority-scheduled, fragmented, resumable), Noise XX. Real throughput 5–15 KB/s — that number drives everything.
- Borrowed openly: **bitchat** for framing, scheduling, `GCSFilter` sizing; **SSB** for the offline sync model and social-distance scoping.
- One deliberate divergence: bitchat's stable peer ID makes devices passively trackable. Our advertisement carries no identity.
- Reconciliation is shaped by human behavior, not latency — someone walks past for eight seconds, so one-shot GCS on connect, negentropy only if the session survives.
- `seen_at` is a record of where you were and who you were near: set once, never transmitted. `created_at` defines the shared scope because it's the only clock both peers can compute.
- **BLE is the address-lookup service** — the only way to learn how to reach someone is to have been in radio range of them.

## 5. iroh: bandwidth, nothing else

- Photos over 10 KB/s is miserable. The obvious fix is a relay, and that fix ends the project.
- Used for one thing: high-bandwidth QUIC between peers **already co-present**.
- **Configuration is the security boundary.** No pkarr/DNS, no DHT, no mDNS, no relay URL, `AddrFilter` to private ranges. Dial only addresses received over an authenticated BLE session.
- Payoff: **there is no code path that could reach a distant peer** — none to audit, none to regress.
- Hence no nostr relays. A relay is a way for two people who've never been near each other to exchange data; adding one wouldn't extend the network, it would delete the premise. The permanent third-party record of who posted what is the second reason.
- Upgrade failure is normal, not an error — two people on cellular share no LAN, BLE carries the session.
- Honest gap: no fast Wi-Fi path without a shared access point, which bites hardest in exactly the crowds this is for.

## 6. One protocol, four backends

*For readers who build things.*

- Four places data lives — in-memory working set, native SQLite across a bridge, a peer over BLE, a peer over QUIC. Normally four integrations.
- Instead: **peers speak the nostr relay wire protocol over every hop, including webview → native.** Reusing it *removes* a protocol from the project rather than adding one.
- Show the seam — `getAdapter` is the whole trick:

  ```ts
  getAdapter: url =>
    url === SQLITE_STORAGE_URL ? new SqliteAdapter()  :
    url.startsWith('ble://')   ? new BleAdapter(url)  :
    url.startsWith('iroh://')  ? new IrohAdapter(url) : undefined,
  ```

- Callout: **they differ in trust, not in protocol.** Peers get grant checking, `AUTH`, and policy; your own store gets none.
- What falls out free: negentropy over Bluetooth, NIP-42 mutual auth over a link with no URLs, sync policy that compiles to filters. The app layer never learns Bluetooth exists.
- Plugin boundary: the webview suspends in the background, so BLE, Noise, iroh, SQLite and peer-serving are native. Native serves from a **compiled policy snapshot** — it applies policy, never computes the web of trust.
- Knowing what *not* to library-ize is half the skill: the grant lifecycle and the relay half stay in the app. Note the flip side — making grants *events* rather than bare signatures means the signing itself needs no custom code at all, and works through an external signer.

## 7. What it costs

- No inbound interop with the open network. Threads have holes at narrow scopes.
- Key custody splits the feature set: hold your own key and you get file backup and device-to-device login; delegate to a signer app or a bunker and you get neither, because both move a key the app never sees. A bunker costs more than that — signing needs the network, so a bunker user in a crowd with no signal cannot start a session at all. Offline-first is a promise about the gossip protocol, not about every way you might log in.
- The thing users get wrong: *"my posts only reach people nearby"* is **false** — proximity constrains connections, not information. The true version: **your posts reach people you meet, and people they meet.**
- Two hops bounds what the protocol carries, never what a person does with what they read.

## 8. Close

- Back to the opening room, mechanism attached: the phones are talking to each other and to nobody else.
- We know how to build cryptography that abolishes distance. Point the same tools the other way and you get a network shaped like where you actually go.
- Invite argument about the two-hop bound specifically — it's the load-bearing and most contestable choice. The sharpest objection to pre-empt: two hops is a *product* decision wearing cryptographic clothes, and a mechanism this rigid can't be tuned later without re-issuing every grant.

---

## Drafting notes

- **Terminology:** the mechanism is *unsigned events + grants + grant proofs*, not "signed data that decays." Decay is still the right hook, but it decays in *provability*, not in readability — the second hop reads fine and simply cannot attest. Never imply content events carry a `sig`; that absence is the whole leak-protection story.
- **Keepers:** "The wall isn't a rule, it's a missing signature." · "Configuration is the security boundary." · "They differ in trust, not in protocol." · "The knowledge propagates; the receipt does not."
- **Length:** 1800–2500 words; §3 and §6 carry it.
- **One diagram:** Alice → Bob → Carol → Dave. Solid arrow A→B labeled *grant*, dashed arrow B→C labeled *proof of grant*, hard stop after Carol. The change in line style is the whole argument.
- Manyverse is MPL-2.0 — conceptual credit only, no code.
