<script lang="ts">
  import Plus from "@lucide/svelte/icons/plus"
  import {Button} from "$lib/components/ui/button"
  import BluetoothBanner from "$lib/components/BluetoothBanner.svelte"
  import Board from "$lib/components/Board.svelte"
  import Bookmarks from "$lib/components/Bookmarks.svelte"
  import BottomNav from "$lib/components/BottomNav.svelte"
  import Composer from "$lib/components/Composer.svelte"
  import ContactDetail from "$lib/components/ContactDetail.svelte"
  import DeviceLogin from "$lib/components/DeviceLogin.svelte"
  import ItemDetail from "$lib/components/ItemDetail.svelte"
  import Pairing from "$lib/components/Pairing.svelte"
  import PairingTray from "$lib/components/PairingTray.svelte"
  import People from "$lib/components/People.svelte"
  import Settings from "$lib/components/Settings.svelte"
  import type {Item} from "$lib/data/feed"
  import {watchLinks} from "$lib/data/links"
  import {go, place} from "$lib/data/nav"
  import {requestOn, requests, watchPairings} from "$lib/data/pairing"
  import {step, watchTransfers} from "$lib/data/transfer"

  let composing = $state(false)
  let about = $state<Item | undefined>(undefined)

  $effect(() => {
    const watching = watchPairings()

    return () => {
      watching.then(stop => stop()).catch(() => undefined)
    }
  })

  $effect(() => {
    const watching = watchLinks()

    return () => {
      watching.then(stop => stop()).catch(() => undefined)
    }
  })

  $effect(() => {
    const watching = watchTransfers()

    return () => {
      watching.then(stop => stop()).catch(() => undefined)
    }
  })

  // A key handover interrupts, since the core only asks with a user right there.
  $effect(() => {
    if ($step.at === "comparing" && !$step.source) go({at: "device"})
  })

  const compose = (item?: Item) => {
    about = item
    composing = true
  }

  const asked = $derived.by(() => {
    const here = $place

    return here.at === "pairing" ? requestOn($requests, here.link) : undefined
  })
</script>

<div class="flex min-h-svh flex-col bg-background pb-36">
  <div class="mx-auto flex w-full max-w-2xl flex-1 flex-col px-5 pt-safe-t">
    <BluetoothBanner />

    {#if $place.at === "board"}
      <Board onBoost={compose} />
    {:else if $place.at === "bookmarks"}
      <Bookmarks onBoost={compose} />
    {:else if $place.at === "people"}
      <People />
    {:else if $place.at === "settings"}
      <Settings />
    {:else if $place.at === "contact"}
      <ContactDetail pubkey={$place.pubkey} />
    {:else if $place.at === "item"}
      <ItemDetail id={$place.id} onBoost={compose} />
    {:else if $place.at === "device"}
      <DeviceLogin />
    {:else}
      <Pairing request={asked} />
    {/if}
  </div>
</div>

<!-- One stack along the bottom, so the bar, the tray and the button cannot overlap. -->
<div class="pointer-events-none fixed inset-x-0 bottom-0 z-20 pb-safe-b">
  {#if $place.at === "board"}
    <div class="mx-auto flex max-w-2xl justify-end px-5 pb-4">
      <Button
        size="icon-lg"
        class="pointer-events-auto size-14 shadow-lg"
        aria-label="Write something"
        onclick={() => compose()}>
        <Plus class="size-6" />
      </Button>
    </div>
  {/if}

  {#if $place.at !== "pairing"}
    <div class="mx-auto max-w-2xl">
      <PairingTray requests={$requests} />
    </div>
  {/if}

  <BottomNav place={$place} />
</div>

<Composer bind:open={composing} {about} />
