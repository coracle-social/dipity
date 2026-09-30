// What the device does on its own, which is a preference because nobody is
// awake to be asked during a background wake.
//
// The core reads these keys and compiles them; the view writes them and
// computes nothing the core depends on. Every value is a JSON document, and the
// defaults below are the core's own restated so a screen has something to draw
// before the read lands. `docs/policy.md`.

import {get, type Readable} from "svelte/store"
import {Dip, type Pref} from "$lib/core"
import {answering, storedPreferences} from "$lib/data/query"

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

const parse = <Value>(stored: Pref | undefined, fallback: Value): Value => {
  if (!stored) return fallback

  try {
    return JSON.parse(stored.value) as Value
  } catch {
    console.error(`${stored.key} is not a JSON document`, stored.value)

    return fallback
  }
}

const read = async (): Promise<Policy> => {
  const {preferences} = await Dip.preferences().catch(() => ({preferences: [] as Pref[]}))
  const at = (key: string) => preferences.find(stored => stored.key === key)

  return {
    accept: parse(at(keys.accept), defaults.accept),
    gossip: parse(at(keys.gossip), defaults.gossip),
    forward: parse(at(keys.forward), defaults.forward),
    visibility: parse(at(keys.visibility), defaults.visibility),
    retentionDays: parse(at(keys.retentionDays), defaults.retentionDays),
    coolOffMinutes: parse(at(keys.coolOffMinutes), defaults.coolOffMinutes),
    disclosureBudget: parse(at(keys.disclosureBudget), defaults.disclosureBudget),
  }
}

export const policy: Readable<Policy> = answering(storedPreferences, read, defaults)

const write = (key: string, value: unknown) =>
  Dip.setPreference({key, value: JSON.stringify(value)})

export const setAccept = (scope: Scope) => write(keys.accept, scope)

export const setGossip = (scope: Scope) => write(keys.gossip, scope)

export const setForward = (scope: Scope) => write(keys.forward, scope)

export const setRetentionDays = (days: number) => write(keys.retentionDays, days)

export const setCoolOffMinutes = (minutes: number) => write(keys.coolOffMinutes, minutes)

export const setDisclosureBudget = (disclosures: number) =>
  write(keys.disclosureBudget, disclosures)

/** Widen or narrow who sees an event no rule covers, leaving the rules alone. */
export const setVisibility = (scope: Scope) =>
  write(keys.visibility, {...get(policy).visibility, default: scope})
