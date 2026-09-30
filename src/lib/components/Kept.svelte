<script lang="ts">
  import ItemCard from "$lib/components/ItemCard.svelte"
  import {keeping, kept} from "$lib/data/bookmarks"
  import {social} from "$lib/data/contacts"
  import {responses, saying, standingOf, warmthOf, type Item} from "$lib/data/feed"
  import {session} from "$lib/data/session"

  let {onBoost}: {onBoost: (item: Item) => void} = $props()

  const now = Date.now() / 1000
</script>

<header class="pt-4 pb-3">
  <h1 class="text-2xl font-semibold">Kept</h1>
</header>

<div class="mt-1 space-y-3">
  {#each $keeping as item (item.event.id)}
    <ItemCard
      {item}
      social={$social}
      standing={standingOf($responses, $saying, item.event.id)}
      warmth={warmthOf(item, undefined, now)}
      session={$session}
      kept={$kept.has(item.event.id)}
      {onBoost} />
  {/each}
</div>

{#if $keeping.length === 0}
  <p class="py-10 text-sm text-pretty text-muted-foreground">
    Nothing kept yet. The bookmark on a card keeps it, and nothing you keep goes away.
  </p>
{/if}
