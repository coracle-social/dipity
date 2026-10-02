<script lang="ts">
  // The app's name as its icon draws it: the i dotted with a beacon. resources/icon.svg is the same drawing.
  let {class: className = ""}: {class?: string} = $props()
</script>

<svg viewBox="130 200 764 660" class={className} role="img" aria-label="dip">
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
  </g>
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
