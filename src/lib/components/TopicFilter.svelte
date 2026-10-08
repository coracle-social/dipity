<script lang="ts">
  import Tag from "@lucide/svelte/icons/tag"
  import X from "@lucide/svelte/icons/x"
  import {Button} from "$lib/components/ui/button"
  import * as DropdownMenu from "$lib/components/ui/dropdown-menu"
  import {topicLabel} from "$lib/kinds"

  let {
    topic,
    offered,
    onPick,
  }: {topic?: string; offered: string[]; onPick: (topic?: string) => void} = $props()
</script>

<!-- The only thing left to offer a board narrowed to one topic is the way back out. -->
{#if topic}
  <Button variant="secondary" size="sm" aria-label="Show every topic" onclick={() => onPick()}>
    <Tag />
    {topicLabel(topic)}
    <X />
  </Button>
{:else if offered.length > 0}
  <DropdownMenu.Root>
    <DropdownMenu.Trigger>
      {#snippet child({props})}
        <Button {...props} variant="ghost" size="sm">
          <Tag />
          Any topic
        </Button>
      {/snippet}
    </DropdownMenu.Trigger>
    <DropdownMenu.Content align="end">
      {#each offered as option (option)}
        <DropdownMenu.Item onSelect={() => onPick(option)}>{topicLabel(option)}</DropdownMenu.Item>
      {/each}
    </DropdownMenu.Content>
  </DropdownMenu.Root>
{/if}
