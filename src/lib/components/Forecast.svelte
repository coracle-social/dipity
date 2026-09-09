<script lang="ts">
  import type {Hour} from "$lib/data/arrivals"

  let {hours}: {hours: Hour[]} = $props()

  const busiest = $derived(Math.max(1, ...hours.map(hour => hour.count)))

  const fill = (count: number) => {
    const share = count / busiest

    if (count) {
      if (share > 0.74) return "bg-primary"

      return share > 0.39 ? "bg-primary/70" : "bg-primary/40"
    } else {
      return "bg-muted"
    }
  }

  const height = (count: number) => Math.max(5, Math.round((count / busiest) * 100))

  const hourOf = (at: number) =>
    new Date(at * 1000).toLocaleTimeString(undefined, {hour: "numeric"})
</script>

<!-- The words above the band already say what it says, so it is decoration to a reader. -->
<div aria-hidden="true">
  <div class="flex h-24 items-end gap-1">
    {#each hours as hour (hour.at)}
      <div
        class="flex-1 rounded-t-md rounded-b-xs {fill(hour.count)}"
        style="height: {height(hour.count)}%">
      </div>
    {/each}
  </div>
  <div class="mt-2 flex text-xs text-muted-foreground">
    {#each hours as hour, index (hour.at)}
      <span class="flex-1">{index % 3 === 0 ? hourOf(hour.at) : ""}</span>
    {/each}
  </div>
</div>
