// The Capacitor plugin boundary, and the only way the view reaches the core.
//
// `registerPlugin` answers a proxy on every platform: in the browser there is
// no implementation behind it, so `just dev` renders the view and every call
// here rejects. That is the intended shape — the shells are where a peer, a
// radio and a key exist.

import {registerPlugin} from "@capacitor/core"

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
}

export const Dip = registerPlugin<DipCore>("Dip")
