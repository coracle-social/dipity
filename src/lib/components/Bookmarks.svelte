<script lang="ts">
  import Bookmark from "@lucide/svelte/icons/bookmark"
  import EmptyState from "$lib/components/EmptyState.svelte"
  import ItemCard from "$lib/components/ItemCard.svelte"
  import {bookmarked, bookmarkedItems} from "$lib/data/bookmarks"
  import {social} from "$lib/data/contacts"
  import {responses, saying, standingOf, warmthOf, type Item} from "$lib/data/feed"
  import {session} from "$lib/data/session"

  let {onBoost}: {onBoost: (item: Item) => void} = $props()

  const now = Date.now() / 1000
</script>

<header class="pt-4 pb-3">
  <h1 class="text-2xl font-semibold">Bookmarks</h1>
  <p class="mt-1 text-sm text-muted-foreground">
    What you bookmark is never cleared from this device, however long ago it stopped going around.
  </p>
</header>

<div class="mt-1 space-y-3">
  {#each $bookmarkedItems as item (item.event.id)}
    <ItemCard
      {item}
      social={$social}
      standing={standingOf($responses, $saying, item.event.id)}
      warmth={warmthOf(item, undefined, now)}
      session={$session}
      bookmarked={$bookmarked.has(item.event.id)}
      {onBoost} />
  {/each}
</div>

{#if $bookmarkedItems.length === 0}
  <EmptyState icon={Bookmark}>No bookmarks yet. The bookmark on a card saves it here.</EmptyState>
{/if}
