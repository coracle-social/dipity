<script lang="ts">
  // The six digits two phones compare, grouped the way a person reads a number off another screen.
  let {code}: {code: number} = $props()

  // Five palette colors, outlined for 0 to 4 and filled for 5 to 9, so no two digits look alike.
  const looks = [
    "bg-card text-chart-1",
    "bg-card text-chart-3",
    "bg-card text-chart-4",
    "bg-card text-chart-5",
    "bg-card text-chart-2",
    "bg-chart-1 text-card",
    "bg-chart-3 text-card",
    "bg-chart-4 text-card",
    "bg-chart-5 text-card",
    "bg-chart-2 text-card",
  ]

  const padded = $derived(String(code).padStart(6, "0"))

  const groups = $derived.by(() => {
    const digits = [...padded].map(Number)

    return [digits.slice(0, 3), digits.slice(3)]
  })
</script>

<div class="flex items-center justify-center gap-4" aria-label={padded}>
  {#each groups as group, index (index)}
    <div class="flex gap-1.5">
      {#each group as digit, place (place)}
        <span
          class="flex size-11 items-center justify-center rounded-xl font-mono text-3xl font-bold tabular-nums shadow-sm {looks[
            digit
          ]}">
          {digit}
        </span>
      {/each}
    </div>
  {/each}
</div>
