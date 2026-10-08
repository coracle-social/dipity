#!/usr/bin/env node
// Sets the version in package.json, android and ios, and advances each platform's build number
// only when its version actually changes. Rerunning with the same version is a no-op.
import {readFileSync, writeFileSync} from "node:fs"
import {join} from "node:path"
import {fileURLToPath} from "node:url"

const root = fileURLToPath(new URL("../../", import.meta.url))
const arg = process.argv[2]
const read = path => readFileSync(join(root, path), "utf-8")
const write = (path, text) => writeFileSync(join(root, path), text)

const pkg = JSON.parse(read("package.json"))
const [major, minor, patch] = pkg.version.split(".").map(Number)

const version = /^\d+\.\d+\.\d+$/.test(arg)
  ? arg
  : {
      major: `${major + 1}.0.0`,
      minor: `${major}.${minor + 1}.0`,
      patch: `${major}.${minor}.${patch + 1}`,
    }[arg]

if (!version) {
  console.error("Usage: just bump <major|minor|patch|x.y.z>")
  process.exit(1)
}

const gradle = read("android/app/build.gradle")
const pbxproj = read("ios/App/App.xcodeproj/project.pbxproj")

const androidVersion = gradle.match(/versionName "(.+)"/)[1]
const androidBuild = Number(gradle.match(/versionCode (\d+)/)[1])
const iosVersion = pbxproj.match(/MARKETING_VERSION = (.+);/)[1]
const iosBuild = Number(pbxproj.match(/CURRENT_PROJECT_VERSION = (\d+);/)[1])

const nextAndroidBuild = version === androidVersion ? androidBuild : androidBuild + 1
const nextIosBuild = version === iosVersion ? iosBuild : iosBuild + 1

pkg.version = version
write("package.json", JSON.stringify(pkg, null, 2) + "\n")

write(
  "android/app/build.gradle",
  gradle
    .replace(/versionName ".+"/, `versionName "${version}"`)
    .replace(/versionCode \d+/, `versionCode ${nextAndroidBuild}`),
)

write(
  "ios/App/App.xcodeproj/project.pbxproj",
  pbxproj
    .replace(/MARKETING_VERSION = .+;/g, `MARKETING_VERSION = ${version};`)
    .replace(/CURRENT_PROJECT_VERSION = \d+;/g, `CURRENT_PROJECT_VERSION = ${nextIosBuild};`),
)

console.log(`android: ${version} (${nextAndroidBuild})`)
console.log(`ios: ${version} (${nextIosBuild})`)
