<script lang="ts">
  import Download from "@lucide/svelte/icons/download"
  import {Button} from "$lib/components/ui/button"
  import {Input} from "$lib/components/ui/input"
  import {Label} from "$lib/components/ui/label"
  import {Switch} from "$lib/components/ui/switch"
  import {backup, exportKey, MIN_PASSWORD, resetBackup} from "$lib/data/backup"

  let locked = $state(false)
  let password = $state("")

  $effect(() => resetBackup())

  const short = $derived(locked && password.length < MIN_PASSWORD)

  const said: Record<string, string> = {
    shared: "Saved. Keep it somewhere other than this phone.",
    dropped: "Nothing took a copy, so there is still only one.",
    failed: "The file could not be written. Try again.",
  }
</script>

<div class="flex items-start justify-between gap-4">
  <div class="min-w-0">
    <Label for="locked" class="text-sm font-semibold">Lock the file with a password</Label>
    <p class="mt-0.5 text-xs text-pretty text-muted-foreground">
      {locked
        ? `At least ${MIN_PASSWORD} characters. Write it down, because nobody can recover the key without it.`
        : "Without one, anyone who finds the file can be you."}
    </p>
  </div>
  <Switch id="locked" bind:checked={locked} />
</div>

{#if locked}
  <Input
    class="mt-3"
    type="password"
    bind:value={password}
    autocomplete="new-password"
    placeholder="Password" />
{/if}

<Button
  class="mt-4"
  variant="secondary"
  disabled={$backup === "asking" || short}
  onclick={() => exportKey(locked ? password : undefined)}>
  <Download />
  Save a copy somewhere safe
</Button>

{#if $backup && said[$backup]}
  <p class="mt-2 text-sm {$backup === 'shared' ? 'text-secondary-accent' : 'text-destructive'}">
    {said[$backup]}
  </p>
{/if}
