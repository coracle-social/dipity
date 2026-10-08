<script lang="ts">
  import Tag from "@lucide/svelte/icons/tag"
  import X from "@lucide/svelte/icons/x"
  import {Badge} from "$lib/components/ui/badge"
  import * as InputGroup from "$lib/components/ui/input-group"
  import {topicFrom, topicLabel} from "$lib/kinds"

  // The input takes anything typed and only suggests what it has seen, because topics are free text.
  let {
    selected,
    offered,
    label,
    onAdd,
    onRemove,
  }: {
    selected: string[]
    offered: string[]
    label: string
    onAdd: (topic: string) => void
    onRemove: (topic: string) => void
  } = $props()

  let typed = $state("")
  let focused = $state(false)

  const wanted = $derived(topicFrom(typed))

  const matches = $derived(
    offered
      .filter(topic => !selected.includes(topic) && (!wanted || topic.includes(wanted)))
      .slice(0, 6),
  )

  const fresh = $derived(wanted && !selected.includes(wanted) && !matches.includes(wanted))

  const add = (topic: string) => {
    onAdd(topic)
    typed = ""
  }

  const keyed = (event: KeyboardEvent) => {
    if (event.key === "Enter" && wanted) {
      event.preventDefault()

      if (!selected.includes(wanted)) add(wanted)
    }

    if (event.key === "Backspace" && !typed && selected.length > 0) onRemove(selected.at(-1)!)
  }

  // Keeps the input focused through a press on a suggestion, which would otherwise blur it first.
  const holdFocus = (event: PointerEvent) => event.preventDefault()
</script>

{#if selected.length > 0}
  <ul class="mb-2 flex flex-wrap gap-1.5">
    {#each selected as topic (topic)}
      <li>
        <Badge variant="secondary">
          {topicLabel(topic)}
          <button
            type="button"
            data-icon="inline-end"
            class="rounded-full text-muted-foreground hover:text-foreground"
            aria-label="Remove {topicLabel(topic)}"
            onclick={() => onRemove(topic)}>
            <X class="size-3" />
          </button>
        </Badge>
      </li>
    {/each}
  </ul>
{/if}

<InputGroup.Root>
  <InputGroup.Addon>
    <Tag />
  </InputGroup.Addon>
  <InputGroup.Input
    bind:value={typed}
    placeholder={label}
    aria-label={label}
    enterkeyhint="done"
    onkeydown={keyed}
    onfocus={() => (focused = true)}
    onblur={() => (focused = false)} />
</InputGroup.Root>

{#if focused && (fresh || matches.length > 0)}
  <ul class="mt-1 overflow-hidden rounded-md border border-border bg-popover text-sm shadow-xs">
    {#if fresh && wanted}
      <li>
        <button
          type="button"
          class="w-full px-3 py-2 text-left hover:bg-muted"
          onpointerdown={holdFocus}
          onclick={() => add(wanted)}>
          Add {topicLabel(wanted)}
        </button>
      </li>
    {/if}
    {#each matches as topic (topic)}
      <li>
        <button
          type="button"
          class="w-full px-3 py-2 text-left hover:bg-muted"
          onpointerdown={holdFocus}
          onclick={() => add(topic)}>
          {topicLabel(topic)}
        </button>
      </li>
    {/each}
  </ul>
{/if}
