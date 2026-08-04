# Protocol Partitioning

This NIP defines a way to partition the nostr protocol, such that events cannot be validated without knowing a priori what network partition they were signed for.

Events are assigned to a non-default partition by appending the partition name to the serialized event (as defined in NIP-01) before signing.

For example:

```
event.id = sha256([
  0,
  <pubkey>,
  <created_at>,
  <kind>,
  <tags>,
  <content>,
  <partition>
])
```

Partition is an arbitrary string, serialized the same way as `content`. Partitions may be application-specific or represent a fork of the protocol, for deliberately non-interoperable data.

Implementations MUST treat the partition as opaque and compare it byte-for-byte. There is no registry, no namespacing, and no inheritance between partitions.

The default partition is the *absence* of the field, not the empty string: a default-partition event serializes exactly as NIP-01 specifies, with no seventh element. Existing implementations are unaffected.

Existing partitions for side-protocols:

- `proximity` - intended for events that are replicated only between devices, not over the open internet or via nostr relays.
