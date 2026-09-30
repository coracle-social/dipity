<script lang="ts">
  import type {Warmth} from "$lib/data/feed"

  // How long an item has left, and only once it is short enough to matter.
  let {warmth, sweptAt}: {warmth: Warmth; sweptAt?: number} = $props()

  const days = $derived(sweptAt ? Math.round((sweptAt * 1000 - Date.now()) / 86_400_000) : 0)
</script>

{#if warmth === "fading" || warmth === "cold"}
  <span
    class="text-xs whitespace-nowrap {warmth === 'cold'
      ? 'text-primary'
      : 'text-muted-foreground'}">
    {days > 1 ? `${days} days left` : "going today"}
  </span>
{/if}
