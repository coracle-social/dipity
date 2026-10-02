<script lang="ts">
  import ArrowLeft from "@lucide/svelte/icons/arrow-left"
  import UserX from "@lucide/svelte/icons/user-x"
  import {Button} from "$lib/components/ui/button"
  import {Input} from "$lib/components/ui/input"
  import {Label} from "$lib/components/ui/label"
  import EmptyState from "$lib/components/EmptyState.svelte"
  import Shapes from "$lib/components/Shapes.svelte"
  import {back} from "$lib/data/nav"
  import {accept, decline, type Request} from "$lib/data/pairing"

  let {request}: {request?: Request} = $props()

  let petname = $state("")

  const answer = async (paired: boolean) => {
    if (request) {
      await (paired ? accept(request.link, petname.trim()) : decline(request.link))
    }

    back()
  }
</script>

<header class="flex items-center gap-1 pt-4 pb-3">
  <Button variant="ghost" size="icon-sm" aria-label="Back" onclick={back}>
    <ArrowLeft />
  </Button>
  <h1 class="text-2xl font-semibold">Pair</h1>
</header>

{#if request}
  <p class="max-w-prose text-sm text-pretty text-muted-foreground">
    Both phones are showing five shapes. If they are the same five in the same order, the two phones
    are talking to each other and to nothing in between.
  </p>

  <div class="my-8">
    <Shapes code={request.code} />
  </div>

  <div class="space-y-2">
    <Label for="petname">What do you call them?</Label>
    <Input id="petname" bind:value={petname} placeholder="Ben" autocomplete="off" />
    <p class="text-xs text-pretty text-muted-foreground">
      Nobody publishes a name here, so this is the name you and your neighbours see them under.
    </p>
  </div>

  <div class="mt-8 flex flex-col gap-2">
    <Button size="lg" disabled={!petname.trim()} onclick={() => answer(true)}>
      The shapes match
    </Button>
    <Button variant="ghost" size="lg" onclick={() => answer(false)}>
      {request.pubkey ? "Not now" : "Not this person"}
    </Button>
  </div>
{:else}
  <EmptyState icon={UserX}>That request is gone. They walked away, or it timed out.</EmptyState>
{/if}
