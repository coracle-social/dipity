<script lang="ts">
  import type {Arrival, Warmth} from "$lib/data/arrivals"
  import Postmark from "$lib/components/Postmark.svelte"

  let {arrival, warmth, sweptAt}: {arrival: Arrival; warmth: Warmth; sweptAt?: number} = $props()

  const lines = $derived(
    arrival.content
      .split("\n")
      .map(line => line.trim())
      .filter(Boolean),
  )

  const surfaces: Record<Warmth, string> = {
    warm: "bg-card shadow-md",
    fading: "bg-card shadow-sm",
    cold: "bg-muted",
    kept: "bg-card shadow-sm",
  }

  const day = (at: number) => {
    const days = Math.round((at * 1000 - Date.now()) / 86_400_000)

    if (days > 6) {
      return `in ${days} days`
    } else {
      return new Date(at * 1000).toLocaleDateString(undefined, {weekday: "long"})
    }
  }

  const stamped = (at: number) => {
    const arrived = new Date(at * 1000)

    if (arrived.toDateString() === new Date().toDateString()) {
      return arrived.toLocaleTimeString(undefined, {timeStyle: "short"})
    } else {
      return arrived.toLocaleDateString(undefined, {weekday: "short"})
    }
  }

  const byline = $derived.by(() => {
    const who = arrival.pubkey.slice(0, 8)

    if (sweptAt && warmth !== "warm") {
      return `${who} · ${warmth === "fading" ? "fading" : "cooling"}, swept ${day(sweptAt)}`
    } else {
      return [who, ...lines.slice(1)].join(" · ")
    }
  })
</script>

<article class="flex items-start gap-3 rounded-lg px-4 py-3 {surfaces[warmth]}">
  <div class="min-w-0 flex-1">
    <h4
      class="text-lg leading-tight font-semibold text-pretty
             {warmth === 'cold' ? 'text-muted-foreground' : 'text-card-foreground'}">
      {lines[0]}
    </h4>
    <p class="mt-1.5 truncate text-sm text-muted-foreground">{byline}</p>
  </div>
  {#if arrival.from.length}
    <Postmark where={arrival.from[0].slice(0, 6)} when={stamped(arrival.seenAt)} {warmth} />
  {/if}
</article>
