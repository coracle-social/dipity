import {existsSync} from "node:fs"
import {readFile} from "node:fs/promises"
import {apkMetadata} from "../lib/android.mjs"
import {apk, missingEnv, name, notes, repository, version} from "../lib/context.mjs"
import {gitea} from "../lib/gitea.mjs"
import {dim} from "../lib/shell.mjs"

export default {
  name: "gitea",
  title: "Publish the gitea release with the APK attached, which Obtainium reads",
  missing: () => missingEnv("GITEA_TOKEN"),
  setup: [
    `Generate an access token at ${repository.origin}/user/settings/applications with the`,
    "write:repository scope, and set GITEA_TOKEN in .env.local.",
  ],
  run: async () => {
    const api = gitea({repository, token: process.env.GITEA_TOKEN})

    if (!(await api.hasTag(version))) {
      throw new Error(`${repository} has no ${version} tag; push it before publishing`)
    }

    if (!existsSync(apk)) {
      throw new Error(`${apk} is missing; run just release apk`)
    }

    await apkMetadata()

    const release = await api.upsertRelease(version, notes)
    const url = await api.attach(release.id, `${name}-${version}.apk`, await readFile(apk))

    console.log(dim(`  ${url}`))

    // A draft is invisible to Obtainium, so the release goes public only once its APK is on it
    if (release.draft) {
      await api.publish(release.id)
    }
  },
}
