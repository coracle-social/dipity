<script lang="ts">
  import ChevronRight from "@lucide/svelte/icons/chevron-right"
  import Clock from "@lucide/svelte/icons/clock"
  import MapPin from "@lucide/svelte/icons/map-pin"
  import {EVENT_DATE, EVENT_TIME, LONG_FORM, POLL} from "@welshman/util"
  import {nameOf, type Social} from "$lib/data/contacts"
  import {heldEventOf} from "$lib/data/feed"
  import {go} from "$lib/data/nav"
  import {article, calendarFor, categoryOf, occasionOf, poll, summaryOf} from "$lib/kinds"

  // A quoted thing in brief, its kind showing, and nothing in it to press but the way to it.
  let {
    id,
    social,
    absent,
  }: {
    id: string
    social: Social
    /** What to say when the thing this stands for never reached this device. */
    absent: string
  } = $props()

  const held = $derived(heldEventOf(id))

  /** How many options a quoted poll shows before saying how many more there are. */
  const OPTIONS_SHOWN = 3

  /** What one kind shows in brief, and what it says to draw the user through to the rest. */
  type Preview = {
    category: ReturnType<typeof categoryOf>
    by: string
    title?: string
    text?: string
    options?: {id: string; label: string}[]
    when?: string
    where?: string
    prompt?: string
  }

  const preview = $derived.by((): Preview | undefined => {
    const event = $held

    if (!event) return undefined

    const category = categoryOf(event.kind)
    const base = {category, by: nameOf(social, event.pubkey).name, text: summaryOf(event)}

    if (event.kind === POLL) {
      const asked = poll.reader(event).parse()

      return {...base, title: asked.title(), options: asked.options(), prompt: "Open to vote"}
    }

    if (event.kind === EVENT_DATE || event.kind === EVENT_TIME) {
      const entry = calendarFor(event.kind).reader(event).parse()
      const start = occasionOf(event)
      const when = start
        ? new Date(start.at * 1000).toLocaleString(
            undefined,
            start.allDay
              ? {weekday: "short", day: "numeric", month: "short"}
              : {weekday: "short", hour: "numeric", minute: "2-digit"},
          )
        : undefined

      return {...base, title: entry.title(), when, where: entry.location(), prompt: "See details"}
    }

    if (event.kind === LONG_FORM) {
      const written = article.reader(event).parse()

      return {...base, title: written.title(), text: written.summary(), prompt: "Read it"}
    }

    return base
  })
</script>

{#if $held === undefined}
  <p class="border-l-2 border-border pl-3 text-sm text-muted-foreground">Loading…</p>
{:else if preview}
  {@const Mark = preview.category.icon}
  <!-- Drawn over the card's own tap target, so this opens what it stands for. -->
  <button
    type="button"
    class="relative block w-full rounded-lg border border-border bg-muted/40 px-3 py-2 text-left"
    onclick={() => go({at: "item", id})}>
    <span class="flex items-center gap-1.5 text-xs text-muted-foreground">
      <Mark class="size-3.5 flex-none" />
      <span class="min-w-0 flex-1 truncate">{preview.category.noun} · {preview.by}</span>
      <ChevronRight class="size-3.5 flex-none" />
    </span>

    {#if preview.title}
      <span class="mt-1 block text-sm font-semibold text-pretty">{preview.title}</span>
    {/if}

    {#if preview.options}
      <span class="mt-1.5 flex flex-wrap gap-1">
        {#each preview.options.slice(0, OPTIONS_SHOWN) as option (option.id)}
          <span class="rounded-md border border-border px-2 py-0.5 text-xs">{option.label}</span>
        {/each}
        {#if preview.options.length > OPTIONS_SHOWN}
          <span class="px-1 py-0.5 text-xs text-muted-foreground">
            +{preview.options.length - OPTIONS_SHOWN} more
          </span>
        {/if}
      </span>
    {:else if preview.when || preview.where}
      <span class="mt-1 flex flex-col gap-0.5 text-xs text-muted-foreground">
        {#if preview.when}
          <span class="flex items-center gap-1.5"><Clock class="size-3.5" />{preview.when}</span>
        {/if}
        {#if preview.where}
          <span class="flex items-center gap-1.5"><MapPin class="size-3.5" />{preview.where}</span>
        {/if}
      </span>
    {:else if preview.text}
      <span class="mt-1 line-clamp-3 block text-sm text-pretty">{preview.text}</span>
    {/if}

    {#if preview.prompt}
      <span class="mt-1.5 block text-xs font-medium text-secondary-accent">{preview.prompt}</span>
    {/if}
  </button>
{:else}
  <p class="border-l-2 border-border pl-3 text-sm text-muted-foreground">{absent}</p>
{/if}
