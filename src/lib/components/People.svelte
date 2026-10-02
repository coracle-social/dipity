<script lang="ts">
  import ChevronRight from "@lucide/svelte/icons/chevron-right"
  import Tag from "@lucide/svelte/icons/tag"
  import Users from "@lucide/svelte/icons/users"
  import {Badge} from "$lib/components/ui/badge"
  import * as Tabs from "$lib/components/ui/tabs"
  import EmptyState from "$lib/components/EmptyState.svelte"
  import {aliases, contacts, nameOf, short, social} from "$lib/data/contacts"
  import {go} from "$lib/data/nav"

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
    <p class="mt-3 text-sm text-muted-foreground">
      People you've met in person, or who you've heard about from others.
    </p>

    <ul class="mt-4 space-y-2">
      {#each $contacts as contact (contact.pubkey)}
        {@const named = nameOf($social, contact.pubkey)}
        <li>
          <button
            type="button"
            class="flex w-full items-center gap-3 rounded-lg bg-card px-4 py-3 text-left shadow-sm
               transition-shadow hover:shadow-md"
            onclick={() => go({at: "contact", pubkey: contact.pubkey})}>
            <div class="min-w-0 flex-1">
              <p class="truncate font-semibold">{named.name}</p>
              <p class="truncate text-xs text-muted-foreground">
                {contact.petname
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

    {#if $contacts.length === 0}
      <EmptyState icon={Users}>
        Nobody yet. Pair with somebody in the room with you and they turn up here under the name you
        gave them.
      </EmptyState>
    {/if}
  </Tabs.Content>

  <Tabs.Content value="aliases">
    <p class="mt-3 text-sm text-pretty text-muted-foreground">
      Nobody publishes a name here, so the people you pair with each give you one. These are the
      names other people know you by, and who gave you each one.
    </p>

    <ul class="mt-4 space-y-2">
      {#each $aliases as alias (alias.slug)}
        <li class="rounded-lg bg-card px-4 py-3 shadow-sm">
          <p class="font-semibold">{alias.spellings.join(" · ")}</p>
          <p class="mt-0.5 text-xs text-pretty text-muted-foreground">
            from {givers(alias.by)}
          </p>
        </li>
      {/each}
    </ul>

    {#if $aliases.length === 0}
      <EmptyState icon={Tag}>
        Nobody has named you yet. When somebody pairs with you, the name they give you shows up
        here.
      </EmptyState>
    {/if}
  </Tabs.Content>
</Tabs.Root>
