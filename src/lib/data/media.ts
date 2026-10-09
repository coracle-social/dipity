// Pictures: made small enough to cross a radio, and drawn once they have.
//
// A picture is re-encoded before it is published, which also drops whatever
// the camera wrote into the file, a location included. It goes out as one
// blob, which the core describes for its `imeta` tag. Nothing is drawn from a
// blob until the core has all of it. `docs/ui.md#images`.

import {Capacitor} from "@capacitor/core"
import {readable, type Readable} from "svelte/store"
import type {Imeta} from "@welshman/domain"
import {Dip, type Blob} from "$lib/core"
import {encodeFile, type Encoded} from "$lib/data/encode"
import type {Social} from "$lib/data/contacts"
import {answering, remembered, storedBlobs} from "$lib/data/query"

/** Shrink in a worker, which fails where the webview cannot draw without a document. */
const inWorker = (file: File) =>
  new Promise<Encoded>((resolve, reject) => {
    const worker = new Worker(new URL("./encode.worker.ts", import.meta.url), {type: "module"})

    worker.onmessage = ({data}: MessageEvent<{shrunk?: Encoded; error?: string}>) => {
      worker.terminate()

      if (data.shrunk) resolve(data.shrunk)
      else reject(new Error(data.error))
    }

    worker.onerror = error => {
      worker.terminate()
      reject(error)
    }

    worker.postMessage(file)
  })

/** A picked file as the image to publish and the preview standing in for it, off the page's thread where possible. */
export const shrink = (file: File): Promise<Encoded> =>
  inWorker(file).catch(error => {
    console.warn("shrinking in a worker failed, trying on the page", describe(error))

    return encodeFile(file)
  })

/** An error as text that survives a webview console, which prints a DOMException as its class name alone. */
export const describe = (error: unknown) =>
  error instanceof Error ? `${error.name}: ${error.message}` : String(error)

/** The `imeta` attachment for a picture: the core's description of its bytes, and what it is. */
export const attachment = async (picture: Encoded): Promise<Imeta> => {
  const {entries} = await Dip.mediaTags({media: picture.base64})
  const hash = entries.find(entry => entry.startsWith("x "))?.slice(2)

  if (!hash) throw new Error("The core described the picture without a hash.")

  return {
    url: "",
    hash,
    mimeType: picture.mime,
    dim: picture.dim,
    extra: entries.filter(entry => !entry.startsWith("x ")),
  }
}

/** Whether a blob is a picture this device holds all of. */
const drawable = (blob: Blob) => blob.complete && Boolean(blob.mime_type?.startsWith("image/"))

/** The picture to draw, the largest held where a post carries more than one. */
export const pictureOf = (media: Blob[]) =>
  media.filter(drawable).sort((a, b) => (b.size ?? 0) - (a.size ?? 0))[0]

/** URLs already resolved, by hash, which never change while the blob is held. */
const resolved = new Map<string, string>()

/** A URL the webview can draw a whole blob from, re-read as blobs land until it is found. */
export const urlOf = (sha256: string | undefined): Readable<string | undefined> => {
  if (!sha256) return readable(undefined)

  const known = resolved.get(sha256)

  if (known) return readable(known)

  return answering(
    storedBlobs,
    async () => {
      const held = resolved.get(sha256)

      if (held) return held

      const url = await Dip.blobPath({sha256})
        .then(({path}) => (path ? Capacitor.convertFileSrc(path) : undefined))
        .catch(() => undefined)

      if (url) resolved.set(sha256, url)

      return url
    },
    undefined as string | undefined,
  )
}

/** Whose pictures are blurred until tapped. */
export type Blurring = "strangers" | "others" | "everyone"

export const blurring = remembered<Blurring>("display.blur", "strangers")

/**
 * Whether somebody's pictures are blurred: by people outside the network, by
 * anybody but a contact, or by everybody but the user.
 */
export const blurs = (
  who: Blurring,
  social: Social,
  identity: string | undefined,
  pubkey: string,
) => {
  if (pubkey === identity) return false
  if (who === "everyone") return true

  const contact = (key: string) => Boolean(social.people.get(key)?.petname)

  if (contact(pubkey)) return false
  if (who === "others") return true

  // Outside the network is nobody a contact has named.
  return !social.people.get(pubkey)?.aliases.some(alias => contact(alias.by))
}
