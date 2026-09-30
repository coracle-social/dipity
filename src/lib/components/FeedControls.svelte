<script lang="ts">
  import Filter from "@lucide/svelte/icons/list-filter"
  import {Button} from "$lib/components/ui/button"
  import * as DropdownMenu from "$lib/components/ui/dropdown-menu"
  import {setOrder, toggleCategory, type View} from "$lib/data/feed"
  import {categories} from "$lib/kinds"

  let {view}: {view: View} = $props()

  // Two words for the same list: the order it reached this device, and the order it was written.
  const orders = [
    {value: "seenAt", label: "New to you"},
    {value: "createdAt", label: "Recent"},
  ] as const

  const narrowed = $derived(view.showing.length !== categories.length)
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

  <DropdownMenu.Root>
    <DropdownMenu.Trigger>
      {#snippet child({props})}
        <Button {...props} variant={narrowed ? "secondary" : "ghost"} size="sm">
          <Filter />
          {narrowed ? `${view.showing.length} of ${categories.length}` : "Everything"}
        </Button>
      {/snippet}
    </DropdownMenu.Trigger>
    <DropdownMenu.Content align="end">
      {#each categories as category (category.id)}
        <DropdownMenu.CheckboxItem
          checked={view.showing.includes(category.id)}
          closeOnSelect={false}
          onCheckedChange={() => toggleCategory(category.id)}>
          {category.label}
        </DropdownMenu.CheckboxItem>
      {/each}
    </DropdownMenu.Content>
  </DropdownMenu.Root>
</div>
