<script lang="ts">
  import Check from "@lucide/svelte/icons/check"
  import Download from "@lucide/svelte/icons/download"
  import {Button} from "$lib/components/ui/button"
  import {Input} from "$lib/components/ui/input"
  import {Label} from "$lib/components/ui/label"
  import {Separator} from "$lib/components/ui/separator"
  import {Dip} from "$lib/core"
  import {
    policy,
    setAccept,
    setCoolOffMinutes,
    setDisclosureBudget,
    setForward,
    setGossip,
    setRetentionDays,
    setVisibility,
    type Scope,
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
      label: "Whose things you keep",
      detail:
        "A phone that comes near yours offers you what it is carrying. This is whose things you take a copy of.",
      scopes: ["trusted", "network", "lenient"] as Scope[],
      on: $policy.accept,
      set: setAccept,
    },
    {
      id: "gossip",
      label: "Whose things you carry",
      detail: "Everything you keep, you can hand on to the next phone you pass.",
      scopes: ["nothing", "trusted", "network", "lenient"] as Scope[],
      on: $policy.gossip,
      set: setGossip,
    },
    {
      id: "forward",
      label: "May share your things forward",
      detail:
        "Somebody who can show they got a thing from you may hand it on further. This is who you let do that.",
      scopes: ["nothing", "trusted", "network"] as Scope[],
      on: $policy.forward,
      set: setForward,
    },
    {
      id: "visibility",
      label: "Who you hand yours to",
      detail:
        "Whose phone you will offer your own things to at all. Your list of people is never offered, whatever this says.",
      scopes: ["trusted", "network", "lenient", "public"] as Scope[],
      on: $policy.visibility.default,
      set: setVisibility,
    },
  ])

  const words: Record<Scope, string> = {
    nothing: "Nobody",
    trusted: "People you paired with",
    network: "Their people too",
    lenient: "Anyone not blocked",
    public: "Anyone at all",
  }
</script>

<header class="pt-4 pb-3">
  <h1 class="text-2xl font-semibold">Settings</h1>
</header>

<h2 class="mt-4 text-xs font-semibold tracking-widest text-muted-foreground uppercase">Content</h2>

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
  Your phone only trades things with a phone that is in the room with it. These are the three
  numbers that decide how long it keeps looking, how often it is willing to say who you are, and how
  long what you take in sticks around.
</p>

<div class="mt-4 space-y-5">
  <div class="space-y-1.5">
    <Label for="cool-off">Minutes you stay findable after closing the app</Label>
    <Input
      id="cool-off"
      type="number"
      min="0"
      value={$policy.coolOffMinutes}
      onchange={event => setCoolOffMinutes(event.currentTarget.value)} />
    <p class="text-xs text-pretty text-muted-foreground">
      Put the phone in your pocket and it keeps trading for this long. Zero means it stops the
      moment you close the app, and you pick up nothing while you walk home.
    </p>
  </div>

  <div class="space-y-1.5">
    <Label for="budget">People a day you will introduce yourself to</Label>
    <Input
      id="budget"
      type="number"
      min="0"
      value={$policy.disclosureBudget}
      onchange={event => setDisclosureBudget(event.currentTarget.value)} />
    <p class="text-xs text-pretty text-muted-foreground">
      Trading with somebody new means telling them who you are, and somebody who kept asking could
      work out where you go. After this many in a day your phone stops answering strangers.
    </p>
  </div>

  <div class="space-y-1.5">
    <Label for="retention">Days a thing stays after you last saw it going around</Label>
    <Input
      id="retention"
      type="number"
      min="1"
      value={$policy.retentionDays}
      onchange={event => setRetentionDays(event.currentTarget.value)} />
    <p class="text-xs text-pretty text-muted-foreground">
      A phone is not an archive. Something nobody has handed you again in this long is deleted. Your
      own things stay.
    </p>
  </div>
</div>

<Separator class="my-6" />

<h2 class="text-xs font-semibold tracking-widest text-muted-foreground uppercase">Your key</h2>

{#if $session.identity}
  <p class="mt-3 font-mono text-xs break-all text-muted-foreground">
    {npubOf($session.identity)}
  </p>
{/if}

<Button class="mt-4" variant="secondary" disabled={backup === "asking"} onclick={exportKey}>
  <Download />
  Write it down somewhere safe
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
