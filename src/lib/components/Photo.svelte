<script lang="ts">
  import EyeOff from "@lucide/svelte/icons/eye-off"
  import ImageIcon from "@lucide/svelte/icons/image"
  import type {Item} from "$lib/data/feed"
  import type {Social} from "$lib/data/contacts"
  import {blurring, blurs, pictureOf, urlOf} from "$lib/data/media"
  import type {Session} from "$lib/data/session"

  // A picture post's image, held back behind a tap when the user blurs its author, or a placeholder until all of it arrives.
  let {
    item,
    social,
    session,
  }: {
    item: Item
    social: Social
    session: Session
  } = $props()

  const shown = $derived(pictureOf(item.media))

  // Keyed on the hash alone, since the item is a new object on every store change and a new URL store starts empty.
  const sha256 = $derived(shown?.sha256)

  const url = $derived(urlOf(sha256))

  const covered = $derived(blurs($blurring, social, session.identity, item.event.pubkey))

  let revealed = $state(false)

  const ratio = $derived.by(() => {
    const [width, height] = ((shown ?? item.media[0])?.dim ?? "").split("x").map(Number)

    return width && height ? `${width} / ${height}` : undefined
  })
</script>

<div class="relative overflow-hidden rounded-md bg-muted" style:aspect-ratio={ratio}>
  {#if shown && $url}
    <img
      src={$url}
      alt={shown.alt ?? item.event.content}
      class="size-full object-cover transition-[filter] {covered && !revealed
        ? 'scale-110 blur-2xl'
        : ''}" />
  {:else if !shown}
    <div
      class="flex size-full flex-col items-center justify-center gap-2.5 bg-radial
             from-background/50 to-muted px-4 text-center inset-shadow-clay
             {ratio ? '' : 'min-h-40'}">
      <span
        class="flex size-10 items-center justify-center rounded-full bg-card/70 shadow-xs ring-1 ring-border">
        <ImageIcon class="size-4.5 text-muted-foreground/70" strokeWidth={1.5} />
      </span>
      <span class="text-xs text-muted-foreground/70">Waiting for the image</span>
    </div>
  {/if}

  {#if shown && covered && !revealed}
    <!-- Drawn over the card's own tap target so that the first tap shows the picture rather than opening the post. -->
    <button
      type="button"
      class="absolute inset-0 flex flex-col items-center justify-center gap-1.5 bg-background/30
             px-4 text-center text-sm font-medium text-foreground"
      onclick={() => (revealed = true)}>
      <EyeOff class="size-5" />
      <span class="text-xs text-muted-foreground">Tap to show</span>
    </button>
  {/if}
</div>
