<script lang="ts">
  import CategoryFilter from "$lib/components/CategoryFilter.svelte"
  import SearchBox from "$lib/components/SearchBox.svelte"
  import TopicFilter from "$lib/components/TopicFilter.svelte"
  import {boardTopics, search, setOrder, setTopic, toggleCategory, type View} from "$lib/data/feed"

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

  <CategoryFilter showing={view.showing} onToggle={toggleCategory} />
</div>

<div class="mt-2 flex justify-end">
  <TopicFilter topic={view.topic} offered={$boardTopics} onPick={setTopic} />
</div>

<SearchBox class="mt-3" label="Search the board" bind:value={$search} />
