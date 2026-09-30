<script lang="ts">
  import Check from "@lucide/svelte/icons/check"
  import {answer, type Item, type Standing} from "$lib/data/feed"
  import type {Session} from "$lib/data/session"
  import {poll, pollResponse} from "$lib/kinds"

  // The count is what reached this device, not what the neighbourhood said.
  let {
    item,
    standing,
    session,
  }: {
    item: Item
    standing: Standing
    session: Session
  } = $props()

  const asked = $derived(poll.reader(item.event).parse())

  const closed = $derived(asked.isClosed())

  const single = $derived(asked.pollType() === "singlechoice")

  const result = $derived(asked.results(standing.response.votes))

  const mine = $derived(
    standing.response.votes
      .filter(vote => vote.pubkey === session.identity)
      .sort((a, b) => b.created_at - a.created_at)[0],
  )

  const chosen = $derived<Set<string>>(
    new Set(mine ? pollResponse.reader(mine).parse().selections() : []),
  )

  const share = (votes: number) => (result.voters ? (votes / result.voters) * 100 : 0)

  const choose = (id: string) => {
    if (single) return answer(item, [id])

    return answer(item, chosen.has(id) ? [...chosen].filter(kept => kept !== id) : [...chosen, id])
  }
</script>

<h4 class="text-base font-semibold text-pretty">{asked.title()}</h4>

<ul class="mt-2 space-y-1.5">
  {#each result.options as option (option.id)}
    <li>
      <button
        type="button"
        class="relative block w-full overflow-hidden rounded-md border px-3 py-1.5 text-left text-sm
               transition-colors disabled:cursor-default
               {chosen.has(option.id) ? 'border-secondary-accent' : 'border-border'}"
        disabled={closed}
        aria-pressed={chosen.has(option.id)}
        onclick={() => choose(option.id)}>
        <span
          class="absolute inset-y-0 left-0 bg-secondary-accent/15"
          style:width="{share(option.votes)}%"></span>
        <span class="relative flex items-center gap-1.5">
          {#if chosen.has(option.id)}
            <Check class="size-3.5 flex-none text-secondary-accent" />
          {/if}
          <span class="min-w-0 flex-1 truncate">{option.label}</span>
          <span class="flex-none text-xs text-muted-foreground">{option.votes}</span>
        </span>
      </button>
    </li>
  {/each}
</ul>

<p class="mt-1.5 text-xs text-muted-foreground">
  {closed ? "Closed" : single ? "Pick one" : "Pick as many as you like"} ·
  {result.voters}
  {result.voters === 1 ? "answer" : "answers"} so far
</p>
