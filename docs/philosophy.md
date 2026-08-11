## Part III — Architecture

### Bearers and topology

The substrate is short-range radio between phones — Bluetooth Low Energy as the universal floor, with opportunistic upgrades to higher-bandwidth local links where the operating systems allow. Devices discover each other when near, exchange what they are carrying, and part. Messages propagate by store-carry-forward: a device holds a bundle, moves with its owner, and hands it on at the next encounter.

The network graph is therefore the social and physical geography of the place, and no separate representation of location is needed. Information travels at the speed of people, along the paths people take, reaching the people they encounter. Two neighbors who never leave their street are close in the network; so are two people at opposite ends of the county who both attend the same weekly market, which a geometric model of distance would have missed. The topology tracks the lived structure of the community rather than a cartographic abstraction of it.

One choice delivers a great deal at once. Localism is enforced by physics. This project emerges from whose paths crossed yours. Extraction is impractical, because there is no center to scrape and no vantage point from which the whole is visible. And the cost of participating in the network is the cost of being present in the place.

Where population is sparse and distances large, dedicated long-range low-bitrate relays at fixed points — a shop, a church hall, a farm gate — extend the mesh. Such a relay is civic infrastructure with a location and an owner, like a noticeboard: it stores what passes through it for a bounded time, cannot read what it carries, and holds no authority.

### Identity, split four ways

Identity is four things, kept apart.

**Recognition** answers *is this the same person I met before?* It is a long-lived key held on the device and disclosed only to people who have met the holder. It is never broadcast in the clear, because a stable public identifier on the radio turns the mesh into a tracking network for anyone with a receiver.

**Reachability** answers *how do packets find you?* It is a rotating, short-lived pseudonym, meaningless to anyone without a shared secret, changing often enough that a passive observer cannot follow a device around town. Its separation from recognition is the whole defense of the community's metadata.

**Introduction** answers *who says this person belongs?* It is an attestation, signed by an existing member, produced during a face-to-face meeting.

**Authorship** answers *who said this?* — and the answer varies by register, because the right treatment for private speech is not the right treatment for public speech.

Splitting these apart is what allows deniability and verifiability to coexist, and continuity of relationship to coexist with metadata confidentiality.

### Introduction and the growth of trust

Joining requires an introduction from a member, in person. Two devices confirm they are physically co-located — on the radio, not merely looking at each other over a video call — and the two people perform a brief mutual confirmation that requires them to speak aloud. It takes seconds, and it cannot be automated or performed remotely.

Forging identity therefore costs presence. An adversary who wants a thousand false members must physically attend a thousand introductions, which outperforms any cryptographic anti-Sybil mechanism and falls out of the localism commitment at no additional cost.

Introductions may be witnessed. A third member present at the meeting co-signs, the attestation is stronger, and the ceremony becomes a small public act rather than a private transaction — which is what happens naturally when someone is introduced at a gathering.

Trust accumulates rather than being granted. A new member begins with limited reach: their public speech propagates only a short distance through the graph, and they can address only people they have met. Reach extends as they are met by more people and as those meetings are witnessed. There is no promotion, verification badge, or visible level, which leaves it close to how a newcomer to a town gradually becomes someone whose word travels.

### Two registers: hearth and commons

Verifiability, deniability, confidentiality, and publicity conflict only under a single mechanism for authenticating speech. Split by register, they are not in tension.

**The hearth** is interpersonal speech: conversation with a person or a small circle you have met. It is addressed, encrypted end to end, and authenticated using a shared secret rather than a signature. The recipient knows the message is authentic, because only the two parties could have produced it, and cannot demonstrate that to anyone else, because they could have produced it themselves. Hearth speech is deniable and ephemeral, because conversation in a kitchen leaves no transcript.

**The commons** is public speech: announcements, offers, notices, invitations, complaints, arguments conducted in the open. It is unaddressed, floods to whoever is in range, and is signed. Publicity implies non-repudiation, which is what standing up at a town meeting has always implied — you own what you said, and other people can quote you accurately. At five thousand members the commons is public within the town and assumed to leak beyond it, which is how a noticeboard has always worked.

**The threshold** handles speech to someone you have not met, mediated by someone who has met you both — a request for introduction, a message passed along by a mutual acquaintance. It keeps a town of five thousand navigable without a directory. It is deliberately effortful, and the intermediary can decline.

Signatures are the enemy of conversation and the condition of proclamation. Append-only signed logs, used uniformly, give perfect verifiability and perfect publicity, and therefore zero deniability and zero forgetting.

### Confidentiality

Pairwise and small-circle communication uses forward-secret end-to-end encryption keyed from the in-person introduction, so that compromising a device today neither exposes last month's conversations nor permanently exposes future ones.

Circle communication — a working group, a street, a congregation, a band — uses a group key rotated on membership change. Scalable group messaging protocols generally assume a delivery service providing consistent ordering, which a partitioned mesh cannot supply, so circles use epoch-based group keys distributed pairwise. Any member can leak the contents of a circle, as in any group of humans, and the social layer handles it better than cryptography would.

Commons content is encrypted in transit, so it is opaque on the radio to anyone outside the community, and readable by every member.

Metadata is the harder problem, because radio is broadcast and anyone nearby can log presence. Rotating reachability identifiers, uniform padded bundle sizes, and a steady transmission cadence that does not reveal when someone is talking defeat casual observation and commercial data collection completely. They do not defeat a well-resourced adversary with directional antennas and time.

### Persistence, decay, and carriage

Nothing is archived. Every item carries a limited lifetime in elapsed time and in hops, and local storage is a bounded buffer whose oldest contents fall out as it fills. A device away from the community for a month returns having forgotten most of what it held.

Persistence requires re-carrying. An item survives because people keep choosing to carry it, and its reach is proportional to how many found it worth repeating. Something important to the community is carried by many people and persists for weeks; something inflammatory but hollow is carried briefly by a few and dies, because carrying it costs them something and endorses it. Anti-virality comes from rate-limiting by human attention and human movement rather than acceleration by an algorithm.

Carriage splits along a line that mirrors existing social norms. Sealed addressed traffic — hearth messages between other people — is ferried automatically and blindly. It cannot be read, its recipient is unknown, and carrying it therefore costs nothing socially; it is a postal duty and stays invisible. Commons content, which can be read, requires a deliberate choice to carry. Anyone's sealed mail gets ferried; only gossip worth standing behind gets repeated. Amplification is thereby identical to endorsement, and endorsement to a small personal cost.

Ordering is causal rather than chronological. A partitioned mesh has no trustworthy shared clock, and strict reverse-chronological ordering is the feed, which is the space-binding form par excellence.

Community-scope keys roll on a slow cycle tied to the calendar of the place — a season, a festival, a harvest. Clean revocation is a byproduct; the purpose is a recurring collective moment in which membership is renewed rather than persisting by inertia.

### The bridge

Low-bitrate radio cannot move a fifty-megabyte recording of the fiddle session in the back room of the pub, and a community's memory of itself includes recordings, photographs of the flood, a scan of the old map. Internet transport is available for these under one rule:

> Discovery, addressing, and authorization originate in the mesh. The internet moves only bytes that are already content-addressed and already authorized by something that travelled hand to hand.

A commons item announces that a recording exists, gives its content address, and constitutes the capability to fetch it. That announcement propagated by radio, person to person, at walking speed. The bytes arrive over whatever bearer is fastest — a direct local link, or a peer-to-peer content-addressed transfer over the internet. The content address authenticates the transfer, so the bearer need not be trusted, and encryption leaves relays with nothing.

Three constraints keep the bridge from becoming a hole in the floor. The content address never leaves the mesh in the clear, since knowing it constitutes permission to fetch; no index is published, no outside lookup can enumerate the community's objects, and no persistent internet identity outlives a transfer. The bridge applies only to large opaque objects above a size threshold, so text and voice notes never leave the mesh regardless of convenience.

---

## Part IV — Scale: a town, not a village

At a hundred and fifty people, everyone can meet everyone and none of the hard problems arise. Five thousand is a town, with neighborhoods, institutions, and strangers who are nonetheless not outsiders.

Trust becomes transitive and attenuating. Since nobody has met everyone, trust flows along paths, decaying sharply with distance: a person two hops away is meaningfully vouched for, a person five hops away is barely distinguishable from a stranger. Transitivity reveals only that a path exists and roughly how strong it is, never who is on it, which preserves the privacy of relationships and keeps the vouching structure from becoming a public reputation database.

Admission requires a quorum. One vouch suffices in a village and opens a door in a town, so full reach requires several independent introductions, ideally from people who are not close to each other in the graph. This happens naturally over a newcomer's first weeks, and it means a flood of false identities requires corrupting many separate parts of the community rather than one.

Circles are the primary unit and the commons is comparatively quiet. Most traffic in a town of five thousand belongs to a street, a trade, a congregation, a school year, a band, and circles are the default container for both conversation and confidentiality. Posting to the town-wide commons feels like a slightly larger act than posting to a circle.

Physical partition is normal at this scale. The town has quarters that rarely mix, and a bundle may take days to cross between them. Partition is an accurate representation of the community's real structure rather than a failure mode, and healing it with internet relays would eliminate the property the system exists to create. Information crosses between quarters because a person did.

---

## Part V — The application

**There is no feed.** The home surface is *Around*: an ambient, ephemeral view of what is currently within social and physical reach. It changes as you move through the town and as you encounter people. Pull-to-refresh does nothing; the way to see something new is to go somewhere.

**Meeting** is a ceremony rather than a settings flow. Two people, physically together, register a meeting by displaying and scanning a QR code, resulting in an artifact that can be witnessed by those nearby to lend additional credibility to the meeting.

**The Satchel** shows what you are carrying for other people. Sealed mail appears only as an aggregate — a weight, a count, no detail — because you cannot read it and it costs you nothing. Commons items you have chosen to carry appear individually, with the time they have left. The satchel has bounded capacity: picking something up when it is full means setting something down. Choosing what the community keeps alive is an ordinary daily gesture.

**The Hearth** holds conversations with individuals and circles. Voice notes are first-class and often the default, since the culture being complemented is oral and low bitrate suits speech. Messages fade on a schedule. There are no read receipts, typing indicators, or delivery guarantees — a message arrives when someone carries it, and the interface says so rather than simulating the instantaneous.

**The Commons** is the noticeboard, organized by the functions of a place rather than by content type: things happening, offered, needed, lost and found, announced, disputed. Every post carries its author's name visibly, and the composition screen makes plain that this is public speech attributed permanently. Posts wear out and disappear unless carried.

**Decay is visible.** Items show their remaining life through fading, thinning, wearing. Anything approaching expiry can be renewed by someone nearby who chooses to carry it, and the interface surfaces what is about to be lost, so keeping something alive is a small communal act people notice themselves performing.

**There is no amplification control.** No like button, reshare, reaction, or comment count. Reach comes from the satchel, and its cost is the space it takes.

**There is no search across the community.** You can search your own satchel and your own conversations, not the town. Wanting to know something you do not have means asking, which routes your question to people and returns an answer from a person.

**Profiles are almost empty.** A person is a name they chose, an image they chose, the fact that you met them, roughly when and where, and who witnessed. No bio, no history, no archive of past speech, no counts. There is nothing to browse and therefore nothing to compare.

**Ostracism is local, private, and revocable.** You can stop carrying a person's traffic. The decision is yours, is never shown to anyone including them, and can be undone. When enough people independently make it, the person falls out of the topology. There is no report button, because there is no moderator; the escalation path is to raise it at the commons, as public signed speech with your name on it, so complaints cost the complainant something.

**Time is described, not stamped.** This morning, a few days ago, last month. Wall-clock precision is unavailable in a partitioned mesh and undesirable in a medium that works like memory.

**Network presence reads as weather.** Some quiet indication of how alive the mesh is nearby — many people, or few, or none since yesterday — without ever showing a node graph.

**Onboarding is a wall.** Installing the app and using it are different things. First run says: find someone who is already part of this and stand next to them.

**The alarm register** exists for fire, flood, a missing child. It propagates as fast as every available bearer allows, including the internet, ignoring the ordinary constraints. It is visually unmistakable, signed, attributable, and its use is visible to the whole community.

---

## Part VI — Limits

**The bridge is under constant pressure.** Every constraint here is eventually experienced as friction by someone with a legitimate need, and each individual expansion of the bridge is defensible while the sum of them is a normal messaging app. Holding the line is an organizational problem as much as a technical one.

**Below a threshold of physical density, this project is useless**, and the failure reinforces itself, since nobody stays where nobody is. It spreads one place at a time, seeded at a real gathering among people who already have reasons to talk to each other.

**Mobile background radio is severely constrained**, particularly on one of the two major platforms, where background advertising is degraded, background scanning is restricted, and peer-to-peer high-bandwidth links are unavailable.

**Carriage concentrates.** A small number of highly mobile, highly social people carry most of the traffic. Every village has a postman and a gossip, so this is not automatically pathological, but it creates both a centralization risk and a surveillance target. Bounded satchel capacity limits it structurally.

**Ostracism can become a mob.** The mechanism that lets a community protect itself lets it exile someone unjustly, with no appeal process. Keeping the decision individual, private, revocable, and slow to take effect is a real mitigation and an incomplete one.

**Identity loss has no appeal.** Social recovery — a handful of people you have met reconstituting your identity, in person — fits the medium and is worse UX than a password reset. Some people lose their identity and are reintroduced as newcomers. However, identities can be backed up and copied from one device to another.

**Sybil resistance rests entirely on the co-location check**, which has to defeat an attacker relaying an introduction between two remote locations. It is the load-bearing joint of the trust model and the one place where the mechanism has to be specified precisely.

---

## Part VII — What this project does not do

It does not deliver reliably or promptly. It does not support remote participation. It keeps no searchable archive, offers no discoverability, and has no growth objective. It produces no evidence, since hearth speech is deniable by construction — a real cost with real victims in some situations. It does not moderate at scale, does not support any business model based on attention or data, and cannot reach everyone at once except in an emergency.

It is also not a replacement for meeting. It exists to make meeting more likely and better informed, and it works when people spend more time in each other's physical presence, not less.
