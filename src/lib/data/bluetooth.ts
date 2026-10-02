// Whether the radio can be used, which is the whole of whether anything moves.
//
// The shell reports a switch as it happens. The state is asked for again when
// the app comes back to the front, which is where a permission granted in the
// system's settings shows up, since nothing announces that.

import {readable, type Readable} from "svelte/store"
import {Dip, type Bluetooth} from "$lib/core"

export const bluetooth: Readable<Bluetooth> = readable<Bluetooth>("unknown", set => {
  let live = true
  let stop: (() => void) | undefined

  const ask = () =>
    Dip.bluetooth()
      .then(({state}) => live && set(state))
      .catch(() => undefined)

  const returned = () => {
    if (document.visibilityState === "visible") ask()
  }

  ask()
  document.addEventListener("visibilitychange", returned)

  Dip.addListener("bluetooth", ({state}) => set(state))
    .then(listener => {
      stop = () => listener.remove()

      if (!live) stop()
    })
    .catch(() => undefined)

  return () => {
    live = false
    document.removeEventListener("visibilitychange", returned)
    stop?.()
  }
})
