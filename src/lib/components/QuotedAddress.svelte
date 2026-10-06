<script lang="ts">
  import Quoted from "$lib/components/Quoted.svelte"
  import type {Social} from "$lib/data/contacts"
  import {heldAtAddress} from "$lib/data/feed"

  // A quoted address, drawn as whatever currently holds it.
  let {
    kind,
    pubkey,
    identifier,
    social,
    absent,
  }: {
    kind: number
    pubkey: string
    identifier: string
    social: Social
    absent: string
  } = $props()

  const held = $derived(heldAtAddress(kind, pubkey, identifier))
</script>

{#if $held}
  <Quoted id={$held} {social} {absent} />
{:else if $held === null}
  <p class="border-l-2 border-border pl-3 text-sm text-muted-foreground">{absent}</p>
{/if}
