<script lang="ts">
  import BluetoothOff from "@lucide/svelte/icons/bluetooth-off"
  import {bluetooth} from "$lib/data/bluetooth"

  // Unknown is the system not having said yet, which is not worth alarming anyone over.
  const said: Record<string, string> = {
    off: "Bluetooth is off, so nothing reaches this phone or leaves it. Turn it on to meet people.",
    denied:
      "Dip is not allowed to use Bluetooth. Allow it in your phone's settings to meet people.",
    unsupported: "This phone has no Bluetooth Dip can use, so it cannot meet anyone.",
  }
</script>

{#if said[$bluetooth]}
  <div
    role="status"
    class="sticky top-safe-t z-10 -mx-5 flex items-start gap-3 border-b border-border bg-card px-5 py-3
           text-sm text-pretty">
    <BluetoothOff class="mt-0.5 size-4 flex-none text-destructive" />
    <p>{said[$bluetooth]}</p>
  </div>
{/if}
