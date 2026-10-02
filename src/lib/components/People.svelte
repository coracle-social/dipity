<script lang="ts">
  import ChevronRight from "@lucide/svelte/icons/chevron-right"
  import Tag from "@lucide/svelte/icons/tag"
  import Users from "@lucide/svelte/icons/users"
  import {Badge} from "$lib/components/ui/badge"
  import * as Tabs from "$lib/components/ui/tabs"
  import EmptyState from "$lib/components/EmptyState.svelte"
  import SearchBox from "$lib/components/SearchBox.svelte"
  import {aliases, contacts, nameOf, short, social} from "$lib/data/contacts"
  import {go} from "$lib/data/nav"
  import {matches, nameMatches, wordsOf} from "$lib/data/search"

  let query = $state("")

  const words = $derived(wordsOf(query))

  const shown = $derived(
    words.length ? $contacts.filter(({pubkey}) => nameMatches($social, pubkey, words)) : $contacts,
  )

  const given = $derived(
    words.length
      ? $aliases.filter(({spellings}) => spellings.some(spelling => matches(words, spelling)))
      : $aliases,
  )

  /** Who gave a name, by the name the user knows them by. */
  const givers = (by: string[]) => by.map(pubkey => nameOf($social, pubkey).name).join(", ")
</script>

<header class="pt-4 pb-3">
  <h1 class="text-2xl font-semibold">People</h1>
</header>

<SearchBox class="mb-3" label="Search people" bind:value={query} />

<Tabs.Root value="contacts">
  <Tabs.List>
    <Tabs.Trigger value="contacts">Contacts</Tabs.Trigger>
    <Tabs.Trigger value="aliases">Aliases</Tabs.Trigger>
  </Tabs.List>

  <Tabs.Content value="contacts">
    <p class="mt-3 text-sm text-muted-foreground">
      People you've met in person, or who you've heard about from others.
    </p>

    <ul class="mt-4 space-y-2">
      {#each shown as contact (contact.pubkey)}
        {@const named = nameOf($social, contact.pubkey)}
        <li>
          <button
            type="button"
            class="flex w-full items-center gap-3 rounded-lg bg-card px-4 py-3 text-left shadow-sm
               transition-shadow hover:shadow-md"
            onclick={() => go({at: "contact", pubkey: contact.pubkey})}>
            <div class="min-w-0 flex-1">
              <p class="flex items-center gap-2 truncate font-semibold">
                {#if contact.connected}
                  <span class="size-2 flex-none rounded-full bg-secondary-accent"></span>
                {/if}
                {named.name}
              </p>
              <p class="truncate text-xs text-muted-foreground">
                {contact.connected ? "here now · " : ""}{contact.petname
                  ? `named by you · ${short(contact.pubkey)}`
                  : named.according
                    ? `known through ${named.according}`
                    : short(contact.pubkey)}
              </p>
            </div>

            {#if contact.blocked}
              <Badge variant="destructive">blocked</Badge>
            {:else if contact.trusted}
              <Badge variant="secondary">trusted</Badge>
            {/if}
            {#if contact.muted}
              <Badge variant="outline">muted</Badge>
            {/if}

            <ChevronRight class="size-4 flex-none text-muted-foreground" />
          </button>
        </li>
      {/each}
    </ul>

    {#if words.length && shown.length === 0}
      <EmptyState icon={Users}>No contacts match that.</EmptyState>
    {:else if $contacts.length === 0}
      <EmptyState icon={Users}>
        No contacts yet. Pair with someone nearby and they appear here under the name you give them.
      </EmptyState>
    {/if}
  </Tabs.Content>

  <Tabs.Content value="aliases">
    <p class="mt-3 text-sm text-pretty text-muted-foreground">
      Dipity has no profiles, so everyone you pair with names you. These are the names people know
      you by, and who gave each one.
    </p>

    <ul class="mt-4 space-y-2">
      {#each given as alias (alias.slug)}
        <li class="rounded-lg bg-card px-4 py-3 shadow-sm">
          <p class="font-semibold">{alias.spellings.join(" · ")}</p>
          <p class="mt-0.5 text-xs text-pretty text-muted-foreground">
            from {givers(alias.by)}
          </p>
        </li>
      {/each}
    </ul>

    {#if words.length && given.length === 0}
      <EmptyState icon={Tag}>No aliases match that.</EmptyState>
    {:else if $aliases.length === 0}
      <EmptyState icon={Tag}>
        No aliases yet. When someone pairs with you, the name they give you appears here.
      </EmptyState>
    {/if}
  </Tabs.Content>
</Tabs.Root>
