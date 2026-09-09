<script lang="ts">
  import type {Warmth} from "$lib/data/arrivals"

  let {where, when, warmth}: {where: string; when: string; warmth: Warmth} = $props()

  const rings: Record<Warmth, string> = {
    warm: "border-primary text-primary",
    fading: "border-secondary-accent text-secondary-accent",
    cold: "border-border text-muted-foreground",
    kept: "border-border text-muted-foreground",
  }
</script>

<div
  class="flex size-[4.5rem] flex-none -rotate-6 flex-col items-center justify-center gap-0.5
         rounded-full border-2 border-dashed text-center {rings[warmth]}">
  <span class="font-mono text-[0.5rem] tracking-widest uppercase">{where}</span>
  <span class="text-xs font-semibold">{when}</span>
</div>
