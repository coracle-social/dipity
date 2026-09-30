<script lang="ts">
  import Bookmark from "@lucide/svelte/icons/bookmark"
  import Repeat from "@lucide/svelte/icons/repeat-2"
  import Smile from "@lucide/svelte/icons/smile"
  import Trash from "@lucide/svelte/icons/trash-2"
  import {Button} from "$lib/components/ui/button"
  import Byline from "$lib/components/Byline.svelte"
  import Cooling from "$lib/components/Cooling.svelte"
  import EmojiPicker from "$lib/components/EmojiPicker.svelte"
  import ItemBody from "$lib/components/ItemBody.svelte"
  import Reactions from "$lib/components/Reactions.svelte"
  import {toggleBookmark} from "$lib/data/bookmarks"
  import {isKnown, nameOf, type Social} from "$lib/data/contacts"
  import {
    drop,
    opensId,
    react,
    retract,
    type Item,
    type Standing,
    type Warmth,
  } from "$lib/data/feed"
  import {go} from "$lib/data/nav"
  import type {Session} from "$lib/data/session"
  import {categoryOf} from "$lib/kinds"

  let {
    item,
    social,
    standing,
    warmth,
    sweptAt,
    session,
    bookmarked,
    onBoost,
    detailed = false,
  }: {
    item: Item
    social: Social
    standing: Standing
    warmth: Warmth
    sweptAt?: number
    session: Session
    /** Whether the user bookmarked this, which also spares it from the sweep. */
    bookmarked: boolean
    onBoost: (item: Item) => void
    /** Whether this is the card the detail page is about, which opens nothing. */
    detailed?: boolean
  } = $props()

  let picking = $state(false)
  let reading = $state(false)

  const mine = $derived(item.event.pubkey === session.identity)

  const hops = $derived(mine || isKnown(social, item.event.pubkey) ? 1 : 2)

  const category = $derived(categoryOf(item.event.kind))

  const opens = $derived(opensId(item))

  const Mark = $derived(category.icon)

  const arrived = $derived(
    new Date(item.seenAt * 1000).toLocaleString(undefined, {
      weekday: "short",
      hour: "numeric",
      minute: "2-digit",
    }),
  )

  const carrier = $derived(
    item.from[0] && item.from[0] !== session.identity
      ? nameOf(social, item.from[0]).name
      : undefined,
  )

  const passedOn = $derived(standing.response.boosts.length)

  const tally = $derived(
    standing.response.reactions.reduce<Record<string, number>>(
      (counts, {emoji}) => ({...counts, [emoji]: (counts[emoji] ?? 0) + 1}),
      {},
    ),
  )

  const sent = $derived(
    new Set(
      standing.response.reactions
        .filter(({pubkey}) => pubkey === session.identity)
        .map(({emoji}) => emoji),
    ),
  )
</script>

{#snippet said()}
  <header class="flex items-baseline justify-between gap-3 text-sm">
    {#if mine}
      <span class="font-semibold">You</span>
    {:else}
      <Byline {social} pubkey={item.event.pubkey} />
    {/if}
    <span class="flex flex-none items-center gap-2">
      <Cooling {warmth} {sweptAt} />
      <Mark class="size-4 text-muted-foreground" aria-label={category.noun} />
    </span>
  </header>

  <div class="mt-2">
    <ItemBody {item} {social} {standing} {session} {detailed} />
  </div>
{/snippet}

<article class="relative rounded-lg bg-card shadow-sm">
  {#if !detailed}
    <!-- The card opens the thing from underneath, so a control drawn on it is still its own tap. -->
    <button
      type="button"
      class="absolute inset-0 rounded-lg"
      aria-label="Open this"
      onclick={() => go({at: "item", id: opens})}></button>
  {/if}

  <div class="px-4 pt-3">
    {@render said()}
  </div>

  {#if Object.keys(tally).length > 0}
    <div class="relative flex flex-wrap gap-1 px-4 pt-3">
      {#each Object.entries(tally) as [emoji, count] (emoji)}
        <button
          type="button"
          class="rounded-full border px-2 py-0.5 text-xs transition-colors
                 {sent.has(emoji)
            ? 'border-secondary-accent text-secondary-accent'
            : 'border-border text-muted-foreground'}"
          aria-label="Who reacted"
          onclick={() => (reading = true)}>
          {emoji}
          {count}
        </button>
      {/each}
    </div>
  {/if}

  <footer class="relative mt-3 flex items-center justify-between gap-2 px-4 pb-3">
    <p class="min-w-0 truncate text-xs text-muted-foreground">
      {arrived}{carrier ? ` · from ${carrier}` : ""}{hops === 2 ? " · two hops" : ""}{passedOn > 0
        ? ` · ${passedOn} passed on`
        : ""}{standing.saying > 0 ? ` · ${standing.saying} saying` : ""}
    </p>

    <div class="flex flex-none items-center gap-0.5">
      <Button variant="ghost" size="icon-sm" aria-label="React" onclick={() => (picking = true)}>
        <Smile />
      </Button>
      <Button variant="ghost" size="icon-sm" aria-label="Pass it on" onclick={() => onBoost(item)}>
        <Repeat />
      </Button>
      <Button
        variant="ghost"
        size="icon-sm"
        aria-label={bookmarked ? "Remove bookmark" : "Bookmark this"}
        aria-pressed={bookmarked}
        class={bookmarked ? "text-primary" : undefined}
        onclick={() => toggleBookmark(item)}>
        <Bookmark />
      </Button>
      <Button
        variant="ghost"
        size="icon-sm"
        aria-label={mine ? "Ask for this to be forgotten" : "Drop this from this device"}
        onclick={() => (mine ? retract(item) : drop(item))}>
        <Trash />
      </Button>
    </div>
  </footer>
</article>

<EmojiPicker bind:open={picking} onPick={emoji => react(item, emoji)} />
<Reactions bind:open={reading} response={standing.response} {social} {session} />
