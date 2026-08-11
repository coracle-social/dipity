# Policy

User and app-defined settings which determine what content gets replicated where.

## Hard limits

Data always flows over a local bluetooth connection, so nothing reaches a device except through someone who was physically there. How far it goes from the author is set by [the signature](./sync.md#authorship): an unsigned event stops at the peer it was handed to, since a forwarder has nothing to show; a signed one travels as far as people carry it, bounded by each device's gossip policy rather than by a hop count.

## Discoverability

Any peer that authenticates learns the user's nostr identity, and on a proximity transport also learns that the user was physically present at a time and place. The [consent gate](./discovery.md#the-consent-gate) decides who gets that far.

Peers that have already been paired are stored against a **pair secret** derived from the authenticated session, and pass silently. They are [recognized](./discovery.md#recognition) without either side disclosing anything durable — not a pubkey, which is unavailable before authentication, and not a Noise static key, which does not survive between sessions.

Otherwise, the user has a few preferences they can set for controlling background connections:

- Cool-off window - when the app is foregrounded, it begins accepting connections. This cool-off period determines how long the app will continue accepting unknown connections after the app is backgrounded. 10 minutes by default.
- Discoverable times - times of day, in the device's local timezone, when the user is willing to be passively discoverable. Empty by default.
- Disclosure budget - the number of new pubkeys the device will disclose to per discoverable window. Bounds what a harvester camped in a busy place collects, without needing to know who anyone is; nothing else can, since a burner pubkey defeats any per-identity limit. See [the consent gate](./discovery.md#the-consent-gate).

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
- **Public** - anyone; whatever the setting covers is [signed](./sync.md#authorship) so it can travel. Visibility settings only.

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

Gossip takes one extra value, **Nothing**, which shares only the user's own content. How both compile onto the wire is in [`sync.md`](./sync.md#event-sync).

