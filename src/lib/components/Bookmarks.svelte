<script lang="ts">
  import Bookmark from "@lucide/svelte/icons/bookmark"
  import Trash from "@lucide/svelte/icons/trash-2"
  import {Button} from "$lib/components/ui/button"
  import * as Tabs from "$lib/components/ui/tabs"
  import EmptyState from "$lib/components/EmptyState.svelte"
  import CategoryFilter from "$lib/components/CategoryFilter.svelte"
  import SearchBox from "$lib/components/SearchBox.svelte"
  import ItemCard from "$lib/components/ItemCard.svelte"
  import {bookmarked, bookmarkedItems, trashedItems} from "$lib/data/bookmarks"
  import {social} from "$lib/data/contacts"
  import {responses, saying, standingOf, warmthOf, type Item} from "$lib/data/feed"
  import {matches, nameMatches, wordsOf} from "$lib/data/search"
  import {session} from "$lib/data/session"
  import {emptyTrash, trashed} from "$lib/data/trash"
  import {categories, categoryOf, summaryOf} from "$lib/kinds"

  let {onBoost}: {onBoost: (item: Item) => void} = $props()

  const now = Date.now() / 1000

  const everything = categories.map(({id}) => id)

  // Each tab searches and filters only itself.
  let savedQuery = $state("")
  let binnedQuery = $state("")
  let savedShowing = $state<string[]>(everything)
  let binnedShowing = $state<string[]>(everything)

  // Emptying retracts the user's own writing, so it takes a second tap.
  let confirming = $state(false)

  const toggle = (showing: string[], id: string) =>
    showing.includes(id) ? showing.filter(kept => kept !== id) : [...showing, id]

  // A post matches by what it says, its title, or who wrote it, among the categories switched on.
  const narrowed = (items: Item[], query: string, showing: string[]) => {
    const words = wordsOf(query)

    return items.filter(
      ({event}) =>
        showing.includes(categoryOf(event.kind).id) &&
        (!words.length ||
          matches(words, `${summaryOf(event)} ${event.content}`) ||
          nameMatches($social, event.pubkey, words)),
    )
  }

  const saved = $derived(narrowed($bookmarkedItems, savedQuery, savedShowing))

  const binned = $derived(narrowed($trashedItems, binnedQuery, binnedShowing))

  const empty = async () => {
    confirming = false
    await emptyTrash()
  }
</script>

<header class="pt-4 pb-3">
  <h1 class="text-2xl font-semibold">Bookmarks</h1>
</header>

<Tabs.Root value="saved">
  <Tabs.List>
    <Tabs.Trigger value="saved">Saved</Tabs.Trigger>
    <Tabs.Trigger value="trash">Trash</Tabs.Trigger>
  </Tabs.List>

  <Tabs.Content value="saved">
    <div class="mt-3 flex items-center gap-2">
      <SearchBox class="min-w-0 flex-1" label="Search saved" bind:value={savedQuery} />
      <CategoryFilter
        showing={savedShowing}
        onToggle={id => (savedShowing = toggle(savedShowing, id))} />
    </div>

    <p class="mt-2 text-sm text-pretty text-muted-foreground">
      Bookmarked activity stays on this phone until you remove it.
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

    {#if saved.length === 0 && $bookmarkedItems.length > 0}
      <EmptyState icon={Bookmark}>No bookmarks match that.</EmptyState>
    {:else if $bookmarkedItems.length === 0}
      <EmptyState icon={Bookmark}
        >No bookmarks yet. Tap the bookmark icon on a post to save it here.</EmptyState>
    {/if}
  </Tabs.Content>

  <Tabs.Content value="trash">
    <div class="mt-3 flex items-center gap-2">
      <SearchBox class="min-w-0 flex-1" label="Search trash" bind:value={binnedQuery} />
      <CategoryFilter
        showing={binnedShowing}
        onToggle={id => (binnedShowing = toggle(binnedShowing, id))} />
    </div>

    <p class="mt-2 text-sm text-pretty text-muted-foreground">
      Trashed activity is automatically deleted after 7 days.
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
          retracted={$trashed.get(item.event.id)?.retracted}
          {onBoost} />
      {/each}
    </div>

    {#if binned.length === 0 && $trashedItems.length > 0}
      <EmptyState icon={Trash}>Nothing in the trash matches that.</EmptyState>
    {:else if $trashedItems.length === 0}
      <EmptyState icon={Trash}>The trash is empty.</EmptyState>
    {/if}
  </Tabs.Content>
</Tabs.Root>
