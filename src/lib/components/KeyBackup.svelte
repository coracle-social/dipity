<script lang="ts">
  import Download from "@lucide/svelte/icons/download"
  import KeyRound from "@lucide/svelte/icons/key-round"
  import {Button} from "$lib/components/ui/button"
  import * as Drawer from "$lib/components/ui/drawer"
  import {Input} from "$lib/components/ui/input"
  import {Label} from "$lib/components/ui/label"
  import {backup, exportKey, MIN_PASSWORD, resetBackup} from "$lib/data/backup"
  import {dismissable} from "$lib/data/nav"
  import {npubOf, session} from "$lib/data/session"

  // The backup flow: what a key is, an optional password, and the core's share sheet.
  let open = $state(false)
  let password = $state("")

  $effect(() => {
    if (open) return dismissable(() => (open = false))
  })

  $effect(() => {
    if (!open) {
      password = ""
      resetBackup()
    }
  })

  const short = $derived(password.length > 0 && password.length < MIN_PASSWORD)

  const said: Record<string, string> = {
    shared: "Saved. Keep it somewhere other than this phone.",
    dropped: "No copy was saved.",
    failed: "The file couldn't be saved. Try again.",
  }
</script>

<Button class="w-full" variant="secondary" onclick={() => (open = true)}>
  <KeyRound />
  Back up your key
</Button>

<Drawer.Root bind:open>
  <Drawer.Content>
    <Drawer.Header>
      <Drawer.Title>Back up your key</Drawer.Title>
    </Drawer.Header>

    <div class="space-y-4 px-4 pb-4">
      <p class="text-sm text-pretty">
        Your key has two halves. The public half is your name: people's phones know you by it, and
        it's safe to share.
      </p>

      {#if $session.identity}
        <p class="rounded-md bg-muted px-3 py-2 font-mono text-xs break-all text-muted-foreground">
          {npubOf($session.identity)}
        </p>
      {/if}

      <p class="text-sm text-pretty">
        The private half proves you're you, and it's what the backup holds. Anyone who has it can
        post as you, so keep the file somewhere safe that isn't this phone.
      </p>

      <div class="space-y-1.5">
        <Label for="backup-password">Password (optional)</Label>
        <Input
          id="backup-password"
          type="password"
          bind:value={password}
          autocomplete="new-password"
          placeholder="Lock the file with a password" />
        <p class="text-xs text-pretty text-muted-foreground">
          {password
            ? `At least ${MIN_PASSWORD} characters. Write it down: without it, nobody can open the file.`
            : "Without one, anyone who finds the file can post as you."}
        </p>
      </div>

      <Button
        class="w-full"
        disabled={$backup === "asking" || short}
        onclick={() => exportKey(password || undefined)}>
        <Download />
        Save the backup
      </Button>

      {#if $backup && said[$backup]}
        <p class="text-sm {$backup === 'shared' ? 'text-secondary-accent' : 'text-destructive'}">
          {said[$backup]}
        </p>
      {/if}
    </div>
  </Drawer.Content>
</Drawer.Root>
