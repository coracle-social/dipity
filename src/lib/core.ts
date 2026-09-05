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
