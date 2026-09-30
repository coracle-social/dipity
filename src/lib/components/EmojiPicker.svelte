<script lang="ts">
  import "emoji-picker-element"
  import emojiData from "emoji-picker-element-data/en/emojibase/data.json?url"
  import type {NativeEmoji} from "emoji-picker-element/shared"
  import * as Drawer from "$lib/components/ui/drawer"
  import {dismissable} from "$lib/data/nav"

  // The element carries its own search and data, and dispatches a custom event.
  let {
    open = $bindable(false),
    onPick,
  }: {
    open: boolean
    onPick: (emoji: string) => void
  } = $props()

  $effect(() => {
    if (open) return dismissable(() => (open = false))
  })

  let picker = $state<HTMLElement | undefined>(undefined)

  $effect(() => {
    const element = picker

    if (!element) return

    const chosen = (event: Event) => {
      const {unicode} = (event as CustomEvent<{unicode?: string; emoji: NativeEmoji}>).detail

      if (unicode) {
        onPick(unicode)
        open = false
      }
    }

    // The drawer captures the pointer on whatever it lands on, and shadow DOM retargets that to the picker, so the emoji under the finger never receives the click.
    const held = (event: PointerEvent) => event.stopPropagation()

    element.addEventListener("emoji-click", chosen)
    element.addEventListener("pointerdown", held)

    return () => {
      element.removeEventListener("emoji-click", chosen)
      element.removeEventListener("pointerdown", held)
    }
  })
</script>

<Drawer.Root bind:open>
  <Drawer.Content>
    <Drawer.Header>
      <Drawer.Title>How do you feel about it?</Drawer.Title>
    </Drawer.Header>

    <div class="flex justify-center px-4 pb-4">
      <emoji-picker bind:this={picker} data-source={emojiData}></emoji-picker>
    </div>
  </Drawer.Content>
</Drawer.Root>
