<script lang="ts">
  import ArrowLeft from "@lucide/svelte/icons/arrow-left"
  import BluetoothSearching from "@lucide/svelte/icons/bluetooth-searching"
  import Smartphone from "@lucide/svelte/icons/smartphone"
  import {Button} from "$lib/components/ui/button"
  import Digits from "$lib/components/Digits.svelte"
  import EmptyState from "$lib/components/EmptyState.svelte"
  import {nameOf, social} from "$lib/data/contacts"
  import {links} from "$lib/data/links"
  import {back, swap} from "$lib/data/nav"
  import {logOut, session} from "$lib/data/session"
  import {clear, compare, offer, step} from "$lib/data/transfer"

  // The ending goes on the way out, because the phone's own back button leaves without passing through here.
  $effect(() => clear)

  // Leaving gives up a stand-in identity, which has nowhere to go back to.
  const leave = () => ($session.receiving ? logOut() : back())

  const done = () => back() || swap({at: "board"})
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

  <div class="my-8">
    <Digits code={$step.code} />
  </div>

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
    <Button variant="ghost" size="lg" onclick={() => compare(false)}>Cancel</Button>
  </div>
{:else if $step.at === "waiting"}
  <p class="py-10 text-sm text-pretty text-muted-foreground">
    Waiting for the other phone to confirm.
  </p>
{:else if $step.at === "sent" || $step.at === "arrived"}
  <p class="py-10 text-sm text-pretty">Your key has successfully been transfered!</p>

  <Button size="lg" onclick={done}>Done</Button>
{:else if $step.at === "refused"}
  <p class="py-10 text-sm text-pretty text-muted-foreground">
    No key was transferred. The device declined, went out of range, or has already posted under its
    own key.
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
