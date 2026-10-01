<script lang="ts">
  import KeyRound from "@lucide/svelte/icons/key-round"
  import KeyBackup from "$lib/components/KeyBackup.svelte"
  import {Button} from "$lib/components/ui/button"
  import {Input} from "$lib/components/ui/input"
  import {Label} from "$lib/components/ui/label"
  import {backup} from "$lib/data/backup"
  import {dismissable} from "$lib/data/nav"
  import {backedUp, createIdentity, importIdentity, session} from "$lib/data/session"

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
    <KeyRound class="size-8 text-primary" />
    {#if $session.state === "backup"}
      <h1 class="mt-4 text-2xl font-semibold text-balance">Save a copy of your key</h1>
      <p class="mt-2 text-sm text-pretty text-muted-foreground">
        The key is the only proof that your things are yours, and it is only on this phone. Lose the
        phone without a copy and you start again as somebody new.
      </p>
      <div class="mt-6">
        <KeyBackup />
      </div>
      <Button class="mt-6 w-full" size="lg" disabled={$backup !== "shared"} onclick={backedUp}>
        Continue
      </Button>
    {:else}
      <h1 class="mt-4 text-2xl font-semibold text-balance">This phone needs a key</h1>
      <p class="mt-2 text-sm text-pretty text-muted-foreground">
        It is how the people you pair with know it is you the next time. It stays on this phone.
      </p>

      {#if pasting}
        <div class="mt-6 space-y-2">
          <Label for="nsec">Paste the key</Label>
          <Input id="nsec" bind:value={nsec} placeholder="nsec1…" autocomplete="off" />
        </div>
        <div class="mt-4 flex flex-col gap-2">
          <Button
            size="lg"
            disabled={!nsec.trim()}
            onclick={() => take(() => importIdentity(nsec))}>
            Use this key
          </Button>
          <Button variant="ghost" size="lg" onclick={() => (pasting = false)}>Back</Button>
        </div>
      {:else}
        <div class="mt-6 flex flex-col gap-2">
          <Button size="lg" onclick={() => take(createIdentity)}>Make one</Button>
          <Button variant="ghost" size="lg" onclick={() => (pasting = true)}>
            I already have one
          </Button>
        </div>
      {/if}
    {/if}

    {#if failed}
      <p class="mt-4 text-sm text-destructive">{failed}</p>
    {/if}
  </div>
</div>
