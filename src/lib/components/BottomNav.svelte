<script lang="ts">
  import Bookmark from "@lucide/svelte/icons/bookmark"
  import Settings from "@lucide/svelte/icons/settings"
  import Signpost from "@lucide/svelte/icons/signpost"
  import Users from "@lucide/svelte/icons/users"
  import {go, tabOf, type Place} from "$lib/data/nav"

  let {place}: {place: Place} = $props()

  const items = [
    {tab: "board", label: "Board", icon: Signpost, to: {at: "board"} as Place},
    {tab: "bookmarks", label: "Bookmarks", icon: Bookmark, to: {at: "bookmarks"} as Place},
    {tab: "people", label: "People", icon: Users, to: {at: "people"} as Place},
    {tab: "settings", label: "Settings", icon: Settings, to: {at: "settings"} as Place},
  ]

  const here = $derived(tabOf(place))
</script>

<!-- The card runs under the home indicator so that nothing scrolls past beneath the bar. -->
<nav class="pointer-events-auto border-t border-border bg-card pb-safe-b">
  <ul class="mx-auto flex max-w-2xl">
    {#each items as item (item.tab)}
      {@const Icon = item.icon}
      <li class="flex-1">
        <button
          type="button"
          class="flex h-14 w-full flex-col items-center justify-center gap-0.5 transition-colors
                 {here === item.tab ? 'text-primary' : 'text-muted-foreground'}"
          aria-current={here === item.tab ? "page" : undefined}
          onclick={() => go(item.to)}>
          <Icon class="size-5" />
          <span class="text-xs font-medium">{item.label}</span>
        </button>
      </li>
    {/each}
  </ul>
</nav>
