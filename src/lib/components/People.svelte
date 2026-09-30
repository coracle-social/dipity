<script lang="ts">
  import ChevronRight from "@lucide/svelte/icons/chevron-right"
  import {Badge} from "$lib/components/ui/badge"
  import {contacts, nameOf, short, social} from "$lib/data/contacts"
  import {go} from "$lib/data/nav"
</script>

<header class="pt-4 pb-3">
  <h1 class="text-2xl font-semibold">People</h1>
  <p class="mt-1 text-sm text-muted-foreground">
    Everyone this device knows, and the name you know them by.
  </p>
</header>

<ul class="mt-2 space-y-2">
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
              ? `paired · ${short(contact.pubkey)}`
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
  <p class="py-10 text-sm text-pretty text-muted-foreground">
    Nobody yet. Pair with somebody in the room with you and they turn up here under the name you
    gave them.
  </p>
{/if}
