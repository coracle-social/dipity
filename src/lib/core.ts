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
}

export const Dip = registerPlugin<DipCore>("Dip")
