<script lang="ts">
  import Filter from "@lucide/svelte/icons/list-filter"
  import {Button} from "$lib/components/ui/button"
  import * as DropdownMenu from "$lib/components/ui/dropdown-menu"
  import {categories, topicLabel} from "$lib/kinds"

  // One button narrowing a list by content type and by topic, summarizing both on its face.
  let {
    showing,
    chosen,
    recent,
    onToggleCategory,
    onToggleTopic,
    onClear,
  }: {
    showing: string[]
    chosen: string[]
    /** Topics worth offering beyond the chosen ones, most used first. */
    recent: string[]
    onToggleCategory: (id: string) => void
    onToggleTopic: (topic: string) => void
    onClear: () => void
  } = $props()

  /** How many of the recent topics are offered beside the chosen ones. */
  const OFFERED = 12

  const offered = $derived([
    ...chosen,
    ...recent.filter(topic => !chosen.includes(topic)).slice(0, OFFERED),
  ])

  const types = $derived.by(() => {
    if (showing.length === categories.length) return undefined
    if (showing.length !== 1) return `${showing.length} types`

    return categories.find(({id}) => id === showing[0])?.label
  })

  const topics = $derived.by(() => {
    if (chosen.length === 0) return undefined
    if (chosen.length === 1) return topicLabel(chosen[0])

    return `${chosen.length} topics`
  })

  const summary = $derived(
    types || topics ? [types ?? "All types", topics].filter(Boolean).join(" · ") : "Everything",
  )
</script>

<DropdownMenu.Root>
  <DropdownMenu.Trigger>
    {#snippet child({props})}
      <Button {...props} variant={types || topics ? "secondary" : "ghost"} size="sm">
        <Filter />
        {summary}
      </Button>
    {/snippet}
  </DropdownMenu.Trigger>
  <DropdownMenu.Content align="end" class="max-h-96 min-w-48">
    <DropdownMenu.Group>
      <DropdownMenu.Label>Content types</DropdownMenu.Label>
      {#each categories as category (category.id)}
        <DropdownMenu.CheckboxItem
          checked={showing.includes(category.id)}
          closeOnSelect={false}
          onCheckedChange={() => onToggleCategory(category.id)}>
          {category.label}
        </DropdownMenu.CheckboxItem>
      {/each}
    </DropdownMenu.Group>

    {#if offered.length > 0}
      <DropdownMenu.Separator />
      <DropdownMenu.Group>
        <DropdownMenu.Label>Topics</DropdownMenu.Label>
        {#each offered as topic (topic)}
          <DropdownMenu.CheckboxItem
            checked={chosen.includes(topic)}
            closeOnSelect={false}
            onCheckedChange={() => onToggleTopic(topic)}>
            {topicLabel(topic)}
          </DropdownMenu.CheckboxItem>
        {/each}
      </DropdownMenu.Group>
    {/if}

    {#if types || topics}
      <DropdownMenu.Separator />
      <DropdownMenu.Item onSelect={onClear}>Show everything</DropdownMenu.Item>
    {/if}
  </DropdownMenu.Content>
</DropdownMenu.Root>
