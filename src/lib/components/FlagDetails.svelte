<script lang="ts">
  import Ellipsis from "@lucide/svelte/icons/ellipsis"
  import ShieldCheck from "@lucide/svelte/icons/shield-check"
  import ShieldOff from "@lucide/svelte/icons/shield-off"
  import Trash from "@lucide/svelte/icons/trash-2"
  import {badgeVariants} from "$lib/components/ui/badge"
  import {Button} from "$lib/components/ui/button"
  import * as Drawer from "$lib/components/ui/drawer"
  import * as DropdownMenu from "$lib/components/ui/dropdown-menu"
  import {nameOf, type Social} from "$lib/data/contacts"
  import {dismissable} from "$lib/data/nav"
  import {
    flaggings,
    flaggingsOn,
    ignoredFlaggers,
    reasonLabel,
    setFlagsTrusted,
    trashFlagging,
    type Flagging,
  } from "$lib/data/reports"
  import type {Session} from "$lib/data/session"

  // Who flagged a post, for what, and what they said.
  let {
    open = $bindable(false),
    ids,
    social,
    session,
  }: {
    open: boolean
    /** The posts whose flags are drawn: a boost's own and what it passes on. */
    ids: string[]
    social: Social
    session: Session
  } = $props()

  $effect(() => {
    if (open) return dismissable(() => (open = false))
  })

  const listed = $derived(flaggingsOn($flaggings, ids))

  const ignored = $derived(new Set($ignoredFlaggers))

  // Removing the last flag also removes the warning this was opened from.
  const drop = (flagging: Flagging) => {
    if (listed.length === 1) open = false

    return trashFlagging(flagging)
  }

  const when = (at: number) =>
    new Date(at * 1000).toLocaleString(undefined, {
      month: "short",
      day: "numeric",
      hour: "numeric",
      minute: "2-digit",
    })
</script>

<Drawer.Root bind:open>
  <Drawer.Content>
    <Drawer.Header>
      <Drawer.Title>Flags</Drawer.Title>
      <Drawer.Description>
        Your phone doesn't pass on posts you flag, or pictures a contact you trust flags. A flag in
        the trash no longer counts.
      </Drawer.Description>
    </Drawer.Header>

    <ul class="space-y-4 px-4">
      {#each listed as flagging (flagging.reasons[0].report.id)}
        {@const mine = flagging.pubkey === session.identity}
        <li class="flex items-start gap-3">
          <div class="min-w-0 flex-1">
            <p class="text-sm">
              <span class="font-semibold">
                {mine ? "You" : nameOf(social, flagging.pubkey).name}
              </span>
              <span class="text-xs text-muted-foreground">
                · {when(flagging.at)}{!mine && ignored.has(flagging.pubkey) ? " · ignored" : ""}
              </span>
            </p>
            {#if flagging.message}
              <p class="mt-1 text-sm text-pretty whitespace-pre-line">{flagging.message}</p>
            {/if}
            <div class="mt-1.5 flex flex-wrap gap-1">
              {#each flagging.reasons as { reason } (reason)}
                <span class={badgeVariants({variant: "secondary"})}>{reasonLabel(reason)}</span>
              {/each}
            </div>
          </div>

          <DropdownMenu.Root>
            <DropdownMenu.Trigger>
              {#snippet child({props})}
                <Button {...props} variant="ghost" size="icon-sm" aria-label="More for this flag">
                  <Ellipsis />
                </Button>
              {/snippet}
            </DropdownMenu.Trigger>
            <DropdownMenu.Content align="end">
              <DropdownMenu.Item onSelect={() => drop(flagging)}>
                <Trash />
                {mine ? "Retract your flag" : "Trash this flag"}
              </DropdownMenu.Item>
              {#if !mine}
                {#if ignored.has(flagging.pubkey)}
                  <DropdownMenu.Item onSelect={() => setFlagsTrusted(flagging.pubkey, true)}>
                    <ShieldCheck />
                    Trust their flags
                  </DropdownMenu.Item>
                {:else}
                  <DropdownMenu.Item onSelect={() => setFlagsTrusted(flagging.pubkey, false)}>
                    <ShieldOff />
                    Ignore their flags
                  </DropdownMenu.Item>
                {/if}
              {/if}
            </DropdownMenu.Content>
          </DropdownMenu.Root>
        </li>
      {:else}
        <li class="text-sm text-muted-foreground">Nobody you trust has flagged this.</li>
      {/each}
    </ul>

    <Drawer.Footer>
      <Button onclick={() => (open = false)}>Done</Button>
    </Drawer.Footer>
  </Drawer.Content>
</Drawer.Root>
