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
import type {Social} from "$lib/data/contacts"
import {answering, remembered, storedBlobs} from "$lib/data/query"

/** One re-encoded picture, base64 for the bridge, and what it is. */
export type Encoded = {base64: string; mime: string; dim: string}

/** The longest edge of the image as published, in pixels. */
const IMAGE_EDGE = 1280

/** How much a published image is compressed, as the encoder's quality. */
const IMAGE_QUALITY = 0.72

/** The longest edge of the preview the board draws, in pixels. */
const PREVIEW_EDGE = 640

/** How much a preview is compressed. */
const PREVIEW_QUALITY = 0.6

const base64 = (bytes: Uint8Array) => {
  let binary = ""

  for (let at = 0; at < bytes.length; at += 0x8000) {
    binary += String.fromCharCode(...bytes.subarray(at, at + 0x8000))
  }

  return btoa(binary)
}

const encode = async (bitmap: ImageBitmap, edge: number, quality: number): Promise<Encoded> => {
  const scale = Math.min(1, edge / Math.max(bitmap.width, bitmap.height))
  const width = Math.max(1, Math.round(bitmap.width * scale))
  const height = Math.max(1, Math.round(bitmap.height * scale))
  const canvas = document.createElement("canvas")
  const context = canvas.getContext("2d")

  if (!context) throw new Error("This phone can't draw a picture to shrink it.")

  canvas.width = width
  canvas.height = height
  // JPEG has no transparency, which would otherwise come out black.
  context.fillStyle = "white"
  context.fillRect(0, 0, width, height)
  context.drawImage(bitmap, 0, 0, width, height)

  const blob = await new Promise<globalThis.Blob>((resolve, reject) =>
    canvas.toBlob(
      encoded =>
        encoded ? resolve(encoded) : reject(new Error("The picture couldn't be encoded.")),
      "image/jpeg",
      quality,
    ),
  )

  return {
    base64: base64(new Uint8Array(await blob.arrayBuffer())),
    mime: "image/jpeg",
    dim: `${width}x${height}`,
  }
}

/** A picked file as the image to publish and the preview standing in for it. */
export const shrink = async (file: File) => {
  const bitmap = await createImageBitmap(file, {imageOrientation: "from-image"})

  try {
    return {
      image: await encode(bitmap, IMAGE_EDGE, IMAGE_QUALITY),
      preview: await encode(bitmap, PREVIEW_EDGE, PREVIEW_QUALITY),
    }
  } finally {
    bitmap.close()
  }
}

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
