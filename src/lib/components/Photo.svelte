<script lang="ts">
  import EyeOff from "@lucide/svelte/icons/eye-off"
  import type {Item} from "$lib/data/feed"
  import type {Social} from "$lib/data/contacts"
  import {blurring, blurs, pictureOf, urlOf} from "$lib/data/media"
  import type {Session} from "$lib/data/session"
  import {picture} from "$lib/kinds"

  // A picture post's image, held back behind a tap when it carries a warning or the user blurs its author.
  let {
    item,
    social,
    session,
    detailed = false,
  }: {
    item: Item
    social: Social
    session: Session
    /** Whether this is the post's own page, which draws the image rather than its preview. */
    detailed?: boolean
  } = $props()

  const shown = $derived(pictureOf(item.media, detailed))

  const url = $derived(urlOf(shown?.sha256))

  const read = $derived(picture.reader(item.event).parse())

  const warned = $derived(read.contentWarning())

  const covered = $derived(warned || blurs($blurring, social, session.identity, item.event.pubkey))

  let revealed = $state(false)

  const ratio = $derived.by(() => {
    const [width, height] = (shown?.dim ?? "").split("x").map(Number)

    return width && height ? `${width} / ${height}` : undefined
  })
</script>

{#if shown}
  <div class="relative overflow-hidden rounded-md bg-muted" style:aspect-ratio={ratio}>
    {#if $url}
      <img
        src={$url}
        alt={shown.alt ?? item.event.content}
        class="size-full object-cover transition-[filter] {covered && !revealed
          ? 'scale-110 blur-2xl'
          : ''}" />
    {/if}

    {#if covered && !revealed}
      <!-- Drawn over the card's own tap target, so the first tap shows the picture rather than opening the post. -->
      <button
        type="button"
        class="absolute inset-0 flex flex-col items-center justify-center gap-1.5 bg-background/30
               px-4 text-center text-sm font-medium text-foreground"
        onclick={() => (revealed = true)}>
        <EyeOff class="size-5" />
        {#if warned}
          <span class="text-pretty">{read.contentWarningReason() || "Content warning"}</span>
        {/if}
        <span class="text-xs text-muted-foreground">Tap to show</span>
      </button>
    {/if}
  </div>
{/if}
