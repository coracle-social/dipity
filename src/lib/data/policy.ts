// What the device does on its own, which is a preference because nobody is
// awake to be asked during a background wake.
//
// The core compiles the preferences into a policy and answers it with its own
// defaults filled in, so nothing here restates them. The writes go back one key
// at a time as JSON documents. `docs/policy.md`.

import {get, type Readable} from "svelte/store"
import {Dip, type Policy, type Scope} from "$lib/core"
import {answering, storedPreferences} from "$lib/data/query"

const keys = {
  accept: "policy.accept",
  gossip: "policy.gossip",
  forward: "policy.forward",
  visibility: "policy.visibility",
  retentionDays: "policy.retention_days",
  strangersPerDay: "policy.strangers_per_day",
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
 * Undefined until the first read lands, so a screen draws nothing for a frame
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

export const setGossip = (scope: Scope) => write(keys.gossip, scope)

export const setForward = (scope: Scope) => write(keys.forward, scope)

/**
 * Write one of the three counts, ignoring anything that is not one.
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

export const setStrangersPerDay = (typed: string) => writeCount(keys.strangersPerDay, typed, 0)

/**
 * Widen or narrow who sees an event no rule covers, leaving the rules alone.
 *
 * The rules written back are the ones the core just answered, so a rule this
 * build has never heard of survives the edit.
 */
export const setVisibility = (scope: Scope) => {
  const visibility = get(policy)?.visibility

  if (visibility) return write(keys.visibility, {...visibility, default: scope})

  return Promise.resolve()
}
