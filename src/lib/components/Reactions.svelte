<script lang="ts">
  import * as Drawer from "$lib/components/ui/drawer"
  import {nameOf, type Social} from "$lib/data/contacts"
  import type {Response} from "$lib/data/feed"
  import {dismissable} from "$lib/data/nav"
  import type {Session} from "$lib/data/session"

  // Who reacted and with what, which is the only place a reaction names anyone.
  let {
    open = $bindable(false),
    response,
    social,
    session,
  }: {
    open: boolean
    response: Response
    social: Social
    session: Session
  } = $props()

  $effect(() => {
    if (open) return dismissable(() => (open = false))
  })

  const grouped = $derived(
    response.reactions
      .reduce<{emoji: string; people: string[]}[]>((groups, {emoji, pubkey}) => {
        const group = groups.find(held => held.emoji === emoji)

        if (group) {
          group.people.push(pubkey)

          return groups
        }

        return [...groups, {emoji, people: [pubkey]}]
      }, [])
      .sort((a, b) => b.people.length - a.people.length),
  )
</script>

<Drawer.Root bind:open>
  <Drawer.Content>
    <Drawer.Header>
      <Drawer.Title>What people made of it</Drawer.Title>
    </Drawer.Header>

    <ul class="space-y-4 px-4 pb-4">
      {#each grouped as { emoji, people } (emoji)}
        <li class="flex items-start gap-3">
          <span class="text-2xl leading-none">{emoji}</span>
          <p class="min-w-0 flex-1 text-sm text-pretty">
            {people
              .map(pubkey => (pubkey === session.identity ? "you" : nameOf(social, pubkey).name))
              .join(", ")}
          </p>
        </li>
      {/each}
    </ul>
  </Drawer.Content>
</Drawer.Root>
