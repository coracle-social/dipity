<script lang="ts">
  import {ModeWatcher} from "mode-watcher"
  import Home from "$lib/components/Home.svelte"
  import {open, session} from "$lib/data/session"

  const waiting: Record<string, string> = {
    opening: "Opening…",
    absent: "No identity on this device yet.",
    unavailable: "The core is not running. This screen needs the app shell around it.",
  }

  $effect(() => {
    open()
  })
</script>

<ModeWatcher />

{#if $session.state === "ready"}
  <Home />
{:else}
  <div class="flex min-h-svh items-center justify-center bg-background px-8">
    <p class="text-center text-sm text-pretty text-muted-foreground">{waiting[$session.state]}</p>
  </div>
{/if}
