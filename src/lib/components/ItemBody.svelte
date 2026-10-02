<script lang="ts">
  import Clock from "@lucide/svelte/icons/clock"
  import MapPin from "@lucide/svelte/icons/map-pin"
  import {EVENT_DATE, EVENT_TIME, GENERIC_REPOST, LONG_FORM, POLL, REPOST} from "@welshman/util"
  import Poll from "$lib/components/Poll.svelte"
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

  const paragraphs = (content: string) =>
    content
      .split("\n")
      .map(line => line.trim())
      .filter(Boolean)

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

  // The page it opens on heads the parent with its own section, so the card does not.
  const about = $derived(detailed ? undefined : commentedOn(event))
</script>

{#if event.kind === REPOST || event.kind === GENERIC_REPOST}
  {@const id = boostedBy(event)}
  {#if id}
    <Quoted {id} {social} absent="This phone does not have what was passed on." />
  {/if}
{:else if about}
  <!-- A comment reaches people who never got its subject, so its own words come first. -->
  {#each paragraphs(event.content) as line, index (index)}
    <p class="text-sm text-pretty {index ? 'mt-1.5' : ''}">{line}</p>
  {/each}
  <div class="mt-2">
    <Quoted id={about} {social} absent="This phone does not have what this is about." />
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
      <p class="mt-2 text-sm text-pretty">{event.content}</p>
    {/if}
  </div>
{:else if event.kind === POLL}
  <Poll {item} {standing} {session} />
{:else if written}
  <h4 class="text-lg leading-snug font-semibold text-pretty">{written.title() ?? "Untitled"}</h4>
  {#if written.summary()}
    <p class="mt-1 text-sm text-pretty text-muted-foreground italic">{written.summary()}</p>
  {/if}
  {#if detailed}
    {#each paragraphs(event.content) as line, index (index)}
      <p class="mt-2 text-sm text-pretty">{line}</p>
    {/each}
  {:else}
    <p class="mt-1 line-clamp-2 text-sm text-pretty">{paragraphs(event.content)[0] ?? ""}</p>
  {/if}
{:else}
  {#each paragraphs(event.content) as line, index (index)}
    <p class="text-sm text-pretty {index ? 'mt-1.5' : ''}">{line}</p>
  {/each}
{/if}
