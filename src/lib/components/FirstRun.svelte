<script lang="ts">
  import {Button} from "$lib/components/ui/button"
  import {Checkbox} from "$lib/components/ui/checkbox"
  import Wordmark from "$lib/components/Wordmark.svelte"
  import {Input} from "$lib/components/ui/input"
  import {Label} from "$lib/components/ui/label"
  import {dismissable} from "$lib/data/nav"
  import {awaitIdentity, createIdentity, importIdentity} from "$lib/data/session"

  let nsec = $state("")
  let pasting = $state(false)
  let failed = $state("")
  let agreed = $state(false)

  $effect(() => {
    if (pasting) return dismissable(() => (pasting = false))
  })

  const take = async (make: () => Promise<void>, why = "That didn't work. Try again.") => {
    failed = ""

    try {
      await make()
    } catch (error) {
      failed = why
      console.error("the identity could not be stored", error)
    }
  }
</script>

<div class="flex min-h-svh flex-col justify-center px-6 pt-safe-t pb-safe-b">
  <div class="mx-auto w-full max-w-sm">
    <Wordmark class="mx-auto h-auto w-56" />
    <h1 class="mt-6 text-center text-2xl font-semibold text-balance">Where your town talks.</h1>

    <div class="mt-6 flex items-start gap-3">
      <Checkbox id="terms" bind:checked={agreed} class="mt-0.5" />
      <Label for="terms" class="block text-sm leading-snug font-normal text-muted-foreground">
        I agree to the
        <a
          class="text-foreground underline"
          href="https://dipity.coracle.social/terms"
          target="_blank"
          rel="noopener noreferrer">Terms of use</a
        >.
      </Label>
    </div>

    {#if pasting}
      <div class="mt-6 space-y-2">
        <Label for="nsec">Paste the key</Label>
        <Input id="nsec" bind:value={nsec} placeholder="nsec1…" autocomplete="off" />
      </div>
      <div class="mt-4 flex flex-col gap-2">
        <Button
          size="lg"
          disabled={!nsec.trim() || !agreed}
          onclick={() =>
            take(() => importIdentity(nsec), "That isn't a valid key. Keys start with nsec1.")}>
          Log in
        </Button>
        <Button variant="ghost" size="lg" onclick={() => (pasting = false)}>Back</Button>
      </div>
    {:else}
      <div class="mt-6 flex flex-col gap-2">
        <Button size="lg" disabled={!agreed} onclick={() => take(createIdentity)}>
          Get started
        </Button>
        <Button variant="outline" size="lg" disabled={!agreed} onclick={() => take(awaitIdentity)}>
          Log in with another phone
        </Button>
        <Button variant="outline" size="lg" disabled={!agreed} onclick={() => (pasting = true)}>
          Log in with a key
        </Button>
      </div>
    {/if}

    {#if failed}
      <p class="mt-4 text-sm text-destructive">{failed}</p>
    {/if}
  </div>
</div>
