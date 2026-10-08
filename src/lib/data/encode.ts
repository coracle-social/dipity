// Re-encoding a picked picture, kept free of the app's imports so a worker can load it.
//
// Decoding a camera's full-size photo and drawing it down is the slowest thing
// the view does. `encode.worker.ts` runs this off the page's thread wherever
// the webview can draw without a document, and `media.ts` falls back to
// running it here where it cannot.

/** One re-encoded picture, base64 for the bridge, and what it is. */
export type Encoded = {base64: string; mime: string; dim: string}

/** The longest edge of the image as published, in pixels. */
const IMAGE_EDGE = 1080

/** How much a published image is compressed, as the encoder's quality. */
const IMAGE_QUALITY = 0.85

type Surface = OffscreenCanvas | HTMLCanvasElement

type Source = ImageBitmap | Surface

const base64 = (bytes: Uint8Array) => {
  let binary = ""

  for (let at = 0; at < bytes.length; at += 0x8000) {
    binary += String.fromCharCode(...bytes.subarray(at, at + 0x8000))
  }

  return btoa(binary)
}

/** A blank surface, offscreen wherever the platform has one, since a worker has no document. */
const surface = (width: number, height: number): Surface => {
  if (typeof OffscreenCanvas !== "undefined") return new OffscreenCanvas(width, height)

  const canvas = document.createElement("canvas")

  canvas.width = width
  canvas.height = height

  return canvas
}

const jpeg = (canvas: Surface, quality: number): Promise<Blob> => {
  if ("convertToBlob" in canvas) return canvas.convertToBlob({type: "image/jpeg", quality})

  return new Promise((resolve, reject) =>
    canvas.toBlob(
      encoded =>
        encoded ? resolve(encoded) : reject(new Error("The picture couldn't be encoded.")),
      "image/jpeg",
      quality,
    ),
  )
}

/** Draw `source` with its long edge at most `edge`, on white, since JPEG has no transparency. */
const draw = (source: Source, edge: number) => {
  const scale = Math.min(1, edge / Math.max(source.width, source.height))
  const width = Math.max(1, Math.round(source.width * scale))
  const height = Math.max(1, Math.round(source.height * scale))
  const canvas = surface(width, height)
  const context = canvas.getContext("2d") as
    OffscreenCanvasRenderingContext2D | CanvasRenderingContext2D | null

  if (!context) throw new Error("This phone can't draw a picture to shrink it.")

  context.imageSmoothingEnabled = true
  context.imageSmoothingQuality = "high"
  context.fillStyle = "white"
  context.fillRect(0, 0, width, height)
  context.drawImage(source, 0, 0, width, height)

  return canvas
}

const encoded = async (canvas: Surface, quality: number): Promise<Encoded> => {
  const blob = await jpeg(canvas, quality)

  return {
    base64: base64(new Uint8Array(await blob.arrayBuffer())),
    mime: "image/jpeg",
    dim: `${canvas.width}x${canvas.height}`,
  }
}

/** A picked file as the one image published for it. */
export const encodeFile = async (file: Blob): Promise<Encoded> => {
  const bitmap = await createImageBitmap(file, {imageOrientation: "from-image"})

  try {
    return await encoded(draw(bitmap, IMAGE_EDGE), IMAGE_QUALITY)
  } finally {
    bitmap.close()
  }
}
