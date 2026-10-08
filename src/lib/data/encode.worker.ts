// Shrinks one picked picture off the page's thread, answering the result or the failure.

import {encodeFile} from "./encode"

self.onmessage = async ({data}: MessageEvent<Blob>) => {
  try {
    self.postMessage({shrunk: await encodeFile(data)})
  } catch (error) {
    self.postMessage({error: String(error)})
  }
}
