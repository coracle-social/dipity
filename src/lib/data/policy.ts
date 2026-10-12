// What the device does on its own, which is a preference because nobody is
// awake to be asked during a background wake.
//
// Nothing here restates the defaults, because the core compiles the preferences
// into a policy and answers it with them filled in. The writes go back one key
// at a time as JSON documents. `docs/policy.md`.

import {derived, get, type Readable} from "svelte/store"
import {Dip, type Policy, type Scope, type Sharing} from "$lib/core"
import {answering, storedPreferences} from "$lib/data/query"

const keys = {
  accept: "policy.accept",
  sharing: "policy.sharing",
  retentionDays: "policy.retention_days",
  discoverInBackground: "policy.discover_in_background",
  untrusted: "policy.untrusted",
  blocked: "policy.blocked",
} as const

const read = (): Promise<Policy | undefined> =>
  Dip.policy()
    .then(({policy}) => JSON.parse(policy) as Policy)
    .catch(error => {
      console.error("the policy could not be read", error)

      return undefined
    })

/**
 * The policy the core is applying, once it has said what it is.
 *
 * Undefined until the first read lands so that a screen draws nothing for a frame
 * rather than a guess. A wrong retention window reads as a thing about to be
 * dropped.
 */
export const policy: Readable<Policy | undefined> = answering(
  storedPreferences,
  read,
  undefined as Policy | undefined,
)

const write = (key: string, value: unknown) =>
  Dip.setPreference({key, value: JSON.stringify(value)})

export const setAccept = (scope: Scope) => write(keys.accept, scope)

export const setSharing = (sharing: Sharing) => write(keys.sharing, sharing)

/**
 * Write a count, ignoring anything that is not one.
 *
 * A value the core cannot decode is an error there rather than a default, so
 * every later policy read fails and no screen clears a preference. A number
 * field answers an emptied box as `""`, which reads as zero, and anything past
 * the float range as `Infinity`, which stringifies to `null` — so what a person
 * can type is narrowed here instead. `core/dip/src/db/pref/query.rs`.
 */
const writeCount = (key: string, typed: string, least: number) => {
  const count = Math.floor(Number(typed))

  if (typed.trim() && Number.isSafeInteger(count) && count >= least) return write(key, count)

  return Promise.resolve()
}

export const setRetentionDays = (typed: string) => writeCount(keys.retentionDays, typed, 1)

export const setDiscoverInBackground = (on: boolean) => write(keys.discoverInBackground, on)

/** Add somebody to a list preference or take them off it, which is written whole. */
const toggle = (key: string, list: string[], pubkey: string, on: boolean) => {
  const rest = list.filter(listed => listed !== pubkey)

  return write(key, on ? [...rest, pubkey] : rest)
}

/** The contacts the user does not trust, whose flags and withhold lists count for nothing. */
export const untrusted: Readable<string[]> = derived(policy, $policy => $policy?.untrusted ?? [])

/** Count a contact's flags and withhold list, or ignore both. `docs/policy.md#trust`. */
export const setTrusted = (pubkey: string, trusted: boolean) =>
  toggle(keys.untrusted, get(untrusted), pubkey, !trusted)

/** Send somebody nothing, or start sending to them again. */
export const setBlocked = (pubkey: string, blocked: boolean) =>
  toggle(keys.blocked, get(policy)?.blocked ?? [], pubkey, blocked)
