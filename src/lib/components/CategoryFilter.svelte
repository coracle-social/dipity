<script lang="ts">
  import Filter from "@lucide/svelte/icons/list-filter"
  import {Button} from "$lib/components/ui/button"
  import * as DropdownMenu from "$lib/components/ui/dropdown-menu"
  import {categories} from "$lib/kinds"

  let {showing, onToggle}: {showing: string[]; onToggle: (id: string) => void} = $props()

  const narrowed = $derived(showing.length !== categories.length)
</script>

<DropdownMenu.Root>
  <DropdownMenu.Trigger>
    {#snippet child({props})}
      <Button {...props} variant={narrowed ? "secondary" : "ghost"} size="sm">
        <Filter />
        {narrowed ? `${showing.length} of ${categories.length}` : "Everything"}
      </Button>
    {/snippet}
  </DropdownMenu.Trigger>
  <DropdownMenu.Content align="end">
    {#each categories as category (category.id)}
      <DropdownMenu.CheckboxItem
        checked={showing.includes(category.id)}
        closeOnSelect={false}
        onCheckedChange={() => onToggle(category.id)}>
        {category.label}
      </DropdownMenu.CheckboxItem>
    {/each}
  </DropdownMenu.Content>
</DropdownMenu.Root>
