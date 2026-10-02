<script lang="ts">
  import Check from "@lucide/svelte/icons/check"
  import Smartphone from "@lucide/svelte/icons/smartphone"
  import {Button} from "$lib/components/ui/button"
  import {Input} from "$lib/components/ui/input"
  import * as InputGroup from "$lib/components/ui/input-group"
  import {Label} from "$lib/components/ui/label"
  import {Separator} from "$lib/components/ui/separator"
  import KeyBackup from "$lib/components/KeyBackup.svelte"
  import type {Scope} from "$lib/core"
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

  const tiers = $derived([
    {
      id: "accept",
      label: "What you accept",
      detail: "Whose notes your phone takes in from phones nearby.",
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
    Your phone trades things only with phones near it. These settings control when strangers' phones
    can find yours.
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
        How long after you put your phone away it keeps letting strangers find it. Lower is more
        private, and higher meets more people.
      </p>
    </div>

    <div class="space-y-1.5">
      <Label for="budget">How many strangers a day your phone tells who you are</Label>
      <Input
        id="budget"
        type="number"
        min="0"
        value={$policy.disclosure_budget}
        onchange={event => setDisclosureBudget(event.currentTarget.value)} />
      <p class="text-xs text-pretty text-muted-foreground">
        Meeting somebody new means telling them who you are, but somebody who kept asking could
        track you. Your phone stops answering strangers after this many in a day. People you pair
        with by hand do not count.
      </p>
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
        Counted from the last time anybody handed it to you, so something people keep passing on
        stays. Your own things and your bookmarks are never dropped.
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

<div class="mt-4">
  <KeyBackup />
</div>

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
