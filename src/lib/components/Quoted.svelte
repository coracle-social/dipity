<script lang="ts">
  import {nameOf, type Social} from "$lib/data/contacts"
  import {heldEvent} from "$lib/data/feed"
  import {go} from "$lib/data/nav"
  import {summaryOf} from "$lib/kinds"

  let {
    id,
    social,
    absent,
  }: {
    id: string
    social: Social
    /** What to say when the thing this stands for never reached this device. */
    absent: string
  } = $props()
</script>

{#await heldEvent(id)}
  <p class="border-l-2 border-border pl-3 text-sm text-muted-foreground">Looking for it…</p>
{:then held}
  {#if held}
    <!-- Drawn over the card's own tap target, so this opens what it stands for. -->
    <button
      type="button"
      class="relative block w-full border-l-2 border-border pl-3 text-left"
      onclick={() => go({at: "item", id})}>
      <span class="text-xs text-muted-foreground">{nameOf(social, held.pubkey).name} wrote</span>
      <span class="mt-1 line-clamp-3 block text-sm text-pretty">{summaryOf(held)}</span>
    </button>
  {:else}
    <p class="border-l-2 border-border pl-3 text-sm text-muted-foreground">{absent}</p>
  {/if}
{/await}
