<script lang="ts">
  import Bookmark from "@lucide/svelte/icons/bookmark"
  import Trash from "@lucide/svelte/icons/trash-2"
  import {Button} from "$lib/components/ui/button"
  import * as Tabs from "$lib/components/ui/tabs"
  import EmptyState from "$lib/components/EmptyState.svelte"
  import SearchBox from "$lib/components/SearchBox.svelte"
  import ItemCard from "$lib/components/ItemCard.svelte"
  import {bookmarked, bookmarkedItems, trashedItems} from "$lib/data/bookmarks"
  import {social} from "$lib/data/contacts"
  import {responses, saying, standingOf, warmthOf, type Item} from "$lib/data/feed"
  import {matches, nameMatches, wordsOf} from "$lib/data/search"
  import {session} from "$lib/data/session"
  import {emptyTrash} from "$lib/data/trash"
  import {summaryOf} from "$lib/kinds"

  let {onBoost}: {onBoost: (item: Item) => void} = $props()

  const now = Date.now() / 1000

  let query = $state("")

  // Emptying retracts the user's own writing, so it takes a second tap.
  let confirming = $state(false)

  const words = $derived(wordsOf(query))

  // A thing matches by what it says, its title, or who wrote it.
  const searched = (items: Item[]) =>
    words.length
      ? items.filter(
          ({event}) =>
            matches(words, `${summaryOf(event)} ${event.content}`) ||
            nameMatches($social, event.pubkey, words),
        )
      : items

  const saved = $derived(searched($bookmarkedItems))

  const binned = $derived(searched($trashedItems))

  const empty = async () => {
    confirming = false
    await emptyTrash()
  }
</script>

<header class="pt-4 pb-3">
  <h1 class="text-2xl font-semibold">Bookmarks</h1>
</header>

<SearchBox class="mb-3" label="Search bookmarks and trash" bind:value={query} />

<Tabs.Root value="saved">
  <Tabs.List>
    <Tabs.Trigger value="saved">Saved</Tabs.Trigger>
    <Tabs.Trigger value="trash">Trash</Tabs.Trigger>
  </Tabs.List>

  <Tabs.Content value="saved">
    <p class="mt-2 text-sm text-pretty text-muted-foreground">
      What you bookmark stays on this phone, however long ago it stopped going around.
    </p>

    <div class="mt-3 space-y-3">
      {#each saved as item (item.event.id)}
        <ItemCard
          {item}
          social={$social}
          standing={standingOf($responses, $saying, item.event.id)}
          warmth={warmthOf(item, undefined, now)}
          session={$session}
          bookmarked={$bookmarked.has(item.event.id)}
          {onBoost} />
      {/each}
    </div>

    {#if words.length && saved.length === 0 && $bookmarkedItems.length > 0}
      <EmptyState icon={Bookmark}>None of your bookmarks says that.</EmptyState>
    {:else if $bookmarkedItems.length === 0}
      <EmptyState icon={Bookmark}
        >No bookmarks yet. The bookmark on a card saves it here.</EmptyState>
    {/if}
  </Tabs.Content>

  <Tabs.Content value="trash">
    <p class="mt-2 text-sm text-pretty text-muted-foreground">
      Things you throw out wait here for a week, then they are deleted. Deleting something you wrote
      asks everyone who has it to forget it.
    </p>

    {#if $trashedItems.length > 0}
      <div class="mt-3 flex justify-end gap-2">
        {#if confirming}
          <Button variant="ghost" size="sm" onclick={() => (confirming = false)}>Cancel</Button>
          <Button variant="destructive" size="sm" onclick={empty}>
            Delete {$trashedItems.length} for good
          </Button>
        {:else}
          <Button variant="outline" size="sm" onclick={() => (confirming = true)}>
            Empty trash
          </Button>
        {/if}
      </div>
    {/if}

    <div class="mt-3 space-y-3">
      {#each binned as item (item.event.id)}
        <ItemCard
          {item}
          social={$social}
          standing={standingOf($responses, $saying, item.event.id)}
          warmth={warmthOf(item, undefined, now)}
          session={$session}
          bookmarked={$bookmarked.has(item.event.id)}
          trashed
          {onBoost} />
      {/each}
    </div>

    {#if words.length && binned.length === 0 && $trashedItems.length > 0}
      <EmptyState icon={Trash}>Nothing in the trash says that.</EmptyState>
    {:else if $trashedItems.length === 0}
      <EmptyState icon={Trash}>The trash is empty.</EmptyState>
    {/if}
  </Tabs.Content>
</Tabs.Root>
