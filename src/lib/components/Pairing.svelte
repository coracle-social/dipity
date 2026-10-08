<script lang="ts">
  import ArrowLeft from "@lucide/svelte/icons/arrow-left"
  import ChevronLeft from "@lucide/svelte/icons/chevron-left"
  import ChevronRight from "@lucide/svelte/icons/chevron-right"
  import UserX from "@lucide/svelte/icons/user-x"
  import {Button} from "$lib/components/ui/button"
  import {Input} from "$lib/components/ui/input"
  import {Label} from "$lib/components/ui/label"
  import EmptyState from "$lib/components/EmptyState.svelte"
  import Shapes from "$lib/components/Shapes.svelte"
  import {short} from "$lib/data/contacts"
  import {back, swap} from "$lib/data/nav"
  import {accept, decline, requests, type Request} from "$lib/data/pairing"
  import {session} from "$lib/data/session"

  let {request}: {request?: Request} = $props()

  let petname = $state("")

  // A name typed for one person is not for the next, and somebody already named starts from that name.
  const link = $derived(request?.link)

  $effect(() => {
    if (link !== undefined) petname = request?.known ?? ""
  })

  const at = $derived(request ? $requests.findIndex(asking => asking.link === request.link) : -1)

  const open = (index: number) => {
    const next = $requests[index]

    if (next) swap({at: "pairing", link: next.link})
  }

  // Answering one moves on to whoever else is waiting, rather than leaving them in the tray.
  const answer = async (paired: boolean) => {
    if (request) {
      await (paired ? accept(request.link, petname.trim()) : decline(request.link))
    }

    const waiting = $requests.find(asking => asking.link !== request?.link)

    if (waiting) {
      swap({at: "pairing", link: waiting.link})
    } else {
      back()
    }
  }
</script>

<header class="flex items-center gap-1 pt-4 pb-3">
  <Button variant="ghost" size="icon-sm" aria-label="Back" onclick={back}>
    <ArrowLeft />
  </Button>
  <h1 class="text-2xl font-semibold">Pair</h1>
</header>

{#if request}
  {#if request.known}
    <p class="mb-2 max-w-prose text-sm font-medium text-pretty">
      You already know {request.known}.
    </p>
  {/if}

  <p class="max-w-prose text-sm text-pretty text-muted-foreground">
    Check that both phones show the same five shapes in the same order.
  </p>

  <div class="my-8">
    <Shapes code={request.code} />
  </div>

  {#if request.pubkey && $session.identity}
    <!-- Each pair of phones has its own shapes. The two people check they are on each other's request. -->
    <p class="-mt-4 mb-8 text-center text-xs text-muted-foreground">
      Their phone <span class="font-mono text-foreground">{short(request.pubkey)}</span>
      · Your phone <span class="font-mono text-foreground">{short($session.identity)}</span>
    </p>
  {/if}

  <div class="space-y-2">
    <Label for="petname">What do you call them?</Label>
    <Input id="petname" bind:value={petname} placeholder="Ben" autocomplete="off" />
    <p class="text-xs text-pretty text-muted-foreground">
      This is the only name they have on your phone.
    </p>
  </div>

  <div class="mt-8 flex flex-col gap-2">
    <Button size="lg" disabled={!petname.trim()} onclick={() => answer(true)}>
      The shapes match
    </Button>
    <Button variant="ghost" size="lg" onclick={() => answer(false)}>
      {request.held ? "Not this person" : "Not now"}
    </Button>
  </div>

  {#if $requests.length > 1}
    <p class="mt-8 text-xs text-pretty text-muted-foreground">
      Each request has its own shapes, so if these don't match, try the next one.
    </p>
    <nav class="mt-2 flex items-center justify-between" aria-label="Other pairing requests">
      <Button variant="ghost" size="sm" disabled={at <= 0} onclick={() => open(at - 1)}>
        <ChevronLeft />
        Previous
      </Button>
      <span class="text-sm text-muted-foreground">{at + 1} of {$requests.length}</span>
      <Button
        variant="ghost"
        size="sm"
        disabled={at >= $requests.length - 1}
        onclick={() => open(at + 1)}>
        Next
        <ChevronRight />
      </Button>
    </nav>
  {/if}
{:else}
  <EmptyState icon={UserX}
    >This request has ended. They moved out of range, or it timed out.</EmptyState>
{/if}
