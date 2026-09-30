<script lang="ts">
  import type {Component, Snippet} from "svelte"

  // Grows into whatever height the screen leaves, so the message sits in the middle of it.
  let {icon: Icon, children}: {icon: Component; children: Snippet} = $props()
</script>

<div class="flex flex-1 flex-col items-center justify-center gap-4 px-6 py-16 text-center">
  <Icon class="size-12 text-muted-foreground" strokeWidth={1.5} />
  <p class="max-w-xs text-sm text-balance text-muted-foreground">
    {@render children()}
  </p>
</div>
