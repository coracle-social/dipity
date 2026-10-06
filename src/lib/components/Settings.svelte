<script lang="ts">
  import Check from "@lucide/svelte/icons/check"
  import Smartphone from "@lucide/svelte/icons/smartphone"
  import {Button} from "$lib/components/ui/button"
  import * as InputGroup from "$lib/components/ui/input-group"
  import {Label} from "$lib/components/ui/label"
  import {Separator} from "$lib/components/ui/separator"
  import {Switch} from "$lib/components/ui/switch"
  import KeyBackup from "$lib/components/KeyBackup.svelte"
  import type {Scope, Sharing} from "$lib/core"
  import {go} from "$lib/data/nav"
  import {
    notifyContent,
    notifyPairing,
    permission,
    refreshPermission,
    setNotify,
  } from "$lib/data/notifications"
  import {
    policy,
    setAccept,
    setDiscoverInBackground,
    setRetentionDays,
    setSharing,
  } from "$lib/data/policy"

  const accepts: Record<string, string> = {
    trusted: "People you trust",
    contacts: "People you've paired with",
    network: "People you've paired with, and people they trust",
    lenient: "Anyone not blocked",
  }

  const shares: Record<Sharing, string> = {
    contacts: "People you've paired with",
    network: "People you've paired with, who can pass it on",
    anyone: "Anyone you meet, and people you've paired with can pass it on",
  }

  const tiers = $derived([
    {
      id: "accept",
      label: "What you accept",
      detail: "Whose posts your phone keeps from phones nearby.",
      on: $policy?.accept as string | undefined,
      options: (["trusted", "contacts", "network", "lenient"] as Scope[]).map(scope => ({
        value: scope as string,
        label: accepts[scope],
      })),
      set: (value: string) => setAccept(value as Scope),
    },
    {
      id: "sharing",
      label: "Who can see your activity",
      detail: "Who your phone hands your posts to, and who can carry them a step further.",
      on: $policy?.sharing as string | undefined,
      options: (["contacts", "network", "anyone"] as Sharing[]).map(sharing => ({
        value: sharing as string,
        label: shares[sharing],
      })),
      set: (value: string) => setSharing(value as Sharing),
    },
  ])

  // The phone's answer changes in its own settings, so it is read again whenever this screen opens.
  $effect(() => {
    refreshPermission()
  })

  const alerts = $derived([
    {
      id: "notify-pairing",
      label: "Pairing requested",
      on: $notifyPairing,
      set: (on: boolean) => setNotify(notifyPairing, on),
    },
    {
      id: "notify-content",
      label: "New activity received",
      on: $notifyContent,
      set: (on: boolean) => setNotify(notifyContent, on),
    },
  ])
</script>

<header class="pt-4 pb-3">
  <h1 class="text-2xl font-semibold">Settings</h1>
</header>

{#if $policy}
  <h2 class="mt-4 text-xs font-semibold tracking-widest text-muted-foreground uppercase">
    Content
  </h2>

  <ul class="mt-3 space-y-5">
    {#each tiers as tier (tier.id)}
      <li>
        <p class="text-sm font-semibold">{tier.label}</p>
        <p class="mt-0.5 text-xs text-pretty text-muted-foreground">{tier.detail}</p>
        <div class="mt-2 space-y-0.5 rounded-2xl bg-muted p-1">
          {#each tier.options as option (option.value)}
            <button
              type="button"
              class="flex w-full items-center justify-between gap-2 rounded-xl px-3 py-2 text-left
                     text-sm transition-colors
                     {tier.on === option.value
                ? 'bg-card font-medium text-foreground shadow-xs'
                : 'text-muted-foreground'}"
              aria-pressed={tier.on === option.value}
              onclick={() => tier.set(option.value)}>
              {option.label}
              {#if tier.on === option.value}
                <Check class="size-4 flex-none text-secondary-accent" />
              {/if}
            </button>
          {/each}
        </div>
      </li>
    {/each}
  </ul>

  <Separator class="my-6" />

  <h2 class="text-xs font-semibold tracking-widest text-muted-foreground uppercase">Being found</h2>

  <p class="mt-2 text-sm text-pretty text-muted-foreground">
    Your phone trades posts with phones nearby, and each stranger it meets learns who you are.
  </p>

  <div class="mt-4 space-y-5">
    <div class="flex items-start justify-between gap-4">
      <div class="min-w-0">
        <Label for="discover" class="text-sm font-semibold">
          Allow discovery when the app is closed
        </Label>
        <p class="mt-0.5 text-xs text-pretty text-muted-foreground">
          With this off, strangers wait for your approval while Dipity is closed.
        </p>
      </div>
      <Switch
        id="discover"
        checked={$policy.discover_in_background}
        onCheckedChange={setDiscoverInBackground} />
    </div>

    <div class="space-y-1.5">
      <Label for="retention">How long you keep other people's things</Label>
      <InputGroup.Root>
        <InputGroup.Input
          id="retention"
          type="number"
          min="1"
          value={$policy.retention_days}
          onchange={event => setRetentionDays(event.currentTarget.value)} />
        <InputGroup.Addon align="inline-end">
          <InputGroup.Text>days</InputGroup.Text>
        </InputGroup.Addon>
      </InputGroup.Root>
      <p class="text-xs text-pretty text-muted-foreground">
        Counted from when each post reached you, and your own posts and bookmarks never expire.
      </p>
    </div>
  </div>

  <Separator class="my-6" />
{/if}

<h2 class="text-xs font-semibold tracking-widest text-muted-foreground uppercase">Notifications</h2>

<p class="mt-2 text-sm text-pretty text-muted-foreground">
  Notifications appear only while Dipity is closed, and are created on this phone.
</p>

<ul class="mt-4 space-y-4">
  {#each alerts as alert (alert.id)}
    <li class="flex items-center justify-between gap-4">
      <Label for={alert.id} class="text-sm font-semibold">{alert.label}</Label>
      <Switch
        id={alert.id}
        checked={alert.on && $permission !== "denied"}
        onCheckedChange={alert.set} />
    </li>
  {/each}
</ul>

{#if $permission === "denied"}
  <p class="mt-3 text-xs text-pretty text-destructive">
    Notifications are off for Dipity in your phone's settings.
  </p>
{/if}

<Separator class="my-6" />

<h2 class="text-xs font-semibold tracking-widest text-muted-foreground uppercase">Your key</h2>

<p class="mt-2 text-sm text-pretty text-muted-foreground">
  Your key is who you are on Dipity, and nobody can recover it if you lose this phone.
</p>

<div class="mt-4 space-y-4">
  <KeyBackup />

  <div>
    <Button class="w-full" variant="secondary" onclick={() => go({at: "device"})}>
      <Smartphone />
      Use this key on another phone
    </Button>
    <p class="mt-1.5 text-xs text-pretty text-muted-foreground">
      Hold both phones together, and both will post as you.
    </p>
  </div>
</div>
