# Policy

User and app-defined settings which determine what content gets replicated where.

## Hard limits

Content can never be propagated more than two hops away due to how [authorship proofs](./proofs.md) and [transport](./transport.md) are structured. Data will always flow over a local bluetooth connection, and is never valid unless accompanied by a cryptographic proof of authorship.

## Discoverability

Any peer that authenticates learns the user's nostr identity, and on a proximity transport also learns that the user was physically present at a time and place. The [consent gate](./discovery.md#the-consent-gate) decides who gets that far. Peers that have already been paired are stored by **Noise static key** — not by pubkey, since the static key is the only identity available before authentication — and pass silently.

Otherwise, the user has a few preferences they can set for controlling background connections:

- Cool-off window - when the app is foregrounded, it begins accepting connections. This cool-off period determines how long the app will continue accepting unknown connections after the app is backgrounded. 10 minutes by default.
- Discoverable times - times of day, in the device's local timezone, when the user is willing to be passively discoverable. Empty by default.

An unknown peer is admitted if either preference allows it.

## Social graph

There are certain classifications that users may wish to use to tag other users:

- Trust - the user trusts this person (kind xxxxx)
- Block - the user has blocked this person (kind xxxxx)
- Mute - the user has muted this person (kind 10000)

## Profile Visibility

This setting governs who the user's profile is visible to:

- Public - user profile is visible to anyone; [signatures are generated](./proofs.md) for the user's profile.
- Lenient - user profile is visible to anyone who connects except for blocked pubkeys.
- Strict - user profile is visible only to people who the user explicitly trusts.

Default is `public`.

## Content Visibility

This setting governs who the user's content is visible to:

- Public - user content is visible to anyone; [signatures are generated](./proofs.md) for the user's content.
- Lenient - user content is visible to anyone who connects except for blocked pubkeys.
- Strict - user content is visible only to people who the user explicitly trusts.

Default is `public`.

## Metadata Visibility

This setting governs who the user's social graph information (trust, block, and mute lists) is visible to:

- Public - user metadata is visible to anyone; [signatures are generated](./proofs.md) for the user's metadata.
- Lenient - user metadata is visible to anyone who connects except for blocked pubkeys.
- Strict - user metadata is visible only to people who the user explicitly trusts.

Default is `lenient`.

## Gossip

Users may want to tune how much of others' content to relay to other peers:

- Nothing - nothing is shared with peers except the user's own content.
- Trusted - only content from people the user explicitly trusts is forwarded.
- Network - everything except content from blocked people is forwarded.

Default is `network`.
