<script lang="ts">
  import ArrowLeft from "@lucide/svelte/icons/arrow-left"
  import Smartphone from "@lucide/svelte/icons/smartphone"
  import {Button} from "$lib/components/ui/button"
  import {nameOf, social} from "$lib/data/contacts"
  import {links} from "$lib/data/links"
  import {go} from "$lib/data/nav"
  import {npubOf, session} from "$lib/data/session"
  import {clear, compare, offer, step} from "$lib/data/transfer"

  const leave = () => {
    clear()
    go({at: "settings"})
  }

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

{#if $step.at === "idle"}
  <p class="max-w-prose text-sm text-pretty text-muted-foreground">
    Two phones can hold the same key, so both of them are you. Pair with your other phone first, the
    way you would pair with anybody, then pick it here.
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
            <p class="text-xs text-muted-foreground">in the room now</p>
          </div>
        </button>
      </li>
    {/each}
  </ul>

  {#if $links.length === 0}
    <p class="py-10 text-sm text-pretty text-muted-foreground">
      Nothing is in range. Open the app on the other phone and hold the two together — it turns up
      here once the two have recognised each other.
    </p>
  {/if}

  <p class="mt-2 text-xs text-pretty text-muted-foreground">
    Whoever you pick ends up holding your key, so pick your own phone and nobody else's.
  </p>
{:else if $step.at === "offering"}
  <p class="py-10 text-sm text-pretty text-muted-foreground">
    Asking the other phone. It has to be open and in somebody's hand.
  </p>
{:else if $step.at === "comparing"}
  <p class="max-w-prose text-sm text-pretty text-muted-foreground">
    Both phones are showing six digits. If they are the same six, the two phones are talking to each
    other and to nothing in between.
  </p>

  <p class="my-8 text-center font-mono text-4xl font-semibold tabular-nums">
    {digits($step.code)}
  </p>

  {#if $step.source}
    <p class="max-w-prose text-sm text-pretty text-muted-foreground">
      Say yes and that phone holds your key as well. It posts as you, and you cannot take it back.
    </p>
  {:else}
    <p class="max-w-prose text-sm text-pretty text-destructive">
      Say yes and this phone becomes the other one. What it published under its own key stays on it
      and stops being yours — the names you gave people, and what you kept.
    </p>
  {/if}

  <div class="mt-8 flex flex-col gap-2">
    <Button size="lg" onclick={() => compare(true)}>The digits match</Button>
    <Button variant="ghost" size="lg" onclick={() => compare(false)}>They do not</Button>
  </div>
{:else if $step.at === "waiting"}
  <p class="py-10 text-sm text-pretty text-muted-foreground">
    Waiting for the other phone. Whoever is holding it has the same question in front of them.
  </p>
{:else if $step.at === "sent"}
  <p class="py-10 text-sm text-pretty">That phone is you as well now.</p>

  <Button size="lg" onclick={leave}>Done</Button>
{:else if $step.at === "arrived"}
  <p class="pt-10 text-sm text-pretty">This phone is you now.</p>

  {#if $session.identity}
    <p class="mt-3 font-mono text-xs break-all text-muted-foreground">
      {npubOf($session.identity)}
    </p>
  {/if}

  <Button class="mt-8" size="lg" onclick={leave}>Done</Button>
{:else if $step.at === "refused"}
  <p class="py-10 text-sm text-pretty text-muted-foreground">
    Nothing moved. One of the two phones said no, or it went out of range.
  </p>

  <Button size="lg" onclick={leave}>Done</Button>
{:else}
  <p class="py-10 text-sm text-pretty text-destructive">{$step.why}</p>

  <Button size="lg" onclick={leave}>Done</Button>
{/if}
