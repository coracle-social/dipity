// What the device does on its own, which is a preference because nobody is
// awake to be asked during a background wake.
//
// The core reads these keys and compiles them; the view writes them and
// computes nothing the core depends on. Every value is a JSON document, and the
// defaults below are the core's own restated so a screen has something to draw
// before the read lands. `docs/policy.md`.

import {get, type Readable} from "svelte/store"
import {Dip, type Pref} from "$lib/core"
import {answering, parsed, storedPreferences} from "$lib/data/query"

/** The tiers every setting is expressed on, narrowest first. */
export type Scope = "nothing" | "trusted" | "network" | "lenient" | "public"

/** Who may see an event no rule matches, and the rules that come first. */
export type Visibility = {rules: {filter: unknown; scope: Scope}[]; default: Scope}

/** Everything the settings screen edits. */
export type Policy = {
  accept: Scope
  gossip: Scope
  forward: Scope
  visibility: Visibility
  retentionDays: number
  coolOffMinutes: number
  disclosureBudget: number
}

/**
 * The core's own defaults, from `Policy::new`.
 *
 * The visibility rule is part of them rather than an empty list: an edit writes
 * the whole document back, and a missing rule would publish the user's trust
 * list to anyone who connects.
 */
const defaults: Policy = {
  accept: "lenient",
  gossip: "network",
  forward: "trusted",
  visibility: {
    rules: [{filter: {kinds: [10_000, 16_017, 16_018]}, scope: "trusted"}],
    default: "public",
  },
  retentionDays: 30,
  coolOffMinutes: 10,
  disclosureBudget: 10,
}

const keys = {
  accept: "policy.accept",
  gossip: "policy.gossip",
  forward: "policy.forward",
  visibility: "policy.visibility",
  retentionDays: "policy.retention_days",
  coolOffMinutes: "policy.cool_off_minutes",
  disclosureBudget: "policy.disclosure_budget",
} as const

const read = async (): Promise<Policy> => {
  const {preferences} = await Dip.preferences().catch(() => ({preferences: [] as Pref[]}))
  const at = <Value>(key: string, fallback: Value) =>
    parsed(key, preferences.find(stored => stored.key === key)?.value ?? null, fallback)

  return {
    accept: at(keys.accept, defaults.accept),
    gossip: at(keys.gossip, defaults.gossip),
    forward: at(keys.forward, defaults.forward),
    visibility: at(keys.visibility, defaults.visibility),
    retentionDays: at(keys.retentionDays, defaults.retentionDays),
    coolOffMinutes: at(keys.coolOffMinutes, defaults.coolOffMinutes),
    disclosureBudget: at(keys.disclosureBudget, defaults.disclosureBudget),
  }
}

export const policy: Readable<Policy> = answering(storedPreferences, read, defaults)

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

export const setCoolOffMinutes = (typed: string) => writeCount(keys.coolOffMinutes, typed, 0)

export const setDisclosureBudget = (typed: string) => writeCount(keys.disclosureBudget, typed, 0)

/** Widen or narrow who sees an event no rule covers, leaving the rules alone. */
export const setVisibility = (scope: Scope) =>
  write(keys.visibility, {...get(policy).visibility, default: scope})
