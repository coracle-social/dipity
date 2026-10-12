<script lang="ts">
  // The six digits two phones compare, grouped the way a person reads a number off another screen.
  let {code}: {code: number} = $props()

  const padded = $derived(String(code).padStart(6, "0"))
</script>

<p class="text-center font-mono text-4xl font-semibold tabular-nums">
  {padded.slice(0, 3)}
  {padded.slice(3)}
</p>
