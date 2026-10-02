<script lang="ts">
  import {Button} from "$lib/components/ui/button"
  import Wordmark from "$lib/components/Wordmark.svelte"
  import {Input} from "$lib/components/ui/input"
  import {Label} from "$lib/components/ui/label"
  import {dismissable} from "$lib/data/nav"
  import {createIdentity, importIdentity} from "$lib/data/session"

  let nsec = $state("")
  let pasting = $state(false)
  let failed = $state("")

  $effect(() => {
    if (pasting) return dismissable(() => (pasting = false))
  })

  const take = async (make: () => Promise<void>) => {
    failed = ""

    try {
      await make()
    } catch (error) {
      failed = "That does not read as a key. A key starts with nsec1."
      console.error("the identity could not be stored", error)
    }
  }
</script>

<div class="flex min-h-svh flex-col justify-center px-6 pt-safe-t pb-safe-b">
  <div class="mx-auto w-full max-w-sm">
    <Wordmark class="mx-auto h-auto w-56" />
    <h1 class="mt-6 text-center text-2xl font-semibold text-balance">Where your town talks.</h1>

    {#if pasting}
      <div class="mt-6 space-y-2">
        <Label for="nsec">Paste the key</Label>
        <Input id="nsec" bind:value={nsec} placeholder="nsec1…" autocomplete="off" />
      </div>
      <div class="mt-4 flex flex-col gap-2">
        <Button size="lg" disabled={!nsec.trim()} onclick={() => take(() => importIdentity(nsec))}>
          Log in
        </Button>
        <Button variant="ghost" size="lg" onclick={() => (pasting = false)}>Back</Button>
      </div>
    {:else}
      <div class="mt-6 flex flex-col gap-2">
        <Button size="lg" onclick={() => take(createIdentity)}>Get started</Button>
        <Button variant="ghost" size="lg" onclick={() => (pasting = true)}>
          Log in with a key
        </Button>
      </div>
    {/if}

    {#if failed}
      <p class="mt-4 text-sm text-destructive">{failed}</p>
    {/if}
  </div>
</div>
