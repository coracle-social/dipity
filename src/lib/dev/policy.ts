// The core's policy defaults, and the preferences written over them.
//
// This is the one copy of them outside Rust, and it is here rather than in the
// view because the simulator stands in for the core: `Policy::new` and
// `Visibility::default` in `core/dip/src/model/` are what it mirrors, the way
// `kinds.ts` mirrors a kind `@welshman/domain` does not model.
//
// Dev only. A device has the shell behind the bridge and never loads this.

import type {Policy, Scope, Visibility, Window} from "$lib/core"
import type {Store} from "$lib/dev/store"

const defaults: Policy = {
  accept: "lenient",
  gossip: "network",
  forward: "trusted",
  visibility: {
    // Mute, trust and block to people the user paired with; the bookmark list to nobody.
    rules: [
      {filter: {kinds: [10_000, 16_017, 16_018]}, scope: "trusted"},
      {filter: {kinds: [10_003]}, scope: "nothing"},
    ],
    default: "lenient",
  },
  retention_days: 30,
  quiet_times: [],
  discover_in_background: true,
}

/** One preference's document, or the default where it has never been written. */
const at = <Value>(store: Store, key: string, fallback: Value): Value => {
  const written = store.pref(key)

  if (written === null) return fallback

  return JSON.parse(written) as Value
}

/** Everything the user has said about who gets what, as the core would compile it. */
export const compiled = (store: Store): Policy => ({
  accept: at<Scope>(store, "policy.accept", defaults.accept),
  gossip: at<Scope>(store, "policy.gossip", defaults.gossip),
  forward: at<Scope>(store, "policy.forward", defaults.forward),
  visibility: at<Visibility>(store, "policy.visibility", defaults.visibility),
  retention_days: at(store, "policy.retention_days", defaults.retention_days),
  quiet_times: at<Window[]>(store, "policy.quiet_times", defaults.quiet_times),
  discover_in_background: at(
    store,
    "policy.discover_in_background",
    defaults.discover_in_background,
  ),
})
