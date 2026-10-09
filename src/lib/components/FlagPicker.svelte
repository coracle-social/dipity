<script lang="ts">
  import {untrack} from "svelte"
  import Check from "@lucide/svelte/icons/check"
  import {Button} from "$lib/components/ui/button"
  import * as Drawer from "$lib/components/ui/drawer"
  import {Textarea} from "$lib/components/ui/textarea"
  import {nameOf, setBlocked, social} from "$lib/data/contacts"
  import type {Item} from "$lib/data/feed"
  import {dismissable} from "$lib/data/nav"
  import {FLAG_WORDS, flag, flaggedByMe, reasons, unflag, wordsIn} from "$lib/data/reports"
  import {trash} from "$lib/data/trash"

  // The reasons to flag a post for, then what else to do about it.
  let {open = $bindable(false), item}: {open: boolean; item: Item} = $props()

  let chosen = $state<string[]>([])
  let message = $state("")
  let flagged = $state(false)
  let trashing = $state(false)
  let blocking = $state(false)

  const already = $derived($flaggedByMe.get(item.event.id) ?? new Set<string>())

  const words = $derived(wordsIn(message))

  const added = $derived(chosen.filter(reason => !already.has(reason)))

  const removed = $derived([...already].filter(reason => !chosen.includes(reason)))

  // Unchecking every reason the user gave is taking the flag back.
  const removing = $derived(already.size > 0 && chosen.length === 0)

  const author = $derived(nameOf($social, item.event.pubkey).name)

  const actions = $derived([
    {
      id: "trash",
      heading: "Move it to the trash",
      help: "It leaves your board and is deleted from this phone in a week, unless you put it back.",
      idle: "Move to trash",
      done: "Moving to trash",
      on: trashing,
      toggle: () => (trashing = !trashing),
    },
    {
      id: "block",
      heading: `Block ${author}`,
      help: "Your phone won't connect to theirs, and deletes anything of theirs it receives.",
      idle: `Block ${author}`,
      done: `Blocking ${author}`,
      on: blocking,
      toggle: () => (blocking = !blocking),
    },
  ])

  $effect(() => {
    if (!open) return

    // Read once on opening, since a flag landing while this is open would otherwise reset it.
    chosen = untrack(() => [...already])
    message = ""
    flagged = false
    trashing = false
    blocking = false

    const undismiss = dismissable(() => (open = false))

    // Applied on close, because trashing or blocking removes the card this dialog belongs to.
    return () => {
      undismiss()

      if (trashing) trash(item)
      if (blocking) setBlocked(item.event.pubkey, true)
    }
  })

  const toggle = (reason: string) =>
    (chosen = chosen.includes(reason)
      ? chosen.filter(held => held !== reason)
      : [...chosen, reason])

  const send = async () => {
    const [adding, dropping] = [added, removed]

    if (removing) open = false
    else flagged = true

    for (const reason of dropping) await unflag(item.event, reason)

    for (const reason of adding) await flag(item.event, reason, message)
  }
</script>

<Drawer.Root bind:open>
  <Drawer.Content>
    {#if flagged}
      <Drawer.Header>
        <Drawer.Title>Flagged</Drawer.Title>
        <Drawer.Description>
          Your phone won't pass this on. Anything you choose below happens when you close this.
        </Drawer.Description>
      </Drawer.Header>

      <ul class="space-y-5 px-4">
        {#each actions as action (action.id)}
          <li>
            <h3 class="text-sm font-semibold">{action.heading}</h3>
            <Button
              class="mt-2 w-full {action.on ? 'border-secondary-accent text-secondary-accent' : ''}"
              variant="outline"
              aria-pressed={action.on}
              onclick={action.toggle}>
              {#if action.on}
                <Check />
              {/if}
              {action.on ? action.done : action.idle}
            </Button>
            <p class="mt-1.5 text-xs text-pretty text-muted-foreground">
              {action.help}
            </p>
          </li>
        {/each}
      </ul>

      <Drawer.Footer>
        <Button onclick={() => (open = false)}>Done</Button>
      </Drawer.Footer>
    {:else}
      <Drawer.Header>
        <Drawer.Title>Flag activity</Drawer.Title>
        <Drawer.Description>
          Choose every reason that applies. Your flag is shared the way your posts are.
        </Drawer.Description>
      </Drawer.Header>

      <div class="flex flex-wrap gap-2 px-4">
        {#each reasons as reason (reason.id)}
          {@const on = chosen.includes(reason.id)}
          <Button
            variant="outline"
            size="sm"
            class={on ? "border-secondary-accent text-secondary-accent" : undefined}
            aria-pressed={on}
            onclick={() => toggle(reason.id)}>
            {#if on}
              <Check />
            {/if}
            {reason.label}
          </Button>
        {/each}
      </div>

      <div class="mt-3 space-y-1 px-4">
        <Textarea bind:value={message} class="min-h-20" placeholder="Anything to add? (optional)" />
        <p
          class="text-right text-xs {words > FLAG_WORDS
            ? 'text-destructive'
            : 'text-muted-foreground'}">
          {words} / {FLAG_WORDS} words
        </p>
      </div>

      <Drawer.Footer>
        <Button
          variant={removing ? "destructive" : "default"}
          disabled={(added.length === 0 && removed.length === 0) || words > FLAG_WORDS}
          onclick={send}>
          {removing ? "Remove flag" : "Flag"}
        </Button>
      </Drawer.Footer>
    {/if}
  </Drawer.Content>
</Drawer.Root>
