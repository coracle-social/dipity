<script lang="ts">
  import {Button} from "$lib/components/ui/button"
  import * as Drawer from "$lib/components/ui/drawer"
  import {declineOffer, enableAll, offering} from "$lib/data/notifications"

  // Dismissing by dragging the sheet away is a no as well, so the offer is not made again.
  const changed = (open: boolean) => {
    if (!open) declineOffer()
  }
</script>

<Drawer.Root open={$offering} onOpenChange={changed}>
  <Drawer.Content>
    <Drawer.Header>
      <Drawer.Title>Hear about it when you are away?</Drawer.Title>
      <Drawer.Description>
        Your phone keeps trading with the people around you while it is in your pocket. It can tell
        you when somebody nearby wants to pair, and when new things reach the board.
      </Drawer.Description>
    </Drawer.Header>

    <Drawer.Footer>
      <div class="flex gap-2">
        <Button variant="ghost" size="lg" class="flex-1" onclick={declineOffer}>Not now</Button>
        <Button size="lg" class="flex-1" onclick={enableAll}>Turn on</Button>
      </div>
    </Drawer.Footer>
  </Drawer.Content>
</Drawer.Root>
