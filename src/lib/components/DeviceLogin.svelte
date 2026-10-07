<script lang="ts">
  import ArrowLeft from "@lucide/svelte/icons/arrow-left"
  import BluetoothSearching from "@lucide/svelte/icons/bluetooth-searching"
  import Smartphone from "@lucide/svelte/icons/smartphone"
  import {Button} from "$lib/components/ui/button"
  import EmptyState from "$lib/components/EmptyState.svelte"
  import {nameOf, social} from "$lib/data/contacts"
  import {links} from "$lib/data/links"
  import {back, swap} from "$lib/data/nav"
  import {logOut, npubOf, session} from "$lib/data/session"
  import {clear, compare, offer, step} from "$lib/data/transfer"

  // The phone's own back button leaves without passing through here, so the ending goes on the way out.
  $effect(() => clear)

  // A stand-in identity has nowhere to go back to, so leaving gives it up.
  const leave = () => ($session.receiving ? logOut() : back())

  const done = () => back() || swap({at: "board"})

  /** Six digits, grouped the way a person reads a number off another screen. */
  const digits = (code: number) => {
    const padded = String(code).padStart(6, "0")

    return `${padded.slice(0, 3)} ${padded.slice(3)}`
  }
</script>

<header class="flex items-center gap-1 pt-4 pb-3">
  <Button variant="ghost" size="icon-sm" aria-label="Back" onclick={leave}>
    <ArrowLeft />
  </Button>
  <h1 class="text-2xl font-semibold">Another phone</h1>
</header>

{#if $step.at === "idle" && $session.receiving}
  <p class="max-w-prose text-sm text-pretty text-muted-foreground">
    On your other phone, open Settings, tap Use this key on another phone, and pick this one.
  </p>

  <EmptyState icon={BluetoothSearching}>Hold the two phones together.</EmptyState>
{:else if $step.at === "idle"}
  <p class="max-w-prose text-sm text-pretty text-muted-foreground">
    On your other phone, tap Log in with your other phone, then pick it here.
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
      Open Dipity on your other phone and hold the two together.
    </EmptyState>
  {/if}
{:else if $step.at === "offering"}
  <p class="py-10 text-sm text-pretty text-muted-foreground">
    Waiting for the other phone. Dipity needs to be open on it.
  </p>
{:else if $step.at === "comparing"}
  <p class="max-w-prose text-sm text-pretty text-muted-foreground">
    Check that both phones show the same six digits.
  </p>

  <p class="my-8 text-center font-mono text-4xl font-semibold tabular-nums">
    {digits($step.code)}
  </p>

  {#if $step.source}
    <p class="max-w-prose text-sm text-pretty text-muted-foreground">
      If they match, that phone gets your key for good.
    </p>
  {:else}
    <p class="max-w-prose text-sm text-pretty text-muted-foreground">
      If they match, this phone switches to your other phone's key.
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

  <Button size="lg" onclick={done}>Done</Button>
{:else if $step.at === "arrived"}
  <p class="pt-10 text-sm text-pretty">This phone now has your key.</p>

  {#if $session.identity}
    <p class="mt-3 font-mono text-xs break-all text-muted-foreground">
      {npubOf($session.identity)}
    </p>
  {/if}

  <Button class="mt-8" size="lg" onclick={done}>Done</Button>
{:else if $step.at === "refused"}
  <p class="py-10 text-sm text-pretty text-muted-foreground">
    No key was transferred. A phone declined, went out of range, or has already posted under its own
    key.
  </p>

  <Button size="lg" onclick={$session.receiving ? clear : done}>
    {$session.receiving ? "Try again" : "Done"}
  </Button>
{:else}
  <p class="py-10 text-sm text-pretty text-destructive">{$step.why}</p>

  <Button size="lg" onclick={$session.receiving ? clear : done}>
    {$session.receiving ? "Try again" : "Done"}
  </Button>
{/if}
