<script lang="ts">
  import {
    parse,
    ParsedType,
    type Parsed,
    type ParsedAddress,
    type ParsedCode,
    type ParsedEvent,
    type ParsedLinkGrid,
  } from "@welshman/content"
  import Quoted from "$lib/components/Quoted.svelte"
  import QuotedAddress from "$lib/components/QuotedAddress.svelte"
  import {nameOf, social} from "$lib/data/contacts"
  import {go} from "$lib/data/nav"

  // Somebody's words, parsed by welshman and drawn element by element, never as HTML.
  let {
    event,
    class: className = "",
    gap = "mt-1.5",
  }: {
    event: {content: string; tags: string[][]}
    class?: string
    /** The space above every block after the first. */
    gap?: string
  } = $props()

  /** What stands on its own line: a run of inline elements, or something drawn as a block. */
  type Block =
    | {type: "paragraph"; parts: Parsed[]}
    | {type: "quote"; part: ParsedEvent | ParsedAddress}
    | {type: "code"; part: ParsedCode}
    | {type: "grid"; part: ParsedLinkGrid}

  const isBlockCode = (part: Parsed): part is ParsedCode =>
    part.type === ParsedType.Code && part.raw.startsWith("```")

  const blocks = $derived.by(() => {
    const blocks: Block[] = []
    let parts: Parsed[] = []

    const close = () => {
      while (parts.at(-1)?.type === ParsedType.Newline) parts.pop()
      while (parts[0]?.type === ParsedType.Newline) parts.shift()
      if (parts.length > 0) blocks.push({type: "paragraph", parts})
      parts = []
    }

    for (const part of parse(event)) {
      if (part.type === ParsedType.Event || part.type === ParsedType.Address) {
        close()
        blocks.push({type: "quote", part})
      } else if (isBlockCode(part)) {
        close()
        blocks.push({type: "code", part})
      } else if (part.type === ParsedType.LinkGrid) {
        close()
        blocks.push({type: "grid", part})
      } else if (part.type === ParsedType.Newline && part.value.length > 1) {
        close()
      } else {
        parts.push(part)
      }
    }

    close()

    return blocks
  })

  // Only the web opens from a tap; any other scheme a link names stays text.
  const opens = (url: URL) => url.protocol === "https:" || url.protocol === "http:"

  const shown = (url: URL) => {
    const path = url.pathname === "/" ? "" : url.pathname
    const whole = `${url.host}${path}${url.search}`

    return whole.length > 48 ? `${whole.slice(0, 47)}…` : whole
  }

  const anchor = "relative text-primary underline underline-offset-2 break-all"
</script>

{#snippet link(url: URL, raw: string)}
  {#if opens(url)}
    <a class={anchor} href={url.href} target="_blank" rel="noopener noreferrer nofollow">
      {shown(url)}</a>
  {:else}
    {raw}
  {/if}
{/snippet}

{#snippet inline(part: Parsed)}
  {#if part.type === ParsedType.Text || part.type === ParsedType.Newline}
    {part.value}
  {:else if part.type === ParsedType.Link}
    {@render link(part.value.url, part.raw)}
  {:else if part.type === ParsedType.Profile}
    <button
      type="button"
      class="relative font-medium text-primary"
      onclick={() => go({at: "contact", pubkey: part.value.pubkey})}>
      @{nameOf($social, part.value.pubkey).name}</button>
  {:else if part.type === ParsedType.Topic}
    <span class="font-medium text-secondary-accent">#{part.value}</span>
  {:else if part.type === ParsedType.Code}
    <code class="rounded-sm bg-muted px-1 font-mono text-xs">{part.value}</code>
  {:else if part.type === ParsedType.Emoji}
    <!-- A custom emoji's image is on somebody's server, and this app fetches nothing from one. -->
    <span class="text-muted-foreground">:{part.value.name}:</span>
  {:else if part.type === ParsedType.Email}
    <a class={anchor} href="mailto:{part.value}">{part.value}</a>
  {:else if part.type === ParsedType.Invoice}
    <a class={anchor} href="lightning:{part.value}">Lightning invoice</a>
  {:else if part.type === ParsedType.Cashu}
    <a class={anchor} href="cashu:{part.value}">Cashu token</a>
  {:else if part.type === ParsedType.Room}
    <span class="font-medium">{part.value.room}</span>
    <span class="text-muted-foreground">on {part.value.url}</span>
  {:else if part.type === ParsedType.Ellipsis}
    …
  {:else}
    {part.raw}
  {/if}
{/snippet}

<div class={className}>
  {#each blocks as block, index (index)}
    <div class={index ? gap : ""}>
      {#if block.type === "paragraph"}
        <p class="text-sm text-pretty break-words whitespace-pre-line">
          {#each block.parts as part, at (at)}{@render inline(part)}{/each}
        </p>
      {:else if block.type === "quote" && block.part.type === ParsedType.Event}
        <Quoted
          id={block.part.value.id}
          social={$social}
          absent="The post this mentions isn't on this phone." />
      {:else if block.type === "quote" && block.part.type === ParsedType.Address}
        <QuotedAddress
          kind={block.part.value.kind}
          pubkey={block.part.value.pubkey}
          identifier={block.part.value.identifier}
          social={$social}
          absent="The post this mentions isn't on this phone." />
      {:else if block.type === "code"}
        <pre class="overflow-x-auto rounded-md bg-muted px-3 py-2 font-mono text-xs">{block.part
            .value}</pre>
      {:else if block.type === "grid"}
        <ul class="space-y-1 text-sm">
          {#each block.part.value.links as entry, at (at)}
            <li>{@render link(entry.url, entry.url.href)}</li>
          {/each}
        </ul>
      {/if}
    </div>
  {/each}
</div>
