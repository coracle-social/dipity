This project is a mobile application built to complement local-first community interactions, rather than detract from them.

# Philosophy

## Background

James Carey distinguished two views of communication. The transmission view understands communication as the movement of messages across space for the purpose of control. The ritual view understands communication as the maintenance of a society in time — the shared ceremony, the repeated act, the drawing of people into fellowship.

Space-binding media are light and portable, and they favor expansion, administration, and empire because they make it possible to govern distant things from a center. Time-binding media are heavy, durable, and local, and they favor continuity, community, and memory, because they make it possible for a place to persist as itself.

General-purpose digital networks tend to be aggressively space-binding. The marginal cost of moving a message one meter and ten thousand kilometers is identical, and the economics built on that fact reward the annihilation of distance and the aggregation of attention at a center. A culture that relies too heavily on a space-binding medium suffers from extraction by third parties, is displaced by its representation, its context collapses, and its activity is measured.

This project is time-biased. It treats distance as something to articulate rather than eliminate.

## Principles

1. Digital localism is structural, not based on policy. Exclusive use of physical media enforces proximity. Limited device storage enforces ephemerality. Reach is a function of trust, which means amplification reflects communal assent. Cryptography governs verifiability of public speech and protects confidential speech from middlemen.
2. Admission is embodied. New identities enter only through in-person introduction. This is the anti-Sybil mechanism, the trust mechanism, and the pacing mechanism at once.
3. Forgetting is the default. If someone does not actively participate in the community, they fall out of it. If content is not repeatedly invoked, it disappears. Reach is a function of communal value, expressed through repeated propagation.
4. Private speech is deniable; public speech is attributable. Separate registers, separate cryptographic treatment, and a difference the interface makes obvious.
5. The wire carries mass; the mesh carries meaning. Internet transport is used exclusively for emergencies which require space-binding transmission, and for propagation of large files.

# Architecture

## Topology

The substrate is short-range radio between phones — Bluetooth Low Energy as the universal floor, with opportunistic upgrades to higher-bandwidth local links where the operating systems allow. Devices discover each other when near, exchange what they are carrying, and part. Messages propagate by store-carry-forward: a device holds a bundle, moves with its owner, and hands it on at the next encounter.

The network graph is therefore the social and physical geography of the place - no separate representation of location is needed. Information travels at the speed of people, along the paths people take, reaching the people they encounter.

### Identity

Identity is four distinct things.

**Recognition** answers *is this the same person I met before?* It is a long-lived key held on the device and disclosed only to people who have met the holder. It is never broadcast in the clear, because a stable public identifier on the radio turns the mesh into a tracking network for anyone with a receiver.

**Reachability** answers *how do packets find you?* It is a rotating, short-lived pseudonym, meaningless to anyone without a shared secret, changing often enough that a passive observer cannot follow a device around town. Its separation from recognition is the whole defense of the community's metadata.

**Introduction** answers *who says this person belongs?* It is an attestation, signed by an existing member, produced during a face-to-face meeting.

**Authorship** answers *who said this?* — and the answer varies by register, because the right treatment for private speech is not the right treatment for public speech.

Splitting these apart is what allows deniability and verifiability to coexist, and continuity of relationship to coexist with metadata confidentiality.

### Introductions

A community is built up from edges between members. To connect two members, an introduction must be happen:

- The app prompts the user if another user is physically nearby.
- Either user initiates the introduction ceremony.
- The two devices confirm they are physically co-located.
- Each user enters the other person's petname.

When events by a user A are shared directly with B, B's petname for A is used. When B shares A's events with C, and C hasn't set a petname for A, B's petname (via B) is used (if multiple petnames exist, rank by social trust). Petnames must be sent first when gossiping events, and must be retained until all events by that author are dropped.

### Reputation

Trust accumulates as a member is integrated into a social circle. However, quantifying that trust creates a system which can be gamed, and which draws attention to the wrong things: synthetic status over organic community relationships.

In this system, reputation is not quantified, but falls out as a result of the network topology. A person's speech is replicated through the social graph depending on who chooses to replicate it. It is the aggregate of these trust decisions that cement a person's standing in a community, and which allow that status to change over time.

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
