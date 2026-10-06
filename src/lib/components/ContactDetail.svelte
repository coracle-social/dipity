<script lang="ts">
  import ArrowLeft from "@lucide/svelte/icons/arrow-left"
  import {Button} from "$lib/components/ui/button"
  import {Input} from "$lib/components/ui/input"
  import {Label} from "$lib/components/ui/label"
  import {Separator} from "$lib/components/ui/separator"
  import {Switch} from "$lib/components/ui/switch"
  import {
    forget,
    name,
    nameOf,
    setBlocked,
    setMuted,
    setTrusted,
    short,
    social,
  } from "$lib/data/contacts"
  import {back, go} from "$lib/data/nav"

  let {pubkey}: {pubkey: string} = $props()

  const contact = $derived($social.people.get(pubkey))

  const named = $derived(nameOf($social, pubkey))

  // Deriving this would reset the field every time an arriving event re-collates the people.
  let typed = $state<string | undefined>(undefined)

  const renaming = $derived(typed ?? contact?.petname ?? "")

  const rename = async () => {
    await name(pubkey, renaming.trim())
    typed = undefined
  }

  let forgetting = $state(false)

  // Somebody is worth forgetting while the user has named, trusted or muted them.
  const known = $derived(Boolean(contact?.petname || contact?.trusted || contact?.muted))

  const confirmForget = async () => {
    forgetting = false
    typed = undefined
    await forget(pubkey)
    go({at: "people"})
  }

  $effect(() => {
    void pubkey
    typed = undefined
    forgetting = false
  })

  const controls = $derived([
    {
      id: "trusted",
      label: "Trusted",
      detail: "They can pass your posts on to the people they meet.",
      on: Boolean(contact?.trusted),
      set: (on: boolean) => setTrusted(pubkey, on),
    },
    {
      id: "muted",
      label: "Muted",
      detail: "Their posts are hidden from you, but your phone still passes them on.",
      on: Boolean(contact?.muted),
      set: (on: boolean) => setMuted(pubkey, on),
    },
    {
      id: "blocked",
      label: "Blocked",
      detail: "Your phone won't connect to theirs, and deletes anything of theirs it receives.",
      on: Boolean(contact?.blocked),
      set: (on: boolean) => setBlocked(pubkey, on),
    },
  ])
</script>

<header class="flex items-center gap-1 pt-4 pb-3">
  <Button variant="ghost" size="icon-sm" aria-label="Back" onclick={back}>
    <ArrowLeft />
  </Button>
  <h1 class="min-w-0 truncate text-2xl font-semibold">{named.name}</h1>
</header>

<p class="font-mono text-xs text-muted-foreground">{short(pubkey)}</p>

<div class="mt-6 space-y-2">
  <Label for="petname">Your name for them</Label>
  <div class="flex gap-2">
    <Input
      id="petname"
      value={renaming}
      oninput={event => (typed = event.currentTarget.value)}
      placeholder={named.name}
      autocomplete="off" />
    <Button
      variant="secondary"
      disabled={!renaming.trim() || renaming.trim() === contact?.petname}
      onclick={rename}>
      Save
    </Button>
  </div>
</div>

{#if contact?.aliases.length}
  <div class="mt-6">
    <h2 class="text-xs font-semibold tracking-widest text-muted-foreground uppercase">
      Also known as
    </h2>
    <ul class="mt-2 space-y-1">
      {#each contact.aliases as alias (alias.by)}
        <li class="text-sm">
          {alias.petname}
          <span class="text-xs text-muted-foreground">
            according to {nameOf($social, alias.by).name}
          </span>
        </li>
      {/each}
    </ul>
  </div>
{/if}

<Separator class="my-6" />

<ul class="space-y-4">
  {#each controls as control (control.id)}
    <li class="flex items-start justify-between gap-4">
      <div class="min-w-0">
        <Label for={control.id} class="text-sm font-semibold">{control.label}</Label>
        <p class="mt-0.5 text-xs text-pretty text-muted-foreground">{control.detail}</p>
      </div>
      <Switch id={control.id} checked={control.on} onCheckedChange={control.set} />
    </li>
  {/each}
</ul>

{#if known}
  <Separator class="my-6" />

  <div>
    <h2 class="text-sm font-semibold">Forget them</h2>
    <p class="mt-0.5 text-xs text-pretty text-muted-foreground">
      Clears your name, trust and mute for them, and your phones meet as strangers next time.
    </p>
    <div class="mt-3 flex justify-end gap-2">
      {#if forgetting}
        <Button variant="ghost" size="sm" onclick={() => (forgetting = false)}>Cancel</Button>
        <Button variant="destructive" size="sm" onclick={confirmForget}>
          Forget {named.name}
        </Button>
      {:else}
        <Button variant="outline" size="sm" onclick={() => (forgetting = true)}>Forget</Button>
      {/if}
    </div>
  </div>
{/if}
