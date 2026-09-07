// The Capacitor plugin boundary, and the only way the view reaches the core.
//
// `registerPlugin` answers a proxy on every platform: in the browser there is
// no implementation behind it, so `just dev` renders the view and every call
// here rejects. That is the intended shape — the shells are where a peer, a
// radio and a key exist.
//
// Nothing here names a peer, and nothing here decides anything the core
// decides. A link is a number the shell assigned; there is no way to reach one
// except by answering something the core asked.

import {registerPlugin, type PluginListenerHandle} from "@capacitor/core"

/** A question the core put to the user, waiting on an answer. */
export type Approval = {link: number}

/** The six digits both devices show during a login-with-device. */
export type TransferPrompt = {link: number; code: number}

/** How an identity transfer ended. */
export type TransferOutcome = {link: number; received: boolean}

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

/** One stored preference. */
export type Pref = {key: string; value: string; updatedAt: number}

/** Which group of tables moved. */
export type Change = {group: "events" | "blobs" | "preferences"}

/** The core, as the webview calls it. */
export type DipCore = {
  /**
   * The version of the Rust core the shell loaded.
   *
   * The one call that proves the whole chain — cargo, uniffi, the xcframework
   * or the jniLibs, the plugin — rather than a stale library from a previous
   * build.
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

  /** Store and offer an event, with the media it attaches, base64 each. */
  publish(options: {event: string; media?: string[]}): Promise<void>

  /** Answer a `requestApproval`. */
  approve(options: {link: number; approved: boolean}): Promise<void>

  /** Stored events matching the query, each NIP-01 JSON. */
  listEvents(options?: Query): Promise<{events: string[]}>

  /**
   * The same events, each with the media it references and the peers it
   * arrived from.
   *
   * Two reads for the page rather than two per event, so a feed asks for this
   * rather than looping over `listEvents`.
   */
  listDetails(options?: Query): Promise<{details: string[]}>

  /** One stored event by id, NIP-01 JSON, or null. */
  getEvent(options: {id: string}): Promise<{event: string | null}>

  /** Blobs a stored event references and this device does not hold. */
  wantedBlobs(options?: {limit?: number}): Promise<{blobs: string[]}>

  /** One blob's metadata and transfer progress, or null. */
  getBlob(options: {sha256: string}): Promise<{blob: string | null}>

  /** Every stored event that references a hash, by id. */
  eventsReferencingBlob(options: {sha256: string}): Promise<{ids: string[]}>

  /** Every stored preference. */
  preferences(): Promise<{preferences: Pref[]}>

  /** One preference's value, as the JSON document it was written as. */
  preference(options: {key: string}): Promise<{value: string | null}>

  /**
   * Write a preference, a JSON document, and rebind live sessions under it.
   *
   * Policy is stored as preferences and compiled by the core, so the write and
   * the rebind are one call rather than two the view can get out of order.
   */
  setPreference(options: {key: string; value: string}): Promise<void>

  /** Remove a preference, so its default applies again. */
  clearPreference(options: {key: string}): Promise<{existed: boolean}>

  /**
   * Write a key backup and put it in front of the user.
   *
   * The key never crosses the bridge in either direction: the core encodes the
   * file and the shell shares it. Answers once the sheet or chooser closes,
   * and `keyBackupShared` says whether anything took it.
   */
  exportKey(options?: {password?: string}): Promise<void>

  addListener(
    event: "requestApproval",
    handler: (approval: Approval) => void,
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
   * Backing out is not downloaded rather than an error, so the screen it gates
   * stays where it is and the user can try again.
   */
  addListener(
    event: "keyBackupShared",
    handler: (backup: {shared: boolean}) => void,
  ): Promise<PluginListenerHandle>
}

export const Dip = registerPlugin<DipCore>("Dip")
