<script lang="ts">
  import ArrowLeft from "@lucide/svelte/icons/arrow-left"
  import BluetoothSearching from "@lucide/svelte/icons/bluetooth-searching"
  import Smartphone from "@lucide/svelte/icons/smartphone"
  import {Button} from "$lib/components/ui/button"
  import EmptyState from "$lib/components/EmptyState.svelte"
  import {nameOf, social} from "$lib/data/contacts"
  import {links} from "$lib/data/links"
  import {back} from "$lib/data/nav"
  import {npubOf, session} from "$lib/data/session"
  import {clear, compare, offer, step} from "$lib/data/transfer"

  // The phone's own back button leaves without passing through here, so the ending goes on the way out.
  $effect(() => clear)

  /** Six digits, grouped the way a person reads a number off another screen. */
  const digits = (code: number) => {
    const padded = String(code).padStart(6, "0")

    return `${padded.slice(0, 3)} ${padded.slice(3)}`
  }
</script>

<header class="flex items-center gap-1 pt-4 pb-3">
  <Button variant="ghost" size="icon-sm" aria-label="Back" onclick={back}>
    <ArrowLeft />
  </Button>
  <h1 class="text-2xl font-semibold">Another phone</h1>
</header>

{#if $step.at === "idle"}
  <p class="max-w-prose text-sm text-pretty text-muted-foreground">
    Put your key on another phone so both phones post as you. Pair with the other phone first, then
    pick it here.
  </p>

  <ul class="mt-6 space-y-2">
    {#each $links as live (live.link)}
      <li>
        <button
          type="button"
          class="flex w-full items-center gap-3 rounded-lg bg-card px-4 py-3 text-left shadow-sm
                 transition-shadow hover:shadow-md"
          onclick={() => offer(live.link)}>
          <Smartphone class="size-5 flex-none text-muted-foreground" />
          <div class="min-w-0 flex-1">
            <p class="truncate font-semibold">{nameOf($social, live.pubkey).name}</p>
            <p class="text-xs text-muted-foreground">nearby</p>
          </div>
        </button>
      </li>
    {/each}
  </ul>

  {#if $links.length === 0}
    <EmptyState icon={BluetoothSearching}>
      No phones in range. Open Dipity on the other phone and hold the two phones together. It
      appears here once they connect.
    </EmptyState>
  {/if}

  <p class="mt-2 text-xs text-pretty text-muted-foreground">
    The phone you pick gets your key. Only pick your own phone.
  </p>
{:else if $step.at === "offering"}
  <p class="py-10 text-sm text-pretty text-muted-foreground">
    Waiting for the other phone. Dipity needs to be open on it.
  </p>
{:else if $step.at === "comparing"}
  <p class="max-w-prose text-sm text-pretty text-muted-foreground">
    Check that both phones show the same six digits. If they match, the phones are connected
    directly to each other.
  </p>

  <p class="my-8 text-center font-mono text-4xl font-semibold tabular-nums">
    {digits($step.code)}
  </p>

  {#if $step.source}
    <p class="max-w-prose text-sm text-pretty text-muted-foreground">
      If the digits match, that phone gets your key and can post as you. This can't be undone.
    </p>
  {:else}
    <p class="max-w-prose text-sm text-pretty text-destructive">
      If the digits match, this phone switches to the other phone's key. Everything on this phone,
      including the names you gave people and your bookmarks, is erased.
    </p>
  {/if}

  <div class="mt-8 flex flex-col gap-2">
    <Button size="lg" onclick={() => compare(true)}>The digits match</Button>
    <Button variant="ghost" size="lg" onclick={() => compare(false)}>They do not</Button>
  </div>
{:else if $step.at === "waiting"}
  <p class="py-10 text-sm text-pretty text-muted-foreground">
    Waiting for the other phone to confirm.
  </p>
{:else if $step.at === "sent"}
  <p class="py-10 text-sm text-pretty">That phone now has your key.</p>

  <Button size="lg" onclick={back}>Done</Button>
{:else if $step.at === "arrived"}
  <p class="pt-10 text-sm text-pretty">This phone now has your key.</p>

  {#if $session.identity}
    <p class="mt-3 font-mono text-xs break-all text-muted-foreground">
      {npubOf($session.identity)}
    </p>
  {/if}

  <Button class="mt-8" size="lg" onclick={back}>Done</Button>
{:else if $step.at === "refused"}
  <p class="py-10 text-sm text-pretty text-muted-foreground">
    No key was transferred. One of the phones declined, or they went out of range.
  </p>

  <Button size="lg" onclick={back}>Done</Button>
{:else}
  <p class="py-10 text-sm text-pretty text-destructive">{$step.why}</p>

  <Button size="lg" onclick={back}>Done</Button>
{/if}
