<script lang="ts">
  import Filter from "@lucide/svelte/icons/list-filter"
  import Radar from "@lucide/svelte/icons/radar"
  import Search from "@lucide/svelte/icons/search"
  import EmptyState from "$lib/components/EmptyState.svelte"
  import FeedControls from "$lib/components/FeedControls.svelte"
  import ItemCard from "$lib/components/ItemCard.svelte"
  import {bookmarked} from "$lib/data/bookmarks"
  import {social} from "$lib/data/contacts"
  import {
    board,
    responses,
    saying,
    search,
    standingOf,
    sweptAt,
    view,
    warmthOf,
    type Item,
  } from "$lib/data/feed"
  import {policy} from "$lib/data/policy"
  import {session} from "$lib/data/session"

  let {onBoost}: {onBoost: (item: Item) => void} = $props()

  let now = $state(Date.now() / 1000)

  $effect(() => {
    const tick = setInterval(() => (now = Date.now() / 1000), 60_000)

    return () => clearInterval(tick)
  })

  const hidden = $derived($view.showing.length === 0)
  const searching = $derived($search.trim().length > 0)
</script>

<header class="pt-4 pb-3">
  <h1 class="text-2xl font-semibold">Community Board</h1>
</header>

<FeedControls view={$view} />

<div class="mt-4 space-y-3">
  {#each $board as item (item.event.id)}
    {@const marked = $bookmarked.has(item.event.id)}
    {@const swept = sweptAt(item, $session, $policy?.retention_days, marked)}
    <ItemCard
      {item}
      social={$social}
      standing={standingOf($responses, $saying, item.event.id)}
      warmth={warmthOf(item, swept, now)}
      sweptAt={swept}
      session={$session}
      bookmarked={marked}
      {onBoost} />
  {/each}
</div>

{#if $board.length === 0}
  <EmptyState icon={hidden ? Filter : searching ? Search : Radar}>
    {hidden
      ? "Nothing is switched on in the filter."
      : searching
        ? "Nothing on this phone says that. Only what has reached this device can be searched."
        : "Nothing has reached this device yet. Things arrive when you are near other people."}
  </EmptyState>
{/if}
