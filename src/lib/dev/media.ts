// Bytes the simulated core is handed, and how it describes them.
//
// Shared by the simulator, which describes what the view attaches, and the
// staged street, which attaches its own photographs.

import {blake3} from "@noble/hashes/blake3.js"
import {sha256} from "@noble/hashes/sha2.js"
import {bytesToHex} from "@noble/hashes/utils.js"

export const bytes = (base64: string) =>
  Uint8Array.from(atob(base64), letter => letter.charCodeAt(0))

/** The three `imeta` entries the core answers, computed the same way it does. */
export const describe = (media: Uint8Array) => [
  `x ${bytesToHex(sha256(media))}`,
  `size ${media.length}`,
  `blake3 ${bytesToHex(blake3(media))}`,
]
