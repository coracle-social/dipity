# Policy

User and app-defined settings which determine what content gets replicated where.

## Hard limits

Content can never be propagated more than two hops away due to how [authorship proofs](./proofs.md) and [transport](./transport.md) are structured. Data will always flow over a local bluetooth connection, and is never valid unless accompanied by a cryptographic proof of authorship.

## Discoverability

Any peer that authenticates learns the user's nostr identity, and on a proximity transport also learns that the user was physically present at a time and place. The [consent gate](./discovery.md#the-consent-gate) decides who gets that far.

Peers that have already been paired are stored against a **pair secret** derived from the authenticated session, and pass silently. They are [recognised](./discovery.md#recognition) without either side disclosing anything durable — not a pubkey, which is unavailable before authentication, and not a Noise static key, which does not survive between sessions.

Otherwise, the user has a few preferences they can set for controlling background connections:

- Cool-off window - when the app is foregrounded, it begins accepting connections. This cool-off period determines how long the app will continue accepting unknown connections after the app is backgrounded. 10 minutes by default.
- Discoverable times - times of day, in the device's local timezone, when the user is willing to be passively discoverable. Empty by default.
- Disclosure budget - the number of new pubkeys the device will disclose to per discoverable window. Bounds what a harvester camped in a busy place collects, without needing to know who anyone is; nothing else can, since a burner pubkey defeats any per-identity limit. See [the disclosure budget](./discovery.md#the-disclosure-budget).

An unknown peer is admitted if either of the first two preferences allows it and the budget has not been spent.

The cool-off window carries most of the gate: at its default, checking the app on a bus leaves the device open to that bus for ten minutes.

## Social graph

There are certain classifications that users may wish to use to tag other users:

- Trust - the user trusts this person (kind xxxxx)
- Block - the user has blocked this person (kind xxxxx)
- Mute - the user has muted this person (kind 10000)

Every setting below is expressed on the same tiers, applied either to the peer on the other end of a session or to the author of an event:

- **Trusted** - people the user explicitly trusts.
- **Network** - people the user transitively trusts, two hops out.
- **Lenient** - anyone who connects, except blocked pubkeys.
- **Public** - anyone; [proofs are generated](./proofs.md) for whatever the setting covers. Visibility settings only.

## Visibility

Who can see what the user publishes:

| Setting | Governs | Default |
| --- | --- | --- |
| Profile visibility | the user's profile | `public` |
| Content visibility | the user's content | `public` |
| Metadata visibility | the user's trust, block and mute lists | `trusted` |

## Accept and gossip

How much of other people's content the device takes in, and how much of it goes on to the next peer:

| Setting | Governs | Default |
| --- | --- | --- |
| Accept | what the device stores from a peer | `lenient` |
| Gossip | what the device relays onward | `network` |

Gossip takes one extra value, **Nothing**, which shares only the user's own content. How both compile onto the wire is in [`sync.md`](./sync.md#sync-policy).

