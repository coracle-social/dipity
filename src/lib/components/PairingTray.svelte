<script lang="ts">
  import ChevronRight from "@lucide/svelte/icons/chevron-right"
  import {go} from "$lib/data/nav"
  import type {Request} from "$lib/data/pairing"

  // Hard against the bar, which is what it reads as sliding out from behind.
  let {requests}: {requests: Request[]} = $props()

  const oldest = $derived(requests[0])

  const words = $derived(
    requests.length > 1
      ? `${requests.length} people nearby want to pair`
      : oldest?.known
        ? `${oldest.known} wants to pair again`
        : "Somebody nearby wants to pair",
  )
</script>

{#if oldest}
  <button
    type="button"
    class="pointer-events-auto flex w-full items-center gap-3 rounded-t-xl bg-secondary-accent
           px-5 py-3 text-left text-secondary-accent-foreground"
    onclick={() => go({at: "pairing", link: oldest.link})}>
    <span class="flex-1 text-sm font-semibold">{words}</span>
    <ChevronRight class="size-4" />
  </button>
{/if}
