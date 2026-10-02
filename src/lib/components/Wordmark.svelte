<script lang="ts">
  // The app's name as its icon draws it: the first i dotted with a beacon. resources/icon.svg is the same drawing.
  let {class: className = ""}: {class?: string} = $props()
</script>

<svg viewBox="130 200 1450 660" class={className} role="img" aria-label="dipity">
  <g
    class="text-foreground"
    fill="none"
    stroke="currentColor"
    stroke-width="64"
    stroke-linecap="round">
    <circle cx="282" cy="560" r="112" />
    <line x1="394" y1="300" x2="394" y2="672" />
    <line x1="512" y1="458" x2="512" y2="672" />
    <circle cx="742" cy="560" r="112" />
    <line x1="630" y1="458" x2="630" y2="820" />
    <line x1="972" y1="458" x2="972" y2="672" />
    <path d="M1110 330 V600 A72 72 0 0 0 1182 672" />
    <line x1="1040" y1="458" x2="1186" y2="458" />
    <path d="M1290 458 V560 A112 112 0 0 0 1514 560 V458 M1514 560 V740 A80 80 0 0 1 1434 820" />
  </g>
  <circle class="text-foreground" cx="972" cy="340" r="42" fill="currentColor" />
  <g class="text-primary">
    <circle cx="512" cy="340" r="42" fill="currentColor" />
    <circle
      cx="512"
      cy="340"
      r="86"
      fill="none"
      stroke="currentColor"
      stroke-width="16"
      opacity="0.45" />
    <circle
      cx="512"
      cy="340"
      r="130"
      fill="none"
      stroke="currentColor"
      stroke-width="12"
      opacity="0.2" />
  </g>
</svg>
