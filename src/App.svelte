<script lang="ts">
  import {ModeWatcher, mode, toggleMode} from "mode-watcher"
  import {MoonIcon, RadioIcon, SunIcon, UsersIcon} from "@lucide/svelte"
  import {Button} from "$lib/components/ui/button"
  import {Badge} from "$lib/components/ui/badge"
  import {Switch} from "$lib/components/ui/switch"
  import {Separator} from "$lib/components/ui/separator"
  import * as Avatar from "$lib/components/ui/avatar"
  import * as Card from "$lib/components/ui/card"
  import * as Tabs from "$lib/components/ui/tabs"
  import {Dip} from "$lib/core"

  // Scaffolding: the token scales from src/app.css, in both themes. See docs/ui.md.

  const elevations = [
    {name: "shadow-xs", use: "Inputs and controls at rest", class: "shadow-xs"},
    {name: "shadow-sm", use: "Cards, list rows, anything in flow", class: "shadow-sm"},
    {name: "shadow-md", use: "Lifted on press or hover", class: "shadow-md"},
    {name: "shadow-lg", use: "Popovers, dropdowns, tooltips", class: "shadow-lg"},
    {name: "shadow-xl", use: "Sheets and drawers", class: "shadow-xl"},
    {name: "shadow-2xl", use: "Modal dialogs", class: "shadow-2xl"},
  ]

  const surfaces = [
    {name: "background", class: "bg-background"},
    {name: "card", class: "bg-card"},
    {name: "muted", class: "bg-muted"},
    {name: "accent", class: "bg-accent"},
    {name: "primary", class: "bg-primary"},
    {name: "secondary-accent", class: "bg-secondary-accent"},
    {name: "destructive", class: "bg-destructive"},
  ]

  let nearbyOnly = $state(true)

  // The one call that proves the shell loaded the core this workspace built.
  let core = $state("no plugin")

  $effect(() => {
    Dip.coreVersion()
      .then(({version}) => (core = `core ${version}`))
      .catch(() => (core = "no plugin"))
  })
</script>

<ModeWatcher />

<div class="min-h-svh bg-background pb-safe-b">
  <header
    class="sticky top-0 z-10 bg-background/80 pt-safe-t backdrop-blur-md
           supports-[backdrop-filter]:bg-background/60">
    <div class="mx-auto flex max-w-2xl items-center gap-3 px-5 py-4">
      <div class="flex size-9 items-center justify-center rounded-xl bg-primary/10 text-primary">
        <RadioIcon class="size-4.5" />
      </div>
      <div class="min-w-0 flex-1">
        <h1 class="truncate text-base leading-tight font-semibold">Dip</h1>
        <p class="truncate text-xs text-muted-foreground">Design system reference · {core}</p>
      </div>
      <Button variant="ghost" size="icon" onclick={toggleMode} aria-label="Toggle color scheme">
        {#if mode.current === "dark"}
          <MoonIcon />
        {:else}
          <SunIcon />
        {/if}
      </Button>
    </div>
    <Separator />
  </header>

  <main class="mx-auto max-w-2xl space-y-10 px-5 py-8">
    <section class="space-y-3">
      <h2 class="text-xs font-semibold tracking-widest text-muted-foreground uppercase">
        Elevation
      </h2>
      <div class="grid grid-cols-2 gap-3 sm:grid-cols-3">
        {#each elevations as step (step.name)}
          <div
            class="flex flex-col gap-1 rounded-2xl bg-card p-4 transition-transform
                   hover:-translate-y-0.5 {step.class}">
            <code class="text-xs font-medium">{step.name}</code>
            <span class="text-xs leading-snug text-muted-foreground">{step.use}</span>
          </div>
        {/each}
      </div>
    </section>

    <section class="space-y-3">
      <h2 class="text-xs font-semibold tracking-widest text-muted-foreground uppercase">
        Surfaces
      </h2>
      <div class="grid grid-cols-3 gap-3 sm:grid-cols-6">
        {#each surfaces as surface (surface.name)}
          <div class="space-y-1.5">
            <div class="h-14 rounded-xl border border-border shadow-xs {surface.class}"></div>
            <code class="text-xs text-muted-foreground">{surface.name}</code>
          </div>
        {/each}
      </div>
    </section>

    <section class="space-y-3">
      <h2 class="text-xs font-semibold tracking-widest text-muted-foreground uppercase">
        Components
      </h2>

      <Card.Root>
        <Card.Header>
          <Card.Title>Nearby</Card.Title>
          <Card.Description>Four people are in range right now.</Card.Description>
        </Card.Header>
        <Card.Content class="space-y-4">
          <div class="flex items-center gap-3">
            <Avatar.Root>
              <Avatar.Fallback>JS</Avatar.Fallback>
            </Avatar.Root>
            <div class="min-w-0 flex-1">
              <p class="truncate text-sm font-medium">A neighbor</p>
              <p class="truncate text-xs text-muted-foreground">Seen 2 minutes ago</p>
            </div>
            <Badge class="bg-secondary-accent text-secondary-accent-foreground">1 hop</Badge>
          </div>

          <Separator />

          <label class="flex items-center justify-between gap-4">
            <span class="space-y-0.5">
              <span class="block text-sm font-medium">Nearby only</span>
              <span class="block text-xs text-muted-foreground">
                Never forward beyond people you have met.
              </span>
            </span>
            <Switch bind:checked={nearbyOnly} />
          </label>
        </Card.Content>
        <Card.Footer class="gap-2">
          <Button>
            <UsersIcon />
            Sync now
          </Button>
          <Button variant="outline">Later</Button>
        </Card.Footer>
      </Card.Root>

      <div class="flex flex-wrap gap-2 pt-1">
        <Button size="sm">Default</Button>
        <Button size="sm" variant="secondary">Secondary</Button>
        <Button size="sm" variant="outline">Outline</Button>
        <Button size="sm" variant="ghost">Ghost</Button>
        <Button size="sm" variant="destructive">Destructive</Button>
      </div>

      <Tabs.Root value="feed" class="pt-2">
        <Tabs.List>
          <Tabs.Trigger value="feed">Feed</Tabs.Trigger>
          <Tabs.Trigger value="nearby">Nearby</Tabs.Trigger>
          <Tabs.Trigger value="you">You</Tabs.Trigger>
        </Tabs.List>
        <Tabs.Content value="feed" class="pt-3 text-sm text-muted-foreground">
          Tab panels cross-fade; they do not slide. Movement here would imply a spatial relationship
          between tabs that does not exist.
        </Tabs.Content>
        <Tabs.Content value="nearby" class="pt-3 text-sm text-muted-foreground">
          Nothing in range.
        </Tabs.Content>
        <Tabs.Content value="you" class="pt-3 text-sm text-muted-foreground">
          Your key never leaves the device.
        </Tabs.Content>
      </Tabs.Root>
    </section>
  </main>
</div>
