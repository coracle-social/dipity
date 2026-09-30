<script lang="ts">
  import ArrowLeft from "@lucide/svelte/icons/arrow-left"
  import {Button} from "$lib/components/ui/button"
  import {Input} from "$lib/components/ui/input"
  import {Label} from "$lib/components/ui/label"
  import {Separator} from "$lib/components/ui/separator"
  import {Switch} from "$lib/components/ui/switch"
  import {name, nameOf, setBlocked, setMuted, setTrusted, short, social} from "$lib/data/contacts"
  import {go} from "$lib/data/nav"

  let {pubkey}: {pubkey: string} = $props()

  const contact = $derived($social.people.get(pubkey))

  const named = $derived(nameOf($social, pubkey))

  // Deriving this would reset the field every time an arriving event re-collates the roster.
  let typed = $state<string | undefined>(undefined)

  const renaming = $derived(typed ?? contact?.petname ?? "")

  const rename = async () => {
    await name(pubkey, renaming.trim())
    typed = undefined
  }

  $effect(() => {
    void pubkey
    typed = undefined
  })

  const controls = $derived([
    {
      id: "trusted",
      label: "Trusted",
      detail: "They may share your things forward, so what you write reaches their people too.",
      on: Boolean(contact?.trusted),
      set: (on: boolean) => setTrusted(pubkey, on),
    },
    {
      id: "muted",
      label: "Muted",
      detail: "Their things stop appearing here. Your phone still carries them for other people.",
      on: Boolean(contact?.muted),
      set: (on: boolean) => setMuted(pubkey, on),
    },
    {
      id: "blocked",
      label: "Blocked",
      detail:
        "Your phone will not exchange anything with theirs, and drops whatever arrives from them.",
      on: Boolean(contact?.blocked),
      set: (on: boolean) => setBlocked(pubkey, on),
    },
  ])
</script>

<header class="flex items-center gap-1 pt-4 pb-3">
  <Button variant="ghost" size="icon-sm" aria-label="Back" onclick={() => go({at: "people"})}>
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
