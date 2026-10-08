<script lang="ts">
  import ContentFilter from "$lib/components/ContentFilter.svelte"
  import SearchBox from "$lib/components/SearchBox.svelte"
  import {
    recentTopics,
    search,
    setOrder,
    showEverything,
    toggleCategory,
    toggleTopic,
    topicsOf,
    type View,
  } from "$lib/data/feed"

  let {view}: {view: View} = $props()

  // Two words for the same list: the order it reached this device, and the order it was written.
  const orders = [
    {value: "seenAt", label: "New to you"},
    {value: "createdAt", label: "Recent"},
  ] as const
</script>

<div class="flex items-center justify-between gap-2">
  <div class="inline-flex rounded-4xl bg-muted p-1">
    {#each orders as order (order.value)}
      <button
        type="button"
        class="rounded-4xl px-3 py-1.5 text-sm font-medium transition-colors
               {view.order === order.value
          ? 'bg-card text-foreground shadow-xs'
          : 'text-muted-foreground'}"
        aria-pressed={view.order === order.value}
        onclick={() => setOrder(order.value)}>
        {order.label}
      </button>
    {/each}
  </div>

  <ContentFilter
    showing={view.showing}
    chosen={topicsOf(view)}
    recent={$recentTopics}
    onToggleCategory={toggleCategory}
    onToggleTopic={toggleTopic}
    onClear={showEverything} />
</div>

<SearchBox class="mt-3" label="Search the board" bind:value={$search} />
