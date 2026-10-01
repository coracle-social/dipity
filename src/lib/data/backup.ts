// Writing the key down, which the core and the shell do on the view's behalf.
//
// The core writes the file and the shell puts it in front of the user, so the
// view never holds the key or the path. What comes back is whether anything
// took a copy, through a listener, since the sheet answers after the call that
// opened it. `docs/keys.md#backup`.

import {writable, type Readable} from "svelte/store"
import {Dip} from "$lib/core"

/** The shortest password the core will lock a backup under. */
export const MIN_PASSWORD = 12

/** Where the last export got to. */
export type BackupState = "asking" | "shared" | "dropped" | "failed"

const store = writable<BackupState | undefined>(undefined)

export const backup: Readable<BackupState | undefined> = store

let listening: Promise<unknown> | undefined

const listen = () =>
  (listening ??= Dip.addListener("keyBackupShared", ({shared}) => {
    store.set(shared ? "shared" : "dropped")
  }))

/** Forget how the last export went, for a screen that starts from nothing. */
export const resetBackup = () => store.set(undefined)

/** Write the key to a file and offer it, locked when there is a password. */
export const exportKey = async (password?: string) => {
  await listen()
  store.set("asking")

  try {
    await Dip.exportKey(password ? {password} : {})
  } catch (error) {
    store.set("failed")
    console.error("the key could not be written", error)
  }
}
