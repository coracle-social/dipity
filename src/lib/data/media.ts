// Pictures: made small enough to cross a radio, and drawn once they have.
//
// A picture is re-encoded before it is published, which also drops whatever
// the camera wrote into the file, a location included. It goes out as two
// blobs, the image and a preview standing in for it, and the core describes
// each for its `imeta` tag. Nothing is drawn from a blob until the core has all
// of it. `docs/ui.md#images`.

import {Capacitor} from "@capacitor/core"
import {readable, type Readable} from "svelte/store"
import type {Imeta} from "@welshman/domain"
import {Dip, type Blob} from "$lib/core"
import {encodeFile, type Encoded, type Shrunk} from "$lib/data/encode"
import type {Social} from "$lib/data/contacts"
import {answering, remembered, storedBlobs} from "$lib/data/query"

/** Shrink in a worker, which fails where the webview cannot draw without a document. */
const inWorker = (file: File) =>
  new Promise<Shrunk>((resolve, reject) => {
    const worker = new Worker(new URL("./encode.worker.ts", import.meta.url), {type: "module"})

    worker.onmessage = ({data}: MessageEvent<{shrunk?: Shrunk; error?: string}>) => {
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
export const shrink = (file: File): Promise<Shrunk> => inWorker(file).catch(() => encodeFile(file))

/** The `imeta` attachment for a picture: the core's description of its bytes, and what it is. */
export const attachment = async (picture: Encoded, extra: string[] = []): Promise<Imeta> => {
  const {entries} = await Dip.mediaTags({media: picture.base64})
  const hash = entries.find(entry => entry.startsWith("x "))?.slice(2)

  if (!hash) throw new Error("The core described the picture without a hash.")

  return {
    url: "",
    hash,
    mimeType: picture.mime,
    dim: picture.dim,
    extra: [...entries.filter(entry => !entry.startsWith("x ")), ...extra],
  }
}

/** Whether a blob is a picture this device holds all of. */
const drawable = (blob: Blob) => blob.complete && Boolean(blob.mime_type?.startsWith("image/"))

/** The picture to draw: the preview where one will do, and the image where it is wanted. */
export const pictureOf = (media: Blob[], whole: boolean) => {
  const held = media.filter(drawable)
  const wanted = whole ? "Original" : "Preview"

  return held.find(blob => blob.role === wanted) ?? held[0]
}

/** A URL the webview can draw a whole blob from, re-read as blobs land. */
export const urlOf = (sha256: string | undefined): Readable<string | undefined> =>
  !sha256
    ? readable(undefined)
    : answering(
        storedBlobs,
        () =>
          Dip.blobPath({sha256})
            .then(({path}) => (path ? Capacitor.convertFileSrc(path) : undefined))
            .catch(() => undefined),
        undefined as string | undefined,
      )

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
