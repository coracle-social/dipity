<script lang="ts">
  import Bookmark from "@lucide/svelte/icons/bookmark"
  import Ellipsis from "@lucide/svelte/icons/ellipsis"
  import Flag from "@lucide/svelte/icons/flag"
  import TriangleAlert from "@lucide/svelte/icons/triangle-alert"
  import UserRound from "@lucide/svelte/icons/user-round"
  import MessageCircle from "@lucide/svelte/icons/message-circle"
  import Undo from "@lucide/svelte/icons/undo-2"
  import Smile from "@lucide/svelte/icons/smile"
  import Trash from "@lucide/svelte/icons/trash-2"
  import VolumeOff from "@lucide/svelte/icons/volume-off"
  import {badgeVariants} from "$lib/components/ui/badge"
  import {Button} from "$lib/components/ui/button"
  import * as DropdownMenu from "$lib/components/ui/dropdown-menu"
  import Byline from "$lib/components/Byline.svelte"
  import Cooling from "$lib/components/Cooling.svelte"
  import EmojiPicker from "$lib/components/EmojiPicker.svelte"
  import FlagDetails from "$lib/components/FlagDetails.svelte"
  import FlagPicker from "$lib/components/FlagPicker.svelte"
  import ItemBody from "$lib/components/ItemBody.svelte"
  import Reactions from "$lib/components/Reactions.svelte"
  import {toggleBookmark} from "$lib/data/bookmarks"
  import {nameOf, setTopicMuted, type Social} from "$lib/data/contacts"
  import {
    flaggedIds,
    opensId,
    react,
    showTopic,
    type Item,
    type Standing,
    type Warmth,
  } from "$lib/data/feed"
  import {go} from "$lib/data/nav"
  import type {Session} from "$lib/data/session"
  import {restore, trash} from "$lib/data/trash"
  import {dismissFlags, flags, flagsOn, reasonLabel} from "$lib/data/reports"
  import {categoryOf, topicLabel, topicOf} from "$lib/kinds"

  let {
    item,
    social,
    standing,
    warmth,
    sweptAt,
    session,
    bookmarked,
    trashed = false,
    retracted = false,
    onBoost,
    detailed = false,
  }: {
    item: Item
    social: Social
    standing: Standing
    warmth: Warmth
    sweptAt?: number
    session: Session
    /** Whether the user bookmarked this, which also spares it from the sweep. */
    bookmarked: boolean
    /** Whether this is in the trash, where the button puts it back rather than throwing it out. */
    trashed?: boolean
    /** Whether its author retracted it, which leaves somebody else's nothing to put back. */
    retracted?: boolean
    onBoost: (item: Item) => void
    /** Whether this is the card the detail page is about, which opens nothing. */
    detailed?: boolean
  } = $props()

  let picking = $state(false)
  let reading = $state(false)
  let flagging = $state(false)
  let detailing = $state(false)

  const mine = $derived(item.event.pubkey === session.identity)

  // Its author handed it over in person, or a neighbour carried it in with a proof.
  const hops = $derived(mine || item.from.includes(item.event.pubkey) ? 1 : 2)

  const category = $derived(categoryOf(item.event.kind))

  const topic = $derived(topicOf(item.event))

  const opens = $derived(opensId(item))

  const Mark = $derived(category.icon)

  const flagged = $derived(flagsOn($flags, flaggedIds(item.event)))

  const arrived = $derived(
    new Date(item.seenAt * 1000).toLocaleString(undefined, {
      weekday: "short",
      hour: "numeric",
      minute: "2-digit",
    }),
  )

  const carrier = $derived(
    item.from[0] && item.from[0] !== session.identity
      ? nameOf(social, item.from[0]).name
      : undefined,
  )

  const passedOn = $derived(standing.response.boosts.length)

  const tally = $derived(
    standing.response.reactions.reduce<Record<string, number>>(
      (counts, {emoji}) => ({...counts, [emoji]: (counts[emoji] ?? 0) + 1}),
      {},
    ),
  )

  const sent = $derived(
    new Set(
      standing.response.reactions
        .filter(({pubkey}) => pubkey === session.identity)
        .map(({emoji}) => emoji),
    ),
  )
</script>

{#snippet said()}
  <header class="flex items-baseline justify-between gap-3 text-sm">
    {#if mine}
      <span class="font-semibold">You</span>
    {:else}
      <Byline {social} pubkey={item.event.pubkey} />
    {/if}
    <span class="flex flex-none items-center gap-2">
      {#if topic}
        <button
          type="button"
          class={badgeVariants({variant: "secondary"})}
          onclick={() => {
            showTopic(topic)
            go({at: "board"})
          }}>
          {topicLabel(topic)}
        </button>
      {/if}
      <Cooling {warmth} {sweptAt} />
      <Mark class="size-4 text-muted-foreground" aria-label={category.noun} />
    </span>
  </header>

  {#if flagged.length > 0}
    <div class="relative mt-2 flex items-start gap-3 rounded-md bg-muted p-3">
      <TriangleAlert class="mt-0.5 size-4 flex-none text-destructive" />
      <div class="min-w-0 flex-1">
        <p class="text-sm font-medium">Flagged by people you trust</p>
        <p class="mt-0.5 text-xs text-muted-foreground">
          {flagged
            .map(({reason, count}) =>
              count > 1 ? `${reasonLabel(reason)} (${count})` : reasonLabel(reason),
            )
            .join(" · ")}
        </p>
        <div class="mt-2 flex gap-2">
          <Button variant="outline" size="sm" onclick={() => (detailing = true)}>Details</Button>
          <Button variant="ghost" size="sm" onclick={() => dismissFlags(flaggedIds(item.event))}>
            Show anyway
          </Button>
        </div>
      </div>
    </div>
  {:else}
    <div class="mt-2">
      <ItemBody {item} {social} {standing} {session} {detailed} />
    </div>
  {/if}
{/snippet}

<article class="relative rounded-lg bg-card shadow-sm">
  {#if !detailed}
    <!-- A control drawn on the card is still its own tap, because the card opens the thing from underneath. -->
    <button
      type="button"
      class="absolute inset-0 rounded-lg"
      aria-label="Open this"
      onclick={() => go({at: "item", id: opens})}></button>
  {/if}

  <div class="px-4 pt-3">
    {@render said()}
  </div>

  {#if Object.keys(tally).length > 0}
    <div class="relative flex flex-wrap gap-1 px-4 pt-3">
      {#each Object.entries(tally) as [emoji, count] (emoji)}
        <button
          type="button"
          class="rounded-full border px-2 py-0.5 text-xs transition-colors
                 {sent.has(emoji)
            ? 'border-secondary-accent text-secondary-accent'
            : 'border-border text-muted-foreground'}"
          aria-label="Who reacted"
          onclick={() => (reading = true)}>
          {emoji}
          {count}
        </button>
      {/each}
    </div>
  {/if}

  <footer class="relative mt-3 flex items-center justify-between gap-2 px-4 pb-3">
    <p class="min-w-0 truncate text-xs text-muted-foreground">
      {arrived}{carrier ? ` · from ${carrier}` : ""}{hops === 2 ? " · two hops" : ""}{passedOn > 0
        ? ` · ${passedOn} passed on`
        : ""}{standing.saying > 0 ? ` · ${standing.saying} saying` : ""}{retracted
        ? mine
          ? " · retracted"
          : " · retracted by author"
        : ""}
    </p>

    <div class="flex flex-none items-center gap-0.5">
      <Button variant="ghost" size="icon-sm" aria-label="React" onclick={() => (picking = true)}>
        <Smile />
      </Button>
      <Button variant="ghost" size="icon-sm" aria-label="Pass it on" onclick={() => onBoost(item)}>
        <MessageCircle />
      </Button>
      <Button
        variant="ghost"
        size="icon-sm"
        aria-label={bookmarked ? "Remove bookmark" : "Bookmark this"}
        aria-pressed={bookmarked}
        class={bookmarked ? "text-primary" : undefined}
        onclick={() => toggleBookmark(item)}>
        <Bookmark />
      </Button>
      {#if trashed}
        <Button
          variant="ghost"
          size="icon-sm"
          aria-label={retracted && !mine ? "Retracted by its author" : "Put back"}
          disabled={retracted && !mine}
          onclick={() => restore(item)}>
          <Undo />
        </Button>
      {:else}
        <Button
          variant="ghost"
          size="icon-sm"
          aria-label="Move to trash"
          onclick={() => trash(item)}>
          <Trash />
        </Button>
      {/if}
      <DropdownMenu.Root>
        <!-- The menu is empty on the user's own post when it has no topic. -->
        <DropdownMenu.Trigger disabled={mine && !topic}>
          {#snippet child({props})}
            <Button
              {...props}
              variant="ghost"
              size="icon-sm"
              aria-label="More for this post"
              disabled={mine && !topic}>
              <Ellipsis />
            </Button>
          {/snippet}
        </DropdownMenu.Trigger>
        <DropdownMenu.Content align="end">
          {#if !mine}
            <DropdownMenu.Item onSelect={() => go({at: "contact", pubkey: item.event.pubkey})}>
              <UserRound />
              View author
            </DropdownMenu.Item>
          {/if}
          {#if topic}
            <DropdownMenu.Item onSelect={() => setTopicMuted(topic, true)}>
              <VolumeOff />
              Mute {topicLabel(topic)}
            </DropdownMenu.Item>
          {/if}
          {#if !mine}
            <DropdownMenu.Item onSelect={() => (flagging = true)}>
              <Flag />
              Flag activity
            </DropdownMenu.Item>
          {/if}
        </DropdownMenu.Content>
      </DropdownMenu.Root>
    </div>
  </footer>
</article>

<EmojiPicker bind:open={picking} onPick={emoji => react(item, emoji)} />
<FlagPicker bind:open={flagging} {item} />
<FlagDetails bind:open={detailing} ids={flaggedIds(item.event)} {social} {session} />
<Reactions bind:open={reading} response={standing.response} {social} {session} />
