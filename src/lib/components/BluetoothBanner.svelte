<script lang="ts">
  import BluetoothOff from "@lucide/svelte/icons/bluetooth-off"
  import {bluetooth} from "$lib/data/bluetooth"

  // Unknown is the system not having said yet, which is not worth alarming anyone over.
  const said: Record<string, string> = {
    off: "Turn on Bluetooth to trade posts with phones nearby.",
    denied: "Allow Dipity to use Bluetooth in your phone's settings.",
    unsupported: "This phone doesn't seem to have Bluetooth, which Dipity needs.",
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
