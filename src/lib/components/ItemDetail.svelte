<script lang="ts">
  import ArrowLeft from "@lucide/svelte/icons/arrow-left"
  import {Button} from "$lib/components/ui/button"
  import Byline from "$lib/components/Byline.svelte"
  import ItemCard from "$lib/components/ItemCard.svelte"
  import Quoted from "$lib/components/Quoted.svelte"
  import {kept} from "$lib/data/bookmarks"
  import {social} from "$lib/data/contacts"
  import {
    detailOf,
    responses,
    saying,
    standingOf,
    sweptAt,
    warmthOf,
    type Item,
  } from "$lib/data/feed"
  import {go} from "$lib/data/nav"
  import {policy} from "$lib/data/policy"
  import {session} from "$lib/data/session"
  import {commentedOn} from "$lib/kinds"

  // Reached by id rather than handed down, so it survives the board re-reading under it.
  let {id, onBoost}: {id: string; onBoost: (item: Item) => void} = $props()

  const detail = $derived(detailOf(id))

  const keeping = $derived($kept.has(id))

  const now = Date.now() / 1000
</script>

<header class="flex items-center gap-1 pt-4 pb-3">
  <Button variant="ghost" size="icon-sm" aria-label="Back" onclick={() => go({at: "board"})}>
    <ArrowLeft />
  </Button>
  <h1 class="text-2xl font-semibold">In full</h1>
</header>

{#if $detail.item}
  {@const item = $detail.item}
  {@const swept = sweptAt(item, $session, $policy.retentionDays, keeping)}
  {@const answers = commentedOn(item.event)}
  {@const standing = {
    ...standingOf($responses, $saying, id),
    saying: $detail.comments.length,
  }}

  {#if answers}
    <h2 class="text-xs font-semibold tracking-widest text-muted-foreground uppercase">
      Commenting on
    </h2>
    <div class="mt-3 mb-4 rounded-lg bg-card px-4 py-3 shadow-sm">
      <Quoted
        id={answers}
        social={$social}
        absent="This device does not have what this is about." />
    </div>
  {/if}

  <ItemCard
    {item}
    social={$social}
    {standing}
    warmth={warmthOf(item, swept, now)}
    sweptAt={swept}
    session={$session}
    kept={keeping}
    {onBoost}
    detailed />

  {#if $detail.comments.length > 0}
    <h2 class="mt-6 text-xs font-semibold tracking-widest text-muted-foreground uppercase">
      What people are saying
    </h2>
  {/if}

  <ul class="mt-3 space-y-3">
    {#each $detail.comments as said (said.id)}
      <li class="rounded-lg bg-card px-4 py-3 shadow-sm">
        <div class="text-sm">
          {#if said.pubkey === $session.identity}
            <span class="font-semibold">You</span>
          {:else}
            <Byline social={$social} pubkey={said.pubkey} />
          {/if}
        </div>
        <p class="mt-1 text-sm text-pretty">{said.content}</p>
      </li>
    {/each}
  </ul>

  {#if $detail.comments.length === 0}
    <p class="py-6 text-sm text-pretty text-muted-foreground">
      Nobody has said anything about it yet.
    </p>
  {/if}
{:else}
  <p class="py-10 text-sm text-pretty text-muted-foreground">
    This device does not have that. Things go once they stop going around, unless you keep them.
  </p>
{/if}
