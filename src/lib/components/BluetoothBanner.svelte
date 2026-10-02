<script lang="ts">
  import BluetoothOff from "@lucide/svelte/icons/bluetooth-off"
  import {bluetooth} from "$lib/data/bluetooth"

  // Unknown is the system not having said yet, which is not worth alarming anyone over.
  const said: Record<string, string> = {
    off: "Bluetooth is off, so nothing can reach this phone. Turn it on to start meeting people.",
    denied:
      "Dipity doesn't have permission use Bluetooth. Turn it on in settings to start meeting people.",
    unsupported: "This phone doesn't seem to have Bluetooth. Dipity probably won't work for you.",
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
