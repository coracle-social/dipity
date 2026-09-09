<script lang="ts">
  import {describeDay, hoursOf, recent, sweptAt, today, warmthOf} from "$lib/data/arrivals"
  import {session} from "$lib/data/session"
  import ArrivalCard from "$lib/components/ArrivalCard.svelte"
  import Forecast from "$lib/components/Forecast.svelte"

  let now = $state(Date.now() / 1000)

  $effect(() => {
    const tick = setInterval(() => (now = Date.now() / 1000), 60_000)

    return () => clearInterval(tick)
  })

  const clock = $derived(
    new Date(now * 1000).toLocaleString(undefined, {
      weekday: "long",
      hour: "numeric",
      minute: "2-digit",
    }),
  )

  const hours = $derived(hoursOf($today, now))

  const day = $derived(describeDay($today, hours, now))
</script>

<div class="min-h-svh bg-background pb-safe-b">
  <div class="mx-auto max-w-2xl px-5 pt-safe-t pb-10">
    <header class="flex items-baseline justify-between py-4">
      <span class="text-lg font-semibold">Dip</span>
      <span class="text-xs text-muted-foreground">{clock}</span>
    </header>

    <h1 class="text-3xl leading-tight font-semibold text-balance">{day.headline}</h1>
    <p class="mt-2 max-w-[32ch] text-sm text-pretty text-muted-foreground">{day.detail}</p>

    <div class="mt-6">
      <Forecast {hours} />
    </div>

    <h2 class="mt-8 text-xs font-semibold tracking-widest text-muted-foreground uppercase">
      What arrived
    </h2>

    <div class="mt-3 space-y-3">
      {#each $recent as arrival (arrival.id)}
        {@const swept = sweptAt(arrival, $session)}
        <ArrivalCard {arrival} warmth={warmthOf(arrival, swept, now)} sweptAt={swept} />
      {/each}
    </div>

    {#if day.quiet}
      <p class="py-8 text-sm text-pretty text-muted-foreground">
        It picks up when you're around people.
      </p>
    {/if}
  </div>
</div>
