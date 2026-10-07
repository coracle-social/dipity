# Dipity — UI

The view layer: component framework, design tokens, and the conventions a linter can hold us to. Everything here lives above [the plugin boundary](./overview.md#architecture) and none of it runs at encounter time.

## The framework

[shadcn-svelte](https://shadcn-svelte.com) `1.5`, over [bits-ui](https://bits-ui.com) primitives and Tailwind CSS `4`.

| Package | Role |
| --- | --- |
| `shadcn-svelte` | CLI only. Copies component source into the repo; never imported at runtime. |
| `bits-ui` | Headless behavior — focus traps, roving tabindex, ARIA wiring, dismissal. |
| `tailwindcss` | The token system. Every design decision below is a Tailwind theme variable. |
| `tailwind-variants` | Variant tables (`tv`) inside components. |
| `tailwind-merge`, `clsx` | `cn()` — conflict-aware class merging, so a caller's `class` can override. |
| `@lucide/svelte` | Icons, tree-shaken per import. |
| `mode-watcher` | Light/dark class on `<html>`, persisted. |
| `@fontsource-variable/figtree` | The typeface, as an npm package rather than a CDN link. |

**shadcn-svelte is a code generator, not a dependency.** `just ui button` writes `src/lib/components/ui/button/` into the repo and that source is ours from then on. This suits an app whose surfaces have no analogue in a general component library — a hop badge, a consent gate, a peer in range right now and gone in ninety seconds. Owning the source makes those edits rather than fights with someone's variant API.

The cost: nothing updates itself. A fix upstream reaches us only if someone re-runs the CLI, and any local edit to a vendored file is a merge conflict waiting to happen. Hence [the vendored seam](#the-vendored-seam).

**Fonts are bundled, never fetched.** `@fontsource-variable/figtree` resolves to woff2 files Vite emits into `dist/`. A Google Fonts `@import` would put a network request on first paint, which fails exactly where this app is supposed to work — nothing in the gossip path may require the network. The same rule covers icons, which is why Lucide is an npm package and not a sprite sheet on a CDN.

## Where things live

| Path | What |
| --- | --- |
| `src/app.css` | Every design token. The single source of color, elevation, motion and spacing. |
| `src/lib/components/ui/` | Vendored shadcn components. Generated — see below. |
| `src/lib/components/` | Our components. Everything app-specific. |
| `src/lib/kinds/` | Domain kinds — a `KindFactory` (reader + writer) per event kind. |
| `src/lib/data/` | The controller layer — queries against the core, and the caches over them. |
| `src/lib/dev/` | The simulated core `just dev` runs against. Never in a shipped bundle. |
| `src/lib/utils.ts` | `cn()` and the prop-type helpers shadcn components import. |
| `components.json` | CLI config: aliases, base color, style. Read by `just ui`, not at build time. |

## The browser has a core

A device reaches the real one through [the plugin boundary](./overview.md#architecture), and a browser reaches nothing: `registerPlugin` answers a proxy with no implementation behind it, so every call rejects and a screen can only render its empty state.

`src/lib/dev/` is the other half of that boundary — the same surface over an in-memory store, a faker-generated neighborhood, and a clock that walks it past the device. People turn up, strangers ask to pair, bursts of events arrive, and something already stored is seen again from somebody new. What the view publishes is stored and comes back out of a query.

It is not a second implementation of anything the core decides. Sync, policy, proofs and the radio are below the bridge and are not modelled; what is modelled is their observable shape, which is what a screen is built against.

It refuses what the core refuses, and that half matters more than what it answers. A simulator that stores an ephemeral event, or keeps every version of a replaceable one, is more forgiving than the store a device writes to — so a write the core drops looks like it landed, and the screen that reads it back is only right in a browser.

The shapes themselves are declared once, in `core.ts`, and the simulator answers with them rather than with types of its own — a stored event is `@welshman/util`'s `HashedEvent`, the same unsigned event `coracle-lib` stores, and a query narrows through `matchFilter`. A fixture that restates the contract in its own words is a fixture that can drift from it.

What a simulated peer writes comes out of `@welshman/domain` too, bound in `src/lib/dev/kinds.ts` rather than taken from `src/lib/kinds/`. The dependency points one way: a stand-in for the core that reached for the app would only prove the app agrees with itself. Sharing the library instead means neither side spells a tag out, so a kind the view learns to read is a kind a peer already writes correctly.

`src/lib/core.ts` loads it by dynamic import under `import.meta.env.DEV`, so the branch is dead code in a shipped bundle and neither the simulator nor faker is in one. It is also `window.dip`, because a browser has no second phone: a peer offering this device its identity is started from the console with `dip.receiveOffer(link)`.

**Nothing but `core.ts` may import it**, which `just lint` enforces. A view that reaches past the boundary for a fixture has a code path that only ever runs in a browser, and the screen it draws there is not the screen a device draws.

## The vendored seam

`src/lib/components/ui/` is written by the CLI, so it is fenced off from our conventions:

- **Prettier ignores it** (`.prettierignore`). It stays in upstream's formatting — tabs, double quotes — so that re-adding a component is a clean diff instead of a whitespace conflict.
- **Lint is relaxed there** (`eslint.config.js`). Correctness and accessibility rules still apply; the house conventions do not, because the next `add` would overwrite them.
- **Add with `just ui <name>`**, never by hand-copying from the website.
- **Edit deliberately or not at all.** Restyling belongs in `app.css`, where it reaches every component at once. If a vendored file has to change, say so in the commit; it no longer tracks upstream.

App components go in `src/lib/components/`. The `ui/` directory is a boundary rather than a filing convention, so nothing app-specific belongs in it.

## The composition rule

**Feature code composes components. Components own the pixels.**

The line is the `src/lib/components/` directory, because that is the line a linter can see. Inside it, precise work is expected: variant tables, and arbitrary values where a design needs one. Outside it, a screen assembles components and positions them, and reaches for a design value only through a token.

Arbitrary values — `bg-[#3a2f28]`, `w-[13px]`, `text-[0.6875rem]` — are rejected outside `src/lib/components/`. This is the rule the rest of the convention layer rests on: a style guide holds for about six files before something is locally convenient enough to inline. Arbitrary *variants* are fine (`supports-[backdrop-filter]:`, `group-data-[state=open]:`, `[&_svg]:`); they select things rather than inventing values.

A screen that needs a value which does not exist has two legal moves: add it to `@theme` in `app.css` so it has a name, or push the markup down into a component. "Make the button subtle" has exactly one spelling, `<Button variant="ghost">`.

Outside components, layout, spacing and type utilities remain available. The allowlist is a deny-list rather than an allow-list — no arbitrary values, no palette colors — and it narrows as the component library absorbs the patterns that need them.

### Runes stay in components

`$state`, `$derived`, `$effect`, `$props` and friends are compiler syntax, and the compiler only runs on `.svelte` files. In a plain `.ts` module they are an undefined global that fails at runtime rather than at build, so the linter rejects them there.

Shared reactive state goes in a store from `svelte/store` instead, which works identically in a module and in a component. That also keeps `.svelte.ts` out of the tree, where a rune-bearing module would be a second reactivity system running alongside the controller's stores.

Vendored `ui/` is exempt — upstream ships `.svelte.ts` files and they are not ours to restructure.

## Design

The app is **buttoned-down but pleasant to use**, and gets out of the user's way. The content is other people's posts; the interface is the paper they are printed on. Restraint is not flatness, though: a neighborhood gossip app that looks like an admin dashboard has the wrong character.

The specific answer is **restrained claymorphism**: soft, slightly thick surfaces with warm diffuse shadows and generous rounding, over a warm off-white page. Surfaces feel like objects you could pick up. Nothing glows or has a gradient.

Clay lives in the *chrome* — cards, controls, sheets — and content sits flat on it. When every element is tactile, tactility carries no information.

### Color

Semantic tokens only. `bg-card`, `text-muted-foreground`, `border-border` — never `bg-white` or `text-gray-500`, which are correct in exactly one theme. This is [enforced](#enforced-by-the-linter).

| Token | Use |
| --- | --- |
| `background` / `foreground` | The page. Warm off-white, warm near-black — never `#fff` or `#000`. |
| `card` / `card-foreground` | Raised surfaces. Lighter than the page in *both* themes, so they read as lift. |
| `popover` | Floating surfaces. Lighter again. |
| `muted` / `muted-foreground` | Recessed fills and secondary text. |
| `accent` / `accent-foreground` | Hover and active states on neutral surfaces. |
| `primary` | Terracotta. The loud accent, and the rationed one. |
| `secondary-accent` | Muted teal. The quiet accent — informational chips, status dots, anything that needs color without asking for the eye. |
| `destructive` | Irreversible actions only. |
| `border` / `input` / `ring` | Hairlines and focus. |

Values are `oklch`, so lightness is perceptually even: `oklch(0.7 …)` reads as the same brightness at every hue, which makes the dark theme derivable rather than hand-tuned. Both themes are defined in `app.css`, and dark mode is a `.dark` class on `<html>` set before first paint by an inline script in `index.html` — `ModeWatcher` runs after the bundle parses, which on a cold launch is a white flash on a dark-mode phone.

**`primary` is rationed.** One primary action per screen. Terracotta at scale stops being warm and starts being loud. A state the user set themselves wears it too, and the lit bookmark is the only one. `secondary-accent` at 0.07 chroma is too quiet to find among four grey icons.

`secondary-accent` is what carries color everywhere else. It sits at roughly half the chroma of `primary` (0.07 against 0.148), which is what makes it read as subordinate — not lower contrast, which would just make it hard to read, but less saturated, so it recedes while staying legible.

Three tokens have confusable names, and only one of them is a hue:

| Token | Is |
| --- | --- |
| `secondary` | A neutral surface. Drives `<Button variant="secondary">` and `<Badge variant="secondary">`. |
| `accent` | A neutral hover/active state. Drives every dropdown and menu item. |
| `secondary-accent` | The teal. The only one of the three that is a color. |

`secondary` and `accent` are wired into the vendored components, so neither could be repurposed without restyling every secondary button and every menu hover.

### Elevation

Six steps. A component picks one by **what it is**, not by how much lift looks good in the moment.

| Token | Use |
| --- | --- |
| `shadow-xs` | Inputs and controls at rest |
| `shadow-sm` | Cards, list rows — anything in the document flow |
| `shadow-md` | Lifted on hover or press |
| `shadow-lg` | Popovers, dropdowns, tooltips |
| `shadow-xl` | Sheets and drawers |
| `shadow-2xl` | Modal dialogs |
| `shadow-control` | Filled interactive controls at rest — buttons |
| `shadow-control-raised` | The same control under the pointer |
| `inset-shadow-clay` | Pressed *into* the page — toggles, wells, active segments |

A button is not a small card, and gets its own two steps. Both `--clay-rim` and `--clay-base` are calibrated for near-white card stock, where a near-opaque white edge reads as a lit rim and 5% warm gray reads as a shaded one. On a saturated fill the first is a hard gloss line and the second is invisible, so `shadow-control` works over `--clay-rim-fill` and `--clay-base-fill` instead, and tightens the penumbra — a card-sized one under a button reads as a floating tile.

The thickness comes from edges rather than curvature: a lit hairline along the top, a shaded one along the bottom, and only a trace of interior shading above it. A blurred highlight falling away from the top edge is what makes a surface read as a bubble, so there isn't one — a button is a slab with soft corners, not a blown shape.

Buttons press. `shadow-control` at rest, `shadow-control-raised` under the pointer, and on `:active` the drop shadow collapses to `shadow-2xs` while `inset-shadow-clay` pushes the surface into the page, alongside the 1px translate the base already carries. `ghost` and `link` stay flat, because a button with no surface has nothing to lift.

Each step is built from four primitives in `app.css`: `--clay-umbra` (the tight contact shadow), `--clay-penumbra` (the wide diffuse one), `--clay-rim` (a lit top edge) and `--clay-base` (a shaded bottom edge). The rim and base give a surface thickness rather than mere separation.

The standard scale is redefined rather than supplemented with a `shadow-clay` utility: shadcn components are written against `shadow-sm` and `shadow-lg`, so overriding the scale restyles the entire vendored set without editing a component file. The same applies to `--radius` and to the default transition.

### Motion

**Liberal, and subtle.** Almost every state change is animated; no animation asks to be noticed. If a user would describe the app as "animated", it is too much.

- **One curve.** `ease-clay` — `cubic-bezier(0.32, 0.72, 0, 1)` — quick to leave, slow to settle, **no overshoot**. Bounce and spring make an interface ask to be looked at. `ease-exit` is the counterpart for dismissals, which are faster and less interesting than entrances.
- **A bare `transition` is already correct.** `--default-transition-duration` is 180ms and the default timing function is `ease-clay`, so `transition-colors` with no modifiers picks up the house feel. Set an explicit duration only when 180ms is wrong.
- **Transform and opacity only.** Animating layout properties forces reflow, and this app has a Rust core doing crypto and a radio scanning in the background. The frame budget is not ours alone.
- **Movement carries meaning.** Sheets slide because they come from an edge. Tab panels cross-fade, because sliding would imply a spatial relationship between tabs that does not exist.
- **Never gate an interaction on an animation.** The reduced-motion rule in `app.css` collapses every duration to 1ms, and is safe precisely because nothing waits on one.

### Typography

**Figtree**, variable, self-hosted. Geometric and friendly with rounded terminals that echo the clay surfaces, and legible down to caption sizes on a phone. `--font-mono` is a system stack, used for anything read character by character — event ids, npubs, hex keys.

Weight carries hierarchy before size does: a 14px semibold label reads as distinct from a 14px regular one, and a short size ramp keeps dense list views legible.

### Layout and touch

- **Safe areas are spacing tokens.** `pt-safe-t`, `pb-safe-b`, `pl-safe-l`, `pr-safe-r` compose like any other spacing utility. They only return non-zero because `index.html` sets `viewport-fit=cover`; without it iOS reports zero insets and the header slides under the notch.
- **Touch targets are at least 44px.** Use the `lg` control sizes on primary actions; `icon-sm` and below are for dense secondary rows only.
- **Primary actions sit low.** Phones are held one-handed and the top of a modern screen is out of thumb reach.
- **Zoom stays enabled.** No `user-scalable=no`. It is the one accessibility affordance a webview cannot reimplement itself.

## Images

A picture is a NIP-68 kind 20 post: one image and a description. `src/lib/data/media.ts` re-encodes the picked file through a canvas before it is published, as a JPEG of at most 1280 pixels on its long edge and a 640-pixel preview standing in for it ([`nips/imeta-preview.md`](./nips/imeta-preview.md)). Re-encoding is also what strips the camera's metadata, a location included. The core describes each for its `imeta` tag, which carries a hash rather than a url, and stores both beside the event.

The view draws a blob from the file the core keeps it in, once all of it is there (`blobPath`). A picture post is left off the board until one of its images is whole, because the image is the post. The board draws the preview and the post's own page the image.

Whose pictures are blurred until tapped is a preference, `display.blur`: people outside the user's network by default, everyone but contacts, or everyone but the user.

## Conventions

### Enforced by the linter

`just lint`.

| Rule | Stops |
| --- | --- |
| `no-restricted-classes` → arbitrary values | `bg-[#3a2f28]` outside [`src/lib/components/`](#the-composition-rule). |
| `no-restricted-classes` → palette, easings | `bg-white`, `text-gray-500`, `ease-in-out`. |
| `no-unknown-classes` | Typos, and utilities for tokens that do not exist. Resolved against `app.css`, so the theme *is* the vocabulary. |
| `no-conflicting-classes` | `px-2 px-3` and friends, where the winner depends on stylesheet order. |
| `no-restricted-syntax` → `SvelteStyleElement` | Component-scoped `<style>`. See below. |
| `no-restricted-syntax` → runes in `.ts` | [Runes outside components.](#runes-stay-in-components) |
| `svelte/no-at-html-tags` | `{@html}` on nostr content, which is attacker-controlled. |
| `svelte/require-each-key` | Keyless `{#each}` over events, which recycles DOM across ids. |

Skim `src/lib/components/` periodically. It is where drift starts, and it is the one place the rules above are relaxed.

### No `<style>` blocks

Styling happens in Tailwind utilities, or in the theme. A component-scoped rule is invisible to every other component, cannot participate in the token system, and is where the second, undocumented set of colors always begins. If a value is missing, add it to `@theme` in `app.css` — then both themes and every other component get it too.

### Held by review

- **`cn()` last, always.** `class={cn(variants({size}), className)}` — the caller's class has to be able to win, and `twMerge` is what makes that deterministic.
- **`tv()` for variants, not conditionals.** If a component has more than two visual states, it gets a variant table.
- **Semantic HTML before ARIA.** bits-ui handles the wiring for anything interactive; hand-rolled `role` attributes are a sign the wrong primitive was used.
- **The UI owns no durable state.** The view is suspended in the background, so anything that must survive that lives in the core. Where the user is does not survive; a choice they made about what a screen is for does, as a preference under a `ui.` key (`remembered` in `src/lib/data/query.ts`).
- **Never claim posts only reach nearby people.** Copy may say the phone trades with devices in range, since every link is physical. Copy about who ends up with a post says reach is bounded at two hops, because a second-hop recipient may be anywhere.

## Organizing against welshman

`@welshman/util` supplies event types, kinds, tags and filters; `@welshman/lib` the standalone helpers; and `@welshman/domain` the typed reader/writer pairs. `@welshman/app` is not a dependency — its `App`, `Repository` and derived-store layer assume an in-memory event store, and there isn't one ([`storage.md`](./storage.md#the-sqlite-store)).

### Domain kinds

**An event kind is a `KindFactory` in `src/lib/kinds/`, and nothing outside it touches tags.**

`@welshman/domain` pairs each kind with a Reader (a read-only view over an event, all getters synchronous) and a Writer (chainable setters that render an `EventTemplate`). Ours are declared the same way the built-ins are:

```typescript
// src/lib/kinds/post.ts
export const Post = new KindFactory({kind: NOTE, reader: PostReader, writer: PostWriter})
```

Every factory is configured once, in `src/lib/kinds/index.ts`, against a resolver that answers no relays — which is the truth rather than a stub, since there are none and a writer's routing half is never asked. Components read through the getters that configuration mints.

A kind `@welshman/domain` already models is re-exported configured rather than redeclared, and the rest are its classes at a kind number or a reader of its own it has none of. Block (`people.ts`) is a kind of ours spelled the way NIP-51's mute list is, so it uses its reader and writer. A contact card (`contact.ts`) is ours, addressed to the person it names and carrying that name as its content. A boost (`repost.ts`) is NIP-18 without the embedded copy of what it names, since an event on the gossip path carries no signature to embed.

**`event.tags.find(t => t[0] === "…")` does not appear in a component.** A kind's shape is stated once, in its reader, and every screen reads it through getters. Unmodeled tags survive an edit, because a writer seeded from a reader re-emits whatever it did not model.

### Components do not query the core

**A collection of events is a store in `src/lib/data/`, and components never open a query themselves.**

A component takes a store and renders it; it does not know a bridge exists. `plugin` is an overloaded word here, which is why this directory is `data/` and never `plugins/` — the **native plugin** is the Capacitor boundary the core lives behind ([`overview.md`](./overview.md#architecture)).

### Replacing a list

**A replaceable event is published with the one it replaces, and `publish` stamps it a second later.** Two versions carrying the same second are settled by the lower id, so a rename or a bookmark made within a second of the last is refused by the store while `publish` still answers ([`storage.md`](./storage.md#the-schema)). The screen then redraws the version it thought it had just replaced.

### What does not transfer

There is no relay selection, no outbox computation, and no thunk: publishing is one call to the core ([`storage.md`](./storage.md#the-sqlite-store)).

Flotilla is still worth reading for Svelte idiom and for how readers are used in markup, but its data layer does not apply: it is built on `@welshman/app` against real relays. Clone it into `./ref/flotilla`; see [Reference materials](../AGENTS.md#reference-materials).

## Formatting

Prettier, in the [Coracle house style](https://github.com/coracle-social/coracle/blob/master/.prettierrc): no semicolons, double quotes, no bracket spacing, brackets on the same line, 100 columns. `prettier-plugin-tailwindcss` sorts class lists, so class order is never a review comment.

Markdown is excluded. Prettier pads every table to its widest cell and rewrites `*emphasis*` to `_emphasis_`, which turns a one-line edit to a design doc into a hundred-line diff.

## Commands

```sh
just dev             # Vite dev server, over the simulated core
just ui <component>  # vendor a shadcn component; no args to pick from a list
just lint            # eslint
just fmt             # prettier + eslint --fix + cargo fmt
just qa              # types, lint, format check, the Rust half, the Android shell
```
