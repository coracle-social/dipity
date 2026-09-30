<script lang="ts">
  import ChevronDown from "@lucide/svelte/icons/chevron-down"
  import Plus from "@lucide/svelte/icons/plus"
  import X from "@lucide/svelte/icons/x"
  import {Button} from "$lib/components/ui/button"
  import * as Drawer from "$lib/components/ui/drawer"
  import * as DropdownMenu from "$lib/components/ui/dropdown-menu"
  import {Input} from "$lib/components/ui/input"
  import {Label} from "$lib/components/ui/label"
  import {Textarea} from "$lib/components/ui/textarea"
  import {arrange, ask, boostItem, compose, write, type Item} from "$lib/data/feed"
  import {categories} from "$lib/kinds"

  let {
    open = $bindable(false),
    about,
  }: {
    open: boolean
    /** What this is passing on, or nothing for something of the user's own. */
    about?: Item
  } = $props()

  // Passing something on is already about something; everything else picks its own shape.
  const shapes = ["notes", "polls", "occasions", "articles"] as const

  type Shape = (typeof shapes)[number]

  let shape = $state<Shape>("notes")
  let content = $state("")
  let title = $state("")
  let summary = $state("")
  let where = $state("")
  let when = $state("")
  let options = $state(["", ""])
  let sending = $state(false)
  let failed = $state(false)

  const nounOf = (id: Shape) => categories.find(category => category.id === id)!

  const chosen = $derived(nounOf(shape))

  const answers = $derived(options.map(option => option.trim()).filter(Boolean))

  // Saying nothing is a boost, so passing something on is always ready to send.
  const ready = $derived.by(() => {
    if (about) return true

    if (shape === "notes") return Boolean(content.trim())

    if (shape === "polls") return Boolean(title.trim()) && answers.length >= 2

    if (shape === "occasions") return Boolean(title.trim()) && Boolean(when)

    return Boolean(title.trim()) && Boolean(content.trim())
  })

  const written = () => {
    if (about) return boostItem(about, content)

    if (shape === "notes") return write(content)

    if (shape === "polls") return ask(title.trim(), answers)

    if (shape === "occasions") {
      return arrange(title.trim(), Date.parse(when) / 1000, where.trim(), content)
    }

    return compose(title.trim(), summary.trim(), content)
  }

  // A refused publish keeps what was typed, so the drawer staying open has to say why.
  const send = async () => {
    sending = true
    failed = false

    try {
      await written()

      content = ""
      title = ""
      summary = ""
      where = ""
      when = ""
      options = ["", ""]
      open = false
    } catch (error) {
      failed = true
      console.error("it could not be published", error)
    } finally {
      sending = false
    }
  }
</script>

<Drawer.Root bind:open>
  <Drawer.Content>
    <Drawer.Header>
      <Drawer.Title>{about ? "Pass it on" : "Something to say"}</Drawer.Title>
      <Drawer.Description>
        {about
          ? "Say something about it, or send it on as it is."
          : "This goes out to whoever comes into range next. It may not arrive."}
      </Drawer.Description>
    </Drawer.Header>

    <div class="space-y-3 px-4">
      {#if about}
        <p
          class="line-clamp-2 rounded-md border border-border px-3 py-2 text-sm text-muted-foreground">
          {about.event.content}
        </p>
        <Textarea bind:value={content} class="min-h-32" placeholder="Anything to add?" />
      {:else}
        <DropdownMenu.Root>
          <DropdownMenu.Trigger>
            {#snippet child({props})}
              {@const Mark = chosen.icon}
              <Button {...props} variant="outline" class="w-full justify-between">
                <span class="flex items-center gap-2">
                  <Mark class="size-4" />
                  {chosen.noun}
                </span>
                <ChevronDown class="size-4 text-muted-foreground" />
              </Button>
            {/snippet}
          </DropdownMenu.Trigger>
          <DropdownMenu.Content>
            <DropdownMenu.RadioGroup bind:value={shape}>
              {#each shapes as id (id)}
                {@const category = nounOf(id)}
                {@const Mark = category.icon}
                <DropdownMenu.RadioItem value={id}>
                  <Mark class="size-4" />
                  {category.noun}
                </DropdownMenu.RadioItem>
              {/each}
            </DropdownMenu.RadioGroup>
          </DropdownMenu.Content>
        </DropdownMenu.Root>

        {#if shape === "notes"}
          <Textarea bind:value={content} class="min-h-32" placeholder="Type it out" />
        {:else if shape === "polls"}
          <div class="space-y-1.5">
            <Label for="poll-title">What are you asking?</Label>
            <Input id="poll-title" bind:value={title} placeholder="Saturday or Monday?" />
          </div>
          <div class="space-y-2">
            <Label>Answers to choose from</Label>
            {#each options as _, index (index)}
              <div class="flex gap-2">
                <Input bind:value={options[index]} placeholder="An answer" />
                {#if options.length > 2}
                  <Button
                    variant="ghost"
                    size="icon"
                    aria-label="Drop this answer"
                    onclick={() => (options = options.filter((__, at) => at !== index))}>
                    <X />
                  </Button>
                {/if}
              </div>
            {/each}
            <Button variant="ghost" size="sm" onclick={() => (options = [...options, ""])}>
              <Plus />
              One more
            </Button>
          </div>
        {:else if shape === "occasions"}
          <div class="space-y-1.5">
            <Label for="occasion-title">What is happening?</Label>
            <Input id="occasion-title" bind:value={title} placeholder="Sunday roast" />
          </div>
          <div class="space-y-1.5">
            <Label for="occasion-when">When</Label>
            <Input id="occasion-when" type="datetime-local" bind:value={when} />
          </div>
          <div class="space-y-1.5">
            <Label for="occasion-where">Where</Label>
            <Input id="occasion-where" bind:value={where} placeholder="The one on Ellis Street" />
          </div>
          <Textarea bind:value={content} class="min-h-20" placeholder="Anything else" />
        {:else}
          <div class="space-y-1.5">
            <Label for="article-title">Title</Label>
            <Input id="article-title" bind:value={title} placeholder="What it is called" />
          </div>
          <div class="space-y-1.5">
            <Label for="article-summary">The line people see first</Label>
            <Input id="article-summary" bind:value={summary} placeholder="One sentence" />
          </div>
          <Textarea bind:value={content} class="min-h-48" placeholder="Write it out" />
        {/if}
      {/if}
    </div>

    <Drawer.Footer>
      {#if failed}
        <p class="text-sm text-destructive">
          That did not go out, and what you wrote is still here. Try again.
        </p>
      {/if}
      <Button size="lg" disabled={!ready || sending} onclick={send}>
        {about && !content.trim() ? "Send it on as it is" : "Send it out"}
      </Button>
      <Button variant="ghost" size="lg" onclick={() => (open = false)}>Not now</Button>
    </Drawer.Footer>
  </Drawer.Content>
</Drawer.Root>
