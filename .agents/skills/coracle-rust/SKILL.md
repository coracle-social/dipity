---
name: coracle-rust
description: "Use this skill when working with coracle-lib in Rust: nostr events (the EventContent → EventTemplate → StampedEvent → OwnedEvent → HashedEvent → Event pipeline), keys (PublicKey/SecretKey, npub/nsec/ncryptsec), tags, kinds, addresses (naddr), filters and NIP-50 search, NIP-44 encryption, proof of work, expiration, protected events, and NIP-77 negentropy sync. Also covers the literate-programming layout of the coracle-rust monorepo — book/ is the source, src/ is tangled — and how to depend on it."
---

# coracle-rust — Nostr Types and Utilities for Rust

`coracle-lib` is the core crate of the [coracle-rust](https://gitea.coracle.social/coracle/coracle-rust) monorepo: nostr's data model as Rust types, plus the stateless algorithms over them. No I/O, no runtime, no relay connections — those live in `coracle-net`, `coracle-signer`, `coracle-domain`, `coracle-content` and `coracle-storage`, which are planned but not yet written.

**Reach for it instead of hand-rolling** an event struct, a filter, kind-range checks, an address, a `now()`, or a negentropy implementation. Getting any of those subtly wrong is a byte-for-byte disagreement with every other implementation on the network, and the failure is silent.

## The literate-programming layout

`book/` is the source of truth. Markdown chapters carry the real code in fenced blocks annotated `{file=coracle-lib/src/events.rs}`, and `coracle-tangle` extracts them in document order into the crates' `src/` directories.

Consequences that matter:

- **Never edit `src/`.** It is generated, gitignored, and overwritten by the next `just tangle`. A change upstream is a chapter edit.
- **`src/` is not in the repository.** Only `book/` is. A clone has no `src/` until `just tangle` runs there.
- **Read the book for the why, `src/` for the exact signature.** Code blocks with no `{file=…}` annotation are illustrative and are not compiled.

```sh
cd coracle-rust && just tangle   # generate src/ from book/
just check                       # tangle, then cargo check
```

### Depending on it

A **git dependency does not build today** — cargo clones the repo, finds no `coracle-lib/src/lib.rs`, and fails. Until tangled source ships, the options are a path dependency to a local checkout that has been tangled, or vendoring the tangled output. Put the choice in one place:

```toml
# [workspace.dependencies]
coracle-lib = { path = "../../coracle-rust/coracle-lib" }
```

**A path dependency pins nothing.** Someone re-tangling that checkout changes your build with no version, no lockfile entry, and no warning — a `cargo check` that passed an hour ago fails now. Expect it, read the upstream commit before fixing the errors (the change may delete code on your side rather than needing a shim), and treat the release as a blocker for anything you intend to ship.

## Chapters

| Chapter | Module | Covers |
| --- | --- | --- |
| 02 | `keys` | `PublicKey`, `SecretKey`, hex, npub/nsec, NIP-49 ncryptsec |
| 03 | `encryption` | NIP-44 v2: `shared_secret`, `nip44_encrypt`, `nip44_decrypt` |
| 04 | `tags` | `Tag`, `Tags` |
| 05 | `events` | The six-stage event pipeline, canonical serialization, verification, `Has*` accessors |
| 06 | `kinds` | Range classification, the `Kind` trait |
| 07 | `addresses` | `Address`, `kind:pubkey:identifier`, naddr TLV |
| 08 | `pow` | NIP-13 mining and measuring |
| 09 | `expiration` | NIP-40 |
| 10 | `protected` | NIP-70 |
| 11 | `filters` | `Filter`, `TagMatch`, matching, NIP-01 JSON |
| 12 | `search` | NIP-50 `SearchQuery` and local scoring |
| 13 | `sync` | NIP-77 negentropy, as a pure algorithm |

`util` holds what belongs to no type: `now()`.

## Events

Six structs, each adding exactly one field, so a value's type says which steps it has been through. You cannot sign what has not been hashed, or hash what has no author.

```
EventContent → EventTemplate → StampedEvent → OwnedEvent → HashedEvent → Event
   content        + kind         + created_at    + pubkey      + id        + sig
   tags
```

```rust
use coracle_lib::events::EventContent;
use coracle_lib::util::now;

let hashed = EventContent::new()
    .with_content("hello nostr")
    .with_tags(Tags::new().add("t", ["nostr"]))
    .with_kind(1)
    .with_created_at(now())
    .with_pubkey(secret.public_key())
    .with_id();

let event = hashed.clone().with_sig(secret.sign(&hashed.id));
```

Every stage setter is `with_*`. (`Filter`'s builders are `add_*` / `remove_*` / `clear_*` — a different vocabulary for a different job: a filter accumulates into sets, a stage sets one field once.)

`with_id()` is pure and knows nothing about keys; `with_sig()` takes an already-computed signature rather than doing the signing, which keeps the crypto in `keys`.

**Field types.** `id: [u8; 32]`, `pubkey: PublicKey`, `sig: [u8; 64]`, `created_at: i64`, `kind: u16`, `tags: Tags`, `content: String`. Ids and signatures are bytes in memory and hex on the wire; `Serialize`/`Deserialize` bridge the two by hand.

**Time is `i64` everywhere** — `now()`, `created_at`, `Filter::since`/`until`, `sync::Item::timestamp`. Signed, so `now() - created_at` cannot underflow, and it is SQLite's integer as it stands. A negative `created_at` is rejected on deserialize and `debug_assert`ed in `with_created_at`.

**`HashedEvent` is the unsigned event**, and it is a first-class citizen: NIP-46 hands a remote signer exactly this shape, a PoW miner produces one, and an application that never signs stores one. Deserializing a signed event as a `HashedEvent` succeeds and drops the signature.

**Verification is separate from parsing.** `Deserialize` does not check anything; `event.verify_id()` recomputes the id, and `event.verify()` checks the id and the Schnorr signature. Parsing is cheap and verification is not, so reading events off disk does not pay for it.

### The accessor traits

One trait per field — `HasId`, `HasPubkey`, `HasKind`, `HasCreatedAt`, `HasTags`, `HasContent` — implemented for `HashedEvent` and `Event`. A function bounds itself on the fields it actually reads:

```rust
fn is_from<E: HasPubkey>(event: &E, author: &PublicKey) -> bool {
    event.pubkey() == author
}
```

The library's own entry points carry the same bound rather than naming a concrete stage: `Address::from_event`, `Kind::parse` and `Kind::from_event` are all generic over the accessors they read, so an unsigned `HashedEvent` goes through every one of them.

This is also the extension-trait idiom the library uses for its own behavior, and the right way to add a method to a foreign event type in your own crate:

```rust
pub trait EventExtensionMine: HasKind + HasPubkey + HasTags {
    fn my_thing(&self) -> Option<Thing> { … }
}

impl<T: HasKind + HasPubkey + HasTags> EventExtensionMine for T {}
```

A trait's methods are callable only where the trait is in scope, so the library gathers its own in `coracle_lib::prelude` — `use coracle_lib::prelude::*` brings `address`, `is_expired`, `is_protected`, `get_pow` and the accessors. **Check the prelude before writing an extension trait**; the one you want may already be there.

## Keys

```rust
let secret = SecretKey::generate();
let public = secret.public_key();

public.to_hex();                        // wire form
public.to_npub();                       // user-facing NIP-19
"npub1…".parse::<PublicKey>()?;         // FromStr sniffs the prefix, takes either
secret.to_ncryptsec(password, 16, 0x02)?;   // NIP-49, password-encrypted
SecretKey::from_ncryptsec(s, password)?;
secret.sign(&digest);                   // BIP-340 over 32 bytes, deterministic
```

`SecretKey` has **no `Display`**, a redacted `Debug`, and no `Copy`, so the material only escapes through an explicit `to_hex` / `to_nsec` / `to_ncryptsec`. Those calls are the audit points. The inner `secp256k1::SecretKey` zeroes on drop.

`PublicKey` is `Copy`, `Ord` and `Hash` — usable as a map key or in a `BTreeSet`.

## Tags

`Tag` wraps `Vec<String>`; `Tags` wraps `Vec<Tag>`. Both serialize transparently, so the wire bytes and the canonical hash bytes are those of a bare `Vec<Vec<String>>`.

```rust
tag.name();                  // first entry, "" if empty
tag.value();                 // second entry, "" if absent
tag.values();                // everything after the name
tags.value("d");             // Option<&str>, first d tag
tags.values("p");            // impl Iterator<Item = &str>
tags.find_all("e");          // impl Iterator<Item = &Tag>
tags.has("expiration");
Tags::new().add("t", ["nostr"]).set("d", ["slug"]).remove("alt")   // chainable
```

There is deliberately **no tag taxonomy** — no `TagKind` enum, no parsed variants. An `e` tag means eight different things in eight NIPs, and resolving it requires knowing the event's kind. Ask the question you mean: `tags.values("p").any(|p| p == hex)`.

Single-letter names are the ones relays index and filters can name. Multi-character names (`imeta`, `alt`, `expiration`) travel but are not queryable.

## Kinds

```rust
kinds::is_regular(kind)      // kind != 0 && kind != 3 && kind < 10_000
kinds::is_replaceable(kind)  // 0, 3, or 10_000..20_000
kinds::is_ephemeral(kind)    // 20_000..30_000
kinds::is_addressable(kind)  // 30_000..40_000
```

Free functions, not methods, because storage layers and filter builders classify a raw `u16` with no domain type in hand. Note that a kind ≥ 40000 is none of the four.

The `Kind` trait attaches a domain type to a kind number: implement `parse` and `to_content()`, and get `from_event` (which checks the kind first), `kind()` and `to_template()`. Both entry points are generic over `HasKind + HasContent + HasTags`, so an unsigned event parses into a domain type as readily as a signed one.

## Addresses

`kind:pubkey:identifier` — the slot a replaceable event occupies, stable across versions.

```rust
Address::new(30023, pubkey, "my-article");
Address::from_event(&event);      // None for regular and ephemeral kinds
event.address();                  // the same, via the prelude's extension trait
address.to_string();              // "30023:<hex>:my-article", the `a` tag value
"30023:<hex>:slug".parse()?;      // FromStr also accepts naddr1…
address.with_hints(["wss://…"]);
address.to_naddr();
```

`from_event` is generic over `HasKind + HasPubkey + HasTags + ?Sized`, so it reads any event stage — including the unsigned `HashedEvent` an application that never signs holds.

`hints` carries relay URLs and is **excluded from `PartialEq` and `Hash`** — two addresses naming the same slot are equal whatever relays they suggest.

## Filters

```rust
pub struct Filter {
    pub ids: Option<BTreeSet<[u8; 32]>>,
    pub authors: Option<BTreeSet<PublicKey>>,
    pub kinds: Option<BTreeSet<u16>>,
    pub tags: BTreeMap<String, BTreeSet<String>>,   // keys carry the # or & prefix
    pub since: Option<i64>,
    pub until: Option<i64>,
    pub limit: Option<usize>,
    pub search: Option<String>,
}
```

**`None` means no constraint; `Some(empty set)` means membership of the empty set, and matches nothing.** It arrives through ordinary use — "notes from the people I follow" where the follow list is empty — so the answer is zero events, not all of them.

`filter.matches_nothing()` reports every unsatisfiable case in one place: an empty `ids`, `authors` or `kinds` set, an empty `#` tag constraint, or a `since` past its `until`. A storage backend should ask it before building a query, because `IN ()` is not valid SQL and an empty set has to compile to a false predicate anyway.

**The `&` prefix inverts.** Every value of an empty list is present in any event, so an empty `All` constraint *requires nothing* and matches everything — the opposite of an empty `Any`. `add_tags` drops such a constraint rather than storing it, deserialization drops it too, and `matches_nothing` therefore only looks at `#` keys. A backend compiling tag constraints by hand has to make the same distinction.

Builders consume and return `self`, and are named `add_*` / `remove_*` / `clear_*`:

```rust
Filter::new()
    .add_kinds([1, 30023])
    .add_author(pubkey)
    .add_tags(TagMatch::Any, "t", ["nostr", "rust"])
    .add_since(now() - 86_400)
    .add_limit(50)
    .add_address(&address)       // kind + author + #d in one call
```

Tag constraints take a `TagMatch` and a **bare** name rather than a prefixed key, so a typo cannot silently produce a constraint that never matches. `TagMatch::Any` is NIP-01 `#t` (any listed value); `TagMatch::All` is NIP-91 `&t` (every listed value).

`filter.matches(&event)` bounds on the five accessors it reads, so it works on a `HashedEvent` as well as an `Event`; `matches_any(&filters, &event)` is the OR across a `REQ`. `limit` is not a predicate and is ignored by matching. Serde produces and consumes the flat NIP-01 object with `#`-prefixed tag keys.

## Search

NIP-50 is relay-defined and opt-in. `SearchQuery::parse` splits on whitespace and keeps every token as a term — including `key:value` extensions, which a local scorer cannot evaluate and a relay still sees verbatim.

```rust
SearchQuery::parse("best nostr apps").terms;   // ["best", "nostr", "apps"]
query.score(content);                          // 0.0..=1.0, empty query scores 1.0
filter.search_score(&event);                   // 1.0 when no search is set
```

Scoring is deliberately **not** folded into `matches`: filter structurally, score for relevance, sort as you see fit.

## Sync (NIP-77)

Negentropy as a pure function over values — the `NEG-OPEN` / `NEG-MSG` / `NEG-CLOSE` frames belong to whatever carries them.

```rust
pub struct Item { pub timestamp: i64, pub id: [u8; 32] }   // ordered by (timestamp, id)

let local = SyncSet::from_items(items);
let message = sync::initiate(&local);                        // initiator's first message
let reply = sync::reconcile_responder(&remote, &message)?;   // responder
let out = sync::reconcile_initiator(&local, &reply)?;        // out.need, out.have
```

Implement `SyncStorage` — `size`, `get_item`, `find_lower_bound` — to reconcile straight out of a database instead of materializing a `SyncSet`. The trait supplies `fingerprint` from those three.

`Message::encode` / `decode` are the wire format; `to_hex` / `from_hex` the framing relays expect.

**A store that keeps `(created_at, id)` should hand back `sync::Item` directly** rather than defining its own pair type.

## Other NIPs

```rust
expiration::get_expiration(&tags);   // NIP-40, Option<i64>
event.is_expired();                  // via the prelude
protected::is_protected(&tags);      // NIP-70
pow::get_pow(&id, &tags);            // NIP-13 difficulty actually achieved
owned.mine_pow(difficulty);          // returns a HashedEvent
encryption::nip44_encrypt(&sk, &pk, plaintext)?;
```

## Working against SQLite

**Time needs no conversion.** Timestamps are `i64`, which is SQLite's integer, so they bind and read as themselves. (They were `u64` until August 2026, which rusqlite cannot bind at all — if you find a conversion helper in a codebase, it is left over from that.)

Ids, pubkeys and signatures are `[u8; 32]`, `PublicKey` and `[u8; 64]`; store them as hex `TEXT` (`hex::encode`, `PublicKey::to_hex`, `from_hex`) or as `BLOB` — but pick one and keep it, since every filter compiles against that choice. `PublicKey::from_hex` rejects anything not on the curve, which a `TEXT` column cannot.

## Conventions worth copying

- **Layered types over optional fields.** A stage of construction is a type, not a `None`.
- **Free functions for raw values, traits for domain types.** `is_replaceable(u16)` is callable from a storage layer that has no domain type in hand.
- **Bound on the fields you read**, not on "an event".
- **Don't model what you can't honor.** No tag taxonomy, no NIP-50 extension parsing — both would be a guess dressed as a type.
- **Parsing never validates.** Verification, matching and scoring are separate, explicit steps.
