<script lang="ts">
  import ChevronRight from "@lucide/svelte/icons/chevron-right"
  import Filter from "@lucide/svelte/icons/list-filter"
  import Tag from "@lucide/svelte/icons/tag"
  import Users from "@lucide/svelte/icons/users"
  import {Badge} from "$lib/components/ui/badge"
  import {Button} from "$lib/components/ui/button"
  import * as DropdownMenu from "$lib/components/ui/dropdown-menu"
  import * as Tabs from "$lib/components/ui/tabs"
  import EmptyState from "$lib/components/EmptyState.svelte"
  import SearchBox from "$lib/components/SearchBox.svelte"
  import {aliases, contacts, nameOf, short, social} from "$lib/data/contacts"
  import {go} from "$lib/data/nav"
  import {matches, nameMatches, wordsOf} from "$lib/data/search"

  // Each tab searches only itself.
  let contactQuery = $state("")
  let aliasQuery = $state("")

  const standings = [
    {value: "everyone", label: "Everyone"},
    {value: "trusted", label: "Trusted"},
    {value: "muted", label: "Muted"},
    {value: "blocked", label: "Blocked"},
  ] as const

  let standing = $state<(typeof standings)[number]["value"]>("everyone")

  const contactWords = $derived(wordsOf(contactQuery))

  const aliasWords = $derived(wordsOf(aliasQuery))

  const shown = $derived(
    $contacts.filter(
      contact =>
        (standing === "everyone" || contact[standing]) &&
        (!contactWords.length || nameMatches($social, contact.pubkey, contactWords)),
    ),
  )

  const given = $derived(
    aliasWords.length
      ? $aliases.filter(({spellings}) => spellings.some(spelling => matches(aliasWords, spelling)))
      : $aliases,
  )

  const narrowed = $derived(standing !== "everyone" || contactWords.length > 0)

  /** Who gave a name, by the name the user knows them by. */
  const givers = (by: string[]) => by.map(pubkey => nameOf($social, pubkey).name).join(", ")
</script>

<header class="pt-4 pb-3">
  <h1 class="text-2xl font-semibold">People</h1>
</header>

<Tabs.Root value="contacts">
  <Tabs.List>
    <Tabs.Trigger value="contacts">Contacts</Tabs.Trigger>
    <Tabs.Trigger value="aliases">Aliases</Tabs.Trigger>
  </Tabs.List>

  <Tabs.Content value="contacts">
    <div class="mt-3 flex items-center gap-2">
      <SearchBox class="min-w-0 flex-1" label="Search contacts" bind:value={contactQuery} />
      <DropdownMenu.Root>
        <DropdownMenu.Trigger>
          {#snippet child({props})}
            <Button {...props} variant={standing === "everyone" ? "ghost" : "secondary"} size="sm">
              <Filter />
              {standings.find(({value}) => value === standing)?.label}
            </Button>
          {/snippet}
        </DropdownMenu.Trigger>
        <DropdownMenu.Content align="end">
          <DropdownMenu.RadioGroup bind:value={standing}>
            {#each standings as option (option.value)}
              <DropdownMenu.RadioItem value={option.value}>{option.label}</DropdownMenu.RadioItem>
            {/each}
          </DropdownMenu.RadioGroup>
        </DropdownMenu.Content>
      </DropdownMenu.Root>
    </div>

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

    {#if narrowed && shown.length === 0 && $contacts.length > 0}
      <EmptyState icon={Users}>No contacts match that.</EmptyState>
    {:else if $contacts.length === 0}
      <EmptyState icon={Users}>Pair with someone nearby and they appear here.</EmptyState>
    {/if}
  </Tabs.Content>

  <Tabs.Content value="aliases">
    <SearchBox class="mt-3" label="Search aliases" bind:value={aliasQuery} />

    <p class="mt-3 text-sm text-pretty text-muted-foreground">
      These are the names people gave you when you paired.
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

    {#if aliasWords.length && given.length === 0}
      <EmptyState icon={Tag}>No aliases match that.</EmptyState>
    {:else if $aliases.length === 0}
      <EmptyState icon={Tag}>Names people give you when you pair appear here.</EmptyState>
    {/if}
  </Tabs.Content>
</Tabs.Root>
