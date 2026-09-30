<script lang="ts">
  import {ModeWatcher} from "mode-watcher"
  import FirstRun from "$lib/components/FirstRun.svelte"
  import Shell from "$lib/components/Shell.svelte"
  import {watchBack} from "$lib/data/nav"
  import {open, session} from "$lib/data/session"

  $effect(() => {
    open()
  })

  $effect(() => {
    const watching = watchBack()

    return () => {
      watching.then(stop => stop()).catch(() => undefined)
    }
  })
</script>

<ModeWatcher />

{#if $session.state === "ready"}
  <Shell />
{:else if $session.state === "absent"}
  <FirstRun />
{:else}
  <div class="flex min-h-svh items-center justify-center bg-background px-8">
    <p class="text-center text-sm text-pretty text-muted-foreground">
      {$session.state === "opening"
        ? "Opening…"
        : "The core is not running. This screen needs the app shell around it."}
    </p>
  </div>
{/if}
