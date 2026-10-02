<script lang="ts">
  import Bookmark from "@lucide/svelte/icons/bookmark"
  import EmptyState from "$lib/components/EmptyState.svelte"
  import SearchBox from "$lib/components/SearchBox.svelte"
  import ItemCard from "$lib/components/ItemCard.svelte"
  import {bookmarked, bookmarkedItems} from "$lib/data/bookmarks"
  import {social} from "$lib/data/contacts"
  import {responses, saying, standingOf, warmthOf, type Item} from "$lib/data/feed"
  import {matches, nameMatches, wordsOf} from "$lib/data/search"
  import {session} from "$lib/data/session"
  import {summaryOf} from "$lib/kinds"

  let {onBoost}: {onBoost: (item: Item) => void} = $props()

  const now = Date.now() / 1000

  let query = $state("")

  const words = $derived(wordsOf(query))

  // A bookmark matches by what it says, its title, or who wrote it.
  const shown = $derived(
    words.length
      ? $bookmarkedItems.filter(
          ({event}) =>
            matches(words, `${summaryOf(event)} ${event.content}`) ||
            nameMatches($social, event.pubkey, words),
        )
      : $bookmarkedItems,
  )
</script>

<header class="pt-4 pb-3">
  <h1 class="text-2xl font-semibold">Bookmarks</h1>
  <p class="mt-1 text-sm text-muted-foreground">
    What you bookmark stays on this phone, however long ago it stopped going around.
  </p>
</header>

<SearchBox class="mb-3" label="Search bookmarks" bind:value={query} />

<div class="mt-1 space-y-3">
  {#each shown as item (item.event.id)}
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

{#if words.length && shown.length === 0 && $bookmarkedItems.length > 0}
  <EmptyState icon={Bookmark}>None of your bookmarks says that.</EmptyState>
{:else if $bookmarkedItems.length === 0}
  <EmptyState icon={Bookmark}>No bookmarks yet. The bookmark on a card saves it here.</EmptyState>
{/if}
