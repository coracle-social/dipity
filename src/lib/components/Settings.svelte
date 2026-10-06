<script lang="ts">
  import Check from "@lucide/svelte/icons/check"
  import Smartphone from "@lucide/svelte/icons/smartphone"
  import {Button} from "$lib/components/ui/button"
  import * as InputGroup from "$lib/components/ui/input-group"
  import {Label} from "$lib/components/ui/label"
  import {Separator} from "$lib/components/ui/separator"
  import {Switch} from "$lib/components/ui/switch"
  import KeyBackup from "$lib/components/KeyBackup.svelte"
  import type {Scope} from "$lib/core"
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
    setForward,
    setGossip,
    setRetentionDays,
    setVisibility,
  } from "$lib/data/policy"

  const tiers = $derived([
    {
      id: "accept",
      label: "What you accept",
      detail: "Whose notes your phone accepts from phones nearby.",
      scopes: ["trusted", "network", "lenient"] as Scope[],
      on: $policy?.accept,
      set: setAccept,
    },
    {
      id: "gossip",
      label: "What you pass along",
      detail: "Whose notes your phone passes on to phones nearby.",
      scopes: ["nothing", "trusted", "network", "lenient"] as Scope[],
      on: $policy?.gossip,
      set: setGossip,
    },
    {
      id: "forward",
      label: "What others are allowed to pass along",
      detail: "Who may pass your notes on to the people they meet.",
      scopes: ["nothing", "trusted", "network"] as Scope[],
      on: $policy?.forward,
      set: setForward,
    },
    {
      id: "visibility",
      label: "What others are allowed to see",
      detail:
        "Who may see your notes and the names you give people. Your trust list goes only to people you trust, who can pass it on once if you let them. Your bookmarks stay on this phone.",
      scopes: ["trusted", "network", "lenient"] as Scope[],
      on: $policy?.visibility.default,
      set: setVisibility,
    },
  ])

  const words: Record<Scope, string> = {
    nothing: "Nobody",
    trusted: "People you trust",
    network: "People any of your contacts trust",
    lenient: "Anyone not blocked",
    public: "Anyone at all",
  }

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
          {#each tier.scopes as scope (scope)}
            <button
              type="button"
              class="flex w-full items-center justify-between gap-2 rounded-xl px-3 py-2 text-left
                     text-sm transition-colors
                     {tier.on === scope
                ? 'bg-card font-medium text-foreground shadow-xs'
                : 'text-muted-foreground'}"
              aria-pressed={tier.on === scope}
              onclick={() => tier.set(scope)}>
              {words[scope]}
              {#if tier.on === scope}
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
    Your phone exchanges posts with phones nearby, even while it's in your pocket.
  </p>

  <div class="mt-4 space-y-5">
    <div class="flex items-start justify-between gap-4">
      <div class="min-w-0">
        <Label for="discover" class="text-sm font-semibold">
          Allow discovery when the app is closed
        </Label>
        <p class="mt-0.5 text-xs text-pretty text-muted-foreground">
          Each stranger your phone meets learns who you are and where you were. With Dipity open, it
          meets anyone nearby. With it closed, it meets a few strangers a day if this is on, and
          asks you first if it's off. People you've paired with or trust are always met.
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
        Counted from when a post first reached you, however old the post is. Once it's gone, your
        phone won't take it back. Your own posts and your bookmarks don't expire.
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
    Notifications are turned off for Dipity. Allow them in your phone's settings, then turn these
    back on.
  </p>
{/if}

<Separator class="my-6" />

<h2 class="text-xs font-semibold tracking-widest text-muted-foreground uppercase">Your key</h2>

<p class="mt-2 text-sm text-pretty text-muted-foreground">
  Your key is who you are on Dipity. It lives on this phone, and nobody can recover it for you.
</p>

<div class="mt-4 space-y-4">
  <div>
    <KeyBackup />
    <p class="mt-1.5 text-xs text-pretty text-muted-foreground">
      Save a copy, so losing this phone doesn't lose your identity.
    </p>
  </div>

  <div>
    <Button class="w-full" variant="secondary" onclick={() => go({at: "device"})}>
      <Smartphone />
      Use this key on another phone
    </Button>
    <p class="mt-1.5 text-xs text-pretty text-muted-foreground">
      Both phones post as you. The two phones need to be together.
    </p>
  </div>
</div>
