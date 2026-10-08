<script lang="ts">
  import Clock from "@lucide/svelte/icons/clock"
  import MapPin from "@lucide/svelte/icons/map-pin"
  import {
    EVENT_DATE,
    EVENT_TIME,
    GENERIC_REPOST,
    LONG_FORM,
    PICTURE_NOTE,
    POLL,
    REPOST,
  } from "@welshman/util"
  import Photo from "$lib/components/Photo.svelte"
  import Poll from "$lib/components/Poll.svelte"
  import Prose from "$lib/components/Prose.svelte"
  import Quoted from "$lib/components/Quoted.svelte"
  import {article, boostedBy, calendarFor, commentedOn, occasionOf} from "$lib/kinds"
  import type {Item, Standing} from "$lib/data/feed"
  import type {Social} from "$lib/data/contacts"
  import type {Session} from "$lib/data/session"

  let {
    item,
    social,
    standing,
    session,
    detailed = false,
  }: {
    item: Item
    social: Social
    standing: Standing
    session: Session
    /** Whether the whole of it is worth drawing, rather than enough to choose by. */
    detailed?: boolean
  } = $props()

  const {event} = $derived(item)

  const firstLine = (content: string) =>
    content
      .split("\n")
      .map(line => line.trim())
      .find(Boolean) ?? ""

  const occasion = $derived.by(() => {
    if (event.kind !== EVENT_DATE && event.kind !== EVENT_TIME) return undefined

    const reader = calendarFor(event.kind).reader(event).parse()
    const start = occasionOf(event)
    const when = start
      ? new Date(start.at * 1000).toLocaleString(
          undefined,
          start.allDay
            ? {weekday: "long", day: "numeric", month: "long"}
            : {weekday: "long", hour: "numeric", minute: "2-digit"},
        )
      : undefined

    return {title: reader.title(), where: reader.location(), when}
  })

  const written = $derived(event.kind === LONG_FORM ? article.reader(event).parse() : undefined)

  // The card does not head the parent, because the page it opens on does that with its own section.
  const about = $derived(detailed ? undefined : commentedOn(event))
</script>

{#if event.kind === REPOST || event.kind === GENERIC_REPOST}
  {@const id = boostedBy(event)}
  {#if id}
    <Quoted {id} {social} absent="The original post isn't on this phone." />
  {/if}
{:else if about}
  <!-- A comment's own words come first, because it reaches people who never got its subject. -->
  <Prose {event} />
  <div class="mt-2">
    <Quoted id={about} {social} absent="The post this replies to isn't on this phone." />
  </div>
{:else if occasion}
  <div class="rounded-md border border-secondary-accent/40 px-3 py-2">
    <h4 class="text-base font-semibold text-pretty">{occasion.title ?? "An event"}</h4>
    {#if occasion.when}
      <p class="mt-1 flex items-center gap-1.5 text-sm text-secondary-accent">
        <Clock class="size-3.5" />
        {occasion.when}
      </p>
    {/if}
    {#if occasion.where}
      <p class="flex items-center gap-1.5 text-sm text-muted-foreground">
        <MapPin class="size-3.5" />
        {occasion.where}
      </p>
    {/if}
    {#if detailed && event.content}
      <Prose class="mt-2" {event} />
    {/if}
  </div>
{:else if event.kind === PICTURE_NOTE}
  <Photo {item} {social} {session} {detailed} />
  {#if event.content.trim()}
    <Prose class="mt-2" {event} />
  {/if}
{:else if event.kind === POLL}
  <Poll {item} {standing} {session} />
{:else if written}
  <h4 class="text-lg leading-snug font-semibold text-pretty">{written.title() ?? "Untitled"}</h4>
  {#if written.summary()}
    <p class="mt-1 text-sm text-pretty text-muted-foreground italic">{written.summary()}</p>
  {/if}
  {#if detailed}
    <Prose class="mt-2" gap="mt-2" {event} />
  {:else}
    <p class="mt-1 line-clamp-2 text-sm text-pretty">{firstLine(event.content)}</p>
  {/if}
{:else}
  <Prose {event} />
{/if}
