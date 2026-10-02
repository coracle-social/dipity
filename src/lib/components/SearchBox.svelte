<script lang="ts">
  import Search from "@lucide/svelte/icons/search"
  import X from "@lucide/svelte/icons/x"
  import * as InputGroup from "$lib/components/ui/input-group"

  // One search box for every screen that searches, so they look and clear alike.
  let {
    value = $bindable(""),
    label,
    class: className = "",
  }: {value: string; label: string; class?: string} = $props()
</script>

<InputGroup.Root class={className}>
  <InputGroup.Addon>
    <Search />
  </InputGroup.Addon>
  <!-- Text rather than search: the browser draws its own clear button on a search field, beside ours. -->
  <InputGroup.Input
    type="text"
    enterkeyhint="search"
    placeholder={label}
    aria-label={label}
    bind:value />
  {#if value}
    <InputGroup.Addon align="inline-end">
      <InputGroup.Button size="icon-xs" aria-label="Clear the search" onclick={() => (value = "")}>
        <X />
      </InputGroup.Button>
    </InputGroup.Addon>
  {/if}
</InputGroup.Root>
