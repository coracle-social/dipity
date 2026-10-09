<script lang="ts">
	import { Drawer as DrawerPrimitive } from "vaul-svelte";

	// Hand-edited: iOS already pans a focused input above the keyboard, and vaul lifting the drawer too moved it twice as far.
	let {
		shouldScaleBackground = true,
		repositionInputs = false,
		open = $bindable(false),
		activeSnapPoint = $bindable(null),
		...restProps
	}: DrawerPrimitive.RootProps = $props();
</script>

<DrawerPrimitive.Root {shouldScaleBackground} {repositionInputs} bind:open bind:activeSnapPoint {...restProps} />
