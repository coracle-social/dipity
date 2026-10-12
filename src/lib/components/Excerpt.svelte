<script lang="ts">
  import {firstLine} from "$lib/kinds"

  // What stands for an article wherever it is not open: its first line, cut to two.
  let {content, class: className = ""}: {content: string; class?: string} = $props()
</script>

<span class="line-clamp-2 text-sm text-pretty {className}">{firstLine(content)}</span>
