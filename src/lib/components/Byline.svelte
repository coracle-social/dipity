<script lang="ts">
  import {nameOf, type Social} from "$lib/data/contacts"

  let {social, pubkey}: {social: Social; pubkey: string} = $props()

  const named = $derived(nameOf(social, pubkey))
</script>

<span class="inline-flex min-w-0 items-baseline gap-1.5">
  <span class="truncate font-semibold">{named.name}</span>
  {#if named.according}
    <span class="truncate text-xs text-muted-foreground">according to {named.according}</span>
  {/if}
</span>
