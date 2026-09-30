<script lang="ts">
  import FeedControls from "$lib/components/FeedControls.svelte"
  import ItemCard from "$lib/components/ItemCard.svelte"
  import {kept} from "$lib/data/bookmarks"
  import {social} from "$lib/data/contacts"
  import {
    board,
    responses,
    saying,
    standingOf,
    sweptAt,
    view,
    warmthOf,
    type Item,
  } from "$lib/data/feed"
  import {session} from "$lib/data/session"

  let {onBoost}: {onBoost: (item: Item) => void} = $props()

  let now = $state(Date.now() / 1000)

  $effect(() => {
    const tick = setInterval(() => (now = Date.now() / 1000), 60_000)

    return () => clearInterval(tick)
  })

  const hidden = $derived($view.showing.length === 0)
</script>

<header class="pt-4 pb-3">
  <h1 class="text-2xl font-semibold">Community Board</h1>
</header>

<FeedControls view={$view} />

<div class="mt-4 space-y-3">
  {#each $board as item (item.event.id)}
    {@const keeping = $kept.has(item.event.id)}
    {@const swept = sweptAt(item, $session, keeping)}
    <ItemCard
      {item}
      social={$social}
      standing={standingOf($responses, $saying, item.event.id)}
      warmth={warmthOf(item, swept, now)}
      sweptAt={swept}
      session={$session}
      kept={keeping}
      {onBoost} />
  {/each}
</div>

{#if $board.length === 0}
  <p class="py-10 text-sm text-pretty text-muted-foreground">
    {hidden
      ? "Nothing is switched on in the filter."
      : "Nothing has reached this device yet. Things arrive when you are near other people."}
  </p>
{/if}
