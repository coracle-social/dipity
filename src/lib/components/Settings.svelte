<script lang="ts">
  import Check from "@lucide/svelte/icons/check"
  import Download from "@lucide/svelte/icons/download"
  import Smartphone from "@lucide/svelte/icons/smartphone"
  import {Button} from "$lib/components/ui/button"
  import {Input} from "$lib/components/ui/input"
  import * as InputGroup from "$lib/components/ui/input-group"
  import {Label} from "$lib/components/ui/label"
  import {Separator} from "$lib/components/ui/separator"
  import {Dip, type Scope} from "$lib/core"
  import {go} from "$lib/data/nav"
  import {
    policy,
    setAccept,
    setCoolOffMinutes,
    setDisclosureBudget,
    setForward,
    setGossip,
    setRetentionDays,
    setVisibility,
  } from "$lib/data/policy"
  import {npubOf, session} from "$lib/data/session"

  // The share sheet answers through a listener, so the button cannot report on its own.
  let backup = $state<"asking" | "shared" | "dropped" | "failed" | undefined>(undefined)

  $effect(() => {
    const listening = Dip.addListener("keyBackupShared", ({shared}) => {
      backup = shared ? "shared" : "dropped"
    })

    return () => {
      listening.then(handle => handle.remove()).catch(() => undefined)
    }
  })

  const exportKey = async () => {
    backup = "asking"

    try {
      await Dip.exportKey()
    } catch (error) {
      backup = "failed"
      console.error("the key could not be written", error)
    }
  }

  const backupSaid: Record<string, string> = {
    shared: "Written. Keep it somewhere that is not this phone.",
    dropped: "Nothing took a copy, so there is still only one.",
    failed: "The file could not be written. Try again.",
  }

  const tiers = $derived([
    {
      id: "accept",
      label: "What you accept",
      detail: "This controls whose notes you accept from nearby devices.",
      scopes: ["trusted", "network", "lenient"] as Scope[],
      on: $policy?.accept,
      set: setAccept,
    },
    {
      id: "gossip",
      label: "What you pass along",
      detail: "This controls whose notes you pass along to nearby devices.",
      scopes: ["nothing", "trusted", "network", "lenient"] as Scope[],
      on: $policy?.gossip,
      set: setGossip,
    },
    {
      id: "forward",
      label: "What others are allowed to pass along",
      detail: "This controls who is allowed to pass your notes along to others.",
      scopes: ["nothing", "trusted", "network"] as Scope[],
      on: $policy?.forward,
      set: setForward,
    },
    {
      id: "visibility",
      label: "What others are allowed to see",
      detail:
        "This controls who is allowed to see your notes, and the names you give people. Your trust list only ever reaches people you paired with, and your bookmarks stay on this phone.",
      scopes: ["trusted", "network", "lenient", "public"] as Scope[],
      on: $policy?.visibility.default,
      set: setVisibility,
    },
  ])

  const words: Record<Scope, string> = {
    nothing: "Nobody",
    trusted: "People you paired with",
    network: "People any of your contacts trust",
    lenient: "Anyone not blocked",
    public: "Anyone at all",
  }
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
    Your phone only trades things with a nearby device. These settings control your visibility to
    nearby devices and how much you share with them.
  </p>

  <div class="mt-4 space-y-5">
    <div class="space-y-1.5">
      <Label for="cool-off">Minutes you stay findable after closing the app</Label>
      <Input
        id="cool-off"
        type="number"
        min="0"
        value={$policy.cool_off_minutes}
        onchange={event => setCoolOffMinutes(event.currentTarget.value)} />
      <p class="text-xs text-pretty text-muted-foreground">
        How long after you put your phone in your pocket it keeps talking to other devices. Lower is
        more private, higher is more robust.
      </p>
    </div>

    <div class="space-y-1.5">
      <Label for="budget">How many people a day you can pair with</Label>
      <Input
        id="budget"
        type="number"
        min="0"
        value={$policy.disclosure_budget}
        onchange={event => setDisclosureBudget(event.currentTarget.value)} />
      <p class="text-xs text-pretty text-muted-foreground">
        Meeting somebody new means telling them who you are, but somebody who kept asking could
        track you. Your phone stops answering strangers after this many requests.
      </p>
    </div>

    <div class="space-y-1.5">
      <Label for="retention">How long you keep notes before dropping them</Label>
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
        How long your phone keeps something before it gets deleted. Bookmarks are never dropped.
      </p>
    </div>
  </div>

  <Separator class="my-6" />
{/if}

<h2 class="text-xs font-semibold tracking-widest text-muted-foreground uppercase">Your key</h2>

{#if $session.identity}
  <p class="mt-3 font-mono text-xs break-all text-muted-foreground">
    {npubOf($session.identity)}
  </p>
{/if}

<Button class="mt-4" variant="secondary" disabled={backup === "asking"} onclick={exportKey}>
  <Download />
  Save a copy somewhere safe
</Button>

{#if backup && backupSaid[backup]}
  <p class="mt-2 text-sm {backup === 'shared' ? 'text-secondary-accent' : 'text-destructive'}">
    {backupSaid[backup]}
  </p>
{/if}

<p class="mt-2 text-xs text-pretty text-muted-foreground">
  This is the only proof that your things are yours. Lose the phone without a copy of it and you
  start again as somebody new.
</p>

<Button class="mt-6" variant="secondary" onclick={() => go({at: "device"})}>
  <Smartphone />
  Use this key on another phone
</Button>

<p class="mt-2 text-xs text-pretty text-muted-foreground">
  Two phones holding one key are both you, so losing one costs nothing. The key goes across the same
  way everything else does, with the two phones in the room together.
</p>
