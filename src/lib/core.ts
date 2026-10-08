// The Capacitor plugin boundary, and the only way the view reaches the core.
//
// `registerPlugin` answers a proxy on every platform, and on a device the
// implementation behind it is the shell. In the browser there is none, so
// `just dev` gets the simulator in `$lib/dev` instead: the same surface over an
// in-memory store and a neighborhood that walks past on a timer. A device never
// loads it — the branch below is dead code under `import.meta.env.DEV`.
//
// Nothing here names a peer, and nothing here decides anything the core
// decides. A link is a number the shell assigned; there is no way to reach one
// except by answering something the core asked.

import {registerPlugin, type PluginListenerHandle} from "@capacitor/core"
import type {HashedEvent} from "@welshman/util"

/**
 * A question the core put to the user, waiting on an answer.
 *
 * `code` is the pairing comparison value, derived from the same handshake hash
 * `TransferPrompt` derives its six digits from under a label of its own. There
 * is nothing else here to identify the peer by, because the gate runs before
 * either side has named a pubkey. The code is what the two users compare, and
 * the pet name is for the person in front of them.
 * `docs/discovery.md#the-consent-gate`.
 */
export type Approval = {link: number; code: number}

/**
 * Who the peer on a link turned out to be, once both sides have authenticated,
 * and the five shapes both users compare before naming them.
 */
/** Who proved themselves on a link, and whether this phone recognized them by their pairing. */
export type PeerIdentity = {
  link: number
  pubkey: string
  code: number
  dialed: boolean
  recognized: boolean
}

/** A link that is no longer there. Nothing can be offered over it. */
export type LinkClosed = {link: number}

/**
 * Whether Bluetooth can be used: on, switched off, not permitted, missing from
 * the phone, or not yet reported by the system.
 */
export type Bluetooth = "on" | "off" | "denied" | "unsupported" | "unknown"

/** The six digits both devices show during a login-with-device. */
export type TransferPrompt = {link: number; code: number}

/**
 * How an identity transfer ended, on whichever device is being told.
 *
 * `received` is the target holding a key it has not stored yet, `sent` the
 * source having handed its own over, and `refused` either user saying no. The
 * two devices see different endings for the same transfer.
 */
export type TransferOutcome = {link: number; outcome: "received" | "sent" | "refused"}

/** What a page of stored events is ordered by. */
export type Order = "createdAt" | "seenAt"

/**
 * How a read of stored events is narrowed.
 *
 * `filter` is NIP-01 and is the one part of a query that would be safe to hand
 * a peer. Everything else is provenance and never leaves the device, which is
 * why the core takes them as their own fields rather than off the filter.
 */
export type Query = {
  filter?: string
  seenSince?: number
  seenUntil?: number
  seenFrom?: string[]
  order?: Order
}

/**
 * One row of `event_seen`: which peer handed an event over, and when.
 *
 * Everything the core answers below is serde JSON of the Rust types, which is
 * why these fields are snake_case where the rest of this file is not.
 */
export type Sighting = {event_id: string; pubkey: string; seen_at: number}

/** One row of `event_shared`: which peer this device handed an event to, and when. */
export type Share = {event_id: string; pubkey: string; shared_at: number}

/** A blob an event references, whether or not this device holds the bytes. */
export type Blob = {
  sha256: string
  role: "Preview" | "Original"
  url: string | null
  mime_type: string | null
  size: number | null
  dim: string | null
  blurhash: string | null
  alt: string | null
  blake3: string | null
  imeta: string[]
  stored_bytes: number
  complete: boolean
  accessed_at: number | null
}

/**
 * What `listDetails` answers per event: the thing, its media, and everywhere it
 * has been in both directions.
 *
 * A stored event is a `HashedEvent`, the same name coracle-lib gives it,
 * because content events are never signed — authorship is shown to one peer at
 * a time instead. `docs/proofs.md#events-are-not-signed`.
 */
export type EventDetail = {
  event: HashedEvent
  blobs: Blob[]
  sightings: Sighting[]
  shares: Share[]
}

/** One stored preference. */
export type Pref = {key: string; value: string; updatedAt: number}

/** The tiers every setting is expressed on, narrowest first. */
export type Scope = "nothing" | "contacts" | "network" | "lenient" | "public"

/** Who is handed the user's own activity, and who may carry it further. `docs/policy.md#sharing`. */
export type Sharing = "contacts" | "network" | "anyone"

/** A span of the local day the device is findable in, in minutes from midnight. */
export type Window = {start: number; end: number}

/**
 * Everything the user has said about who gets what, as the core compiled it.
 *
 * The defaults are already filled in, which is why nothing above the bridge
 * carries a copy of them. `Policy::new` is where they live.
 */
export type Policy = {
  accept: Scope
  sharing: Sharing
  retention_days: number
  quiet_times: Window[]
  discover_in_background: boolean
}

/** What the phone says about letting the app notify. */
export type NotificationPermission = "granted" | "denied" | "prompt"

/**
 * Which group of tables moved.
 *
 * This union restates the core's names rather than deciding them. They are
 * answered by `change_name` and put on the event by whichever shell is running.
 */
export type Change = {group: "events" | "blobs" | "preferences"}

/** The core, as the webview calls it. */
export type DipCore = {
  /**
   * The version of the Rust core the shell loaded.
   *
   * The one call that proves the whole chain — cargo, uniffi, the linked
   * library, the plugin — rather than a stale library from a previous build.
   */
  coreVersion(): Promise<{version: string}>

  /**
   * Whether this device has an identity yet.
   *
   * The first-run question, and it is answered without reading the key.
   */
  hasIdentity(): Promise<{exists: boolean}>

  /** Generate an identity and put it in secure storage. */
  createIdentity(): Promise<{npub: string}>

  /** Store an identity the user pasted in. */
  importIdentity(options: {nsec: string}): Promise<{npub: string}>

  /** Forget the identity. Answers whether there was one. */
  deleteIdentity(): Promise<{existed: boolean}>

  /**
   * Open the store and the node, and start the radio.
   *
   * Called once there is an identity, which is what makes first run a screen
   * rather than a failed open. Everything after this happens on its own.
   */
  start(): Promise<{identity: string}>

  /**
   * The `imeta` entries an event has to carry for a peer to fetch `media` and
   * check what it gets, base64 in.
   *
   * This is the one node call that needs no started core, because describing
   * bytes stores nothing. It is also the only way media reaches an event: `publish`
   * reads `imeta` off what the view already signed, and the BLAKE3 root has to
   * be in the tag before the id is computed.
   */
  mediaTags(options: {media: string}): Promise<{entries: string[]}>

  /** Store and offer an event, with the media it attaches, base64 each. */
  publish(options: {event: string; media?: string[]}): Promise<void>

  /** Answer a `requestApproval`. */
  approve(options: {link: number; approved: boolean}): Promise<void>

  /** Stored events matching the query, each NIP-01 JSON. */
  listEvents(options?: Query): Promise<{events: string[]}>

  /**
   * The same events, each with the media it references, the peers it arrived
   * from and the peers it has been handed to.
   *
   * A feed asks for this rather than looping over `listEvents`, because it is
   * three reads for the page rather than three per event.
   */
  listDetails(options?: Query): Promise<{details: string[]}>

  /** One stored event by id, NIP-01 JSON, or null. */
  getEvent(options: {id: string}): Promise<{event: string | null}>

  /**
   * Drop one event from this device. Answers whether it was there.
   *
   * Local and silent: the sightings, who it was handed to, the author's
   * signature and the media go with it and no peer is told. Asking the network to forget something is a
   * kind 5 through `publish`, which only its author can make.
   */
  forgetEvent(options: {id: string}): Promise<{existed: boolean}>

  /** Stop recognizing somebody's device until the two next sync. Answers whether a pairing was held. */
  forgetPairing(options: {pubkey: string}): Promise<{existed: boolean}>

  /** Put an event in the trash, retracting it at once with a kind 5 if the user wrote it. */
  trash(options: {id: string}): Promise<void>

  /** Take an event back out of the trash, restoring it for peers too with a kind 5 of its kind 5 if the user wrote it. */
  restore(options: {id: string}): Promise<void>

  /** What is in the trash, newest first, each as `{id, trashed_at, retracted}` JSON. */
  trashed(): Promise<{trashed: string[]}>

  /** Delete everything in the trash from this device, retracting the user's own not yet retracted. */
  emptyTrash(): Promise<void>

  /** Whether the phone lets the app notify. */
  notificationPermission(): Promise<{permission: NotificationPermission}>

  /** Ask the phone to let the app notify, if the user has not been asked. */
  requestNotificationPermission(): Promise<{permission: NotificationPermission}>

  /** Blobs a stored event references and this device does not hold. */
  wantedBlobs(options?: {limit?: number}): Promise<{blobs: string[]}>

  /** One blob's metadata and transfer progress, or null. */
  getBlob(options: {sha256: string}): Promise<{blob: string | null}>

  /** The file holding a blob's bytes once all of them are here, or null. */
  blobPath(options: {sha256: string}): Promise<{path: string | null}>

  /** Every stored event that references a hash, by id. */
  eventsReferencingBlob(options: {sha256: string}): Promise<{ids: string[]}>

  /**
   * Everything the user has said about who gets what, JSON, defaults filled in.
   *
   * A screen shows what the gossip path obeys, because this is the compiled
   * policy every live session is bound to. `docs/policy.md`.
   */
  policy(): Promise<{policy: string}>

  /** Every stored preference. */
  preferences(): Promise<{preferences: Pref[]}>

  /** One preference's value, as the JSON document it was written as. */
  preference(options: {key: string}): Promise<{value: string | null}>

  /**
   * Write a preference, a JSON document, and rebind live sessions under it.
   *
   * The write and the rebind are one call rather than two the view can get out
   * of order, because policy is stored as preferences and compiled by the core.
   */
  setPreference(options: {key: string; value: string}): Promise<void>

  /** Remove a preference so that its default applies again. */
  clearPreference(options: {key: string}): Promise<{existed: boolean}>

  /**
   * Write a key backup and put it in front of the user.
   *
   * The key never crosses the bridge in either direction: the core encodes the
   * file and the shell shares it. Answers once the sheet or chooser closes,
   * and `keyBackupShared` says whether anything took it.
   */
  exportKey(options?: {password?: string}): Promise<void>

  /**
   * Offer this device's identity to the peer on a link.
   *
   * Both ends are then asked to compare the six digits a
   * `confirmIdentityTransfer` carries, and either may answer first.
   */
  offerIdentity(options: {link: number}): Promise<void>

  /** Answer a `confirmIdentityTransfer`. */
  answerIdentityTransfer(options: {link: number; confirmed: boolean}): Promise<void>

  /**
   * Adopt the identity an `identityTransfer` of `received` announced.
   *
   * The key never crosses the bridge: the shell writes it to secure storage and
   * reopens the core under it. This answers what `start` answers.
   */
  takeTransferredIdentity(options: {link: number}): Promise<{identity: string}>

  /**
   * Whether the view has anywhere to go back to.
   *
   * Android closes an app on a back press nothing claims, which is right at the
   * root and wrong everywhere else. The shell claims the press only while this
   * is true. iOS has no such button and does nothing with it.
   */
  setCanGoBack(options: {can: boolean}): Promise<void>

  /** Whether Bluetooth can be used now. Changes arrive as `bluetooth` events. */
  bluetooth(): Promise<{state: Bluetooth}>

  /**
   * The user pressed the phone's own back button.
   *
   * Only ever sent while `setCanGoBack` last said there was somewhere to go, so
   * the view leaves the top thing rather than deciding whether to.
   */
  addListener(event: "backPressed", handler: () => void): Promise<PluginListenerHandle>
  /** Bluetooth was switched on or off, or its permission changed. */
  addListener(
    event: "bluetooth",
    handler: (power: {state: Bluetooth}) => void,
  ): Promise<PluginListenerHandle>
  addListener(
    event: "requestApproval",
    handler: (approval: Approval) => void,
  ): Promise<PluginListenerHandle>
  /**
   * The peer on a link named a pubkey.
   *
   * A pet name is entered at the gate, which is before anyone has identified
   * themselves. This is what binds the two together.
   */
  addListener(
    event: "peerIdentified",
    handler: (peer: PeerIdentity) => void,
  ): Promise<PluginListenerHandle>
  /**
   * A link went down, whoever was on it and whichever end ended it: the radio
   * losing it, or the core closing it, a lapsed consent gate included.
   *
   * This is so a screen offering something over a named link stops offering it.
   */
  addListener(
    event: "linkClosed",
    handler: (closed: LinkClosed) => void,
  ): Promise<PluginListenerHandle>
  addListener(
    event: "confirmIdentityTransfer",
    handler: (prompt: TransferPrompt) => void,
  ): Promise<PluginListenerHandle>
  addListener(
    event: "identityTransfer",
    handler: (outcome: TransferOutcome) => void,
  ): Promise<PluginListenerHandle>
  /**
   * Writes landed in this group, coalesced.
   *
   * The group, not the rows: the view re-reads the query it is showing, and
   * carrying the change would be a second path to the same data.
   */
  addListener(
    event: "storeChanged",
    handler: (change: Change) => void,
  ): Promise<PluginListenerHandle>
  /**
   * Whether an app took the backup, or the user backed out.
   *
   * Backing out is not downloaded rather than an error. The screen it gates
   * stays where it is and the user can try again.
   */
  addListener(
    event: "keyBackupShared",
    handler: (backup: {shared: boolean}) => void,
  ): Promise<PluginListenerHandle>
}

const web = import.meta.env.DEV
  ? {web: () => import("$lib/dev/simulator").then(({simulated}) => simulated())}
  : {}

export const Dip = registerPlugin<DipCore>("Dip", web)
