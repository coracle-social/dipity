<script lang="ts">
  import Tag from "@lucide/svelte/icons/tag"
  import {Button} from "$lib/components/ui/button"
  import * as DropdownMenu from "$lib/components/ui/dropdown-menu"
  import {topics} from "$lib/kinds"

  let {topic, onPick}: {topic?: string; onPick: (topic?: string) => void} = $props()

  const chosen = $derived(topics.find(({id}) => id === topic))

  const anything = "any"
</script>

<DropdownMenu.Root>
  <DropdownMenu.Trigger>
    {#snippet child({props})}
      <Button {...props} variant={chosen ? "secondary" : "ghost"} size="sm">
        <Tag />
        {chosen ? chosen.label : "Any topic"}
      </Button>
    {/snippet}
  </DropdownMenu.Trigger>
  <DropdownMenu.Content align="end">
    <DropdownMenu.RadioGroup
      value={topic ?? anything}
      onValueChange={picked => onPick(picked === anything ? undefined : picked)}>
      <DropdownMenu.RadioItem value={anything}>Any topic</DropdownMenu.RadioItem>
      {#each topics as option (option.id)}
        <DropdownMenu.RadioItem value={option.id}>{option.label}</DropdownMenu.RadioItem>
      {/each}
    </DropdownMenu.RadioGroup>
  </DropdownMenu.Content>
</DropdownMenu.Root>
