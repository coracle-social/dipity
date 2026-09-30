<script lang="ts">
  import Circle from "@lucide/svelte/icons/circle"
  import Diamond from "@lucide/svelte/icons/diamond"
  import Heart from "@lucide/svelte/icons/heart"
  import Hexagon from "@lucide/svelte/icons/hexagon"
  import Moon from "@lucide/svelte/icons/moon"
  import Square from "@lucide/svelte/icons/square"
  import Star from "@lucide/svelte/icons/star"
  import Triangle from "@lucide/svelte/icons/triangle"

  // Five slots of eight shapes in three tints, which is `session::sas::PAIRING_SPACE` exactly.
  let {code}: {code: number} = $props()

  const shapes = [Circle, Square, Triangle, Diamond, Hexagon, Star, Heart, Moon]

  const tints = ["text-primary", "text-secondary-accent", "text-foreground"]

  const slots = $derived(
    [0, 1, 2, 3, 4].map(slot => {
      const digit = Math.floor(code / 24 ** slot) % 24

      return {shape: shapes[digit % 8], tint: tints[Math.floor(digit / 8)]}
    }),
  )
</script>

<div class="flex items-center justify-center gap-2">
  {#each slots as slot, index (index)}
    {@const Shape = slot.shape}
    <div class="flex size-14 items-center justify-center rounded-2xl bg-card shadow-sm {slot.tint}">
      <Shape class="size-8" strokeWidth={1.75} />
    </div>
  {/each}
</div>
