// Xcode compiles a list, not a directory, and that list is `project.pbxproj`. A
// Swift file on disk that nothing references builds nowhere, with no error — see
// core/README.md#the-xcode-project. Nothing on Linux can open the project, so these
// are the parts of it a machine can still check.

import {existsSync, readdirSync, readFileSync} from "node:fs"

const PROJECT = "ios/App/App.xcodeproj/project.pbxproj"
const SHELL = "ios/App/App"
const PACKAGES = "ios/App"

const source = readFileSync(PROJECT, "utf8")

// An object is `<uuid> /* comment */ = { ... }` on one line or many, and the comment is decoration.
const objects = new Map()

for (const match of source.matchAll(/^\t\t([0-9A-F]{24})(?: \/\*[^\n]*\*\/)? = \{(.*)\};$/gm)) {
  objects.set(match[1], match[2])
}

for (const match of source.matchAll(
  /^\t\t([0-9A-F]{24})(?: \/\*[^\n]*\*\/)? = \{\n(.*?)^\t\t\};$/gms,
)) {
  objects.set(match[1], match[2])
}

const field = (uuid, name) =>
  objects.get(uuid)?.match(new RegExp(`\\b${name} = "?(.+?)"?(?: /\\*[^\\n]*\\*/)?;`))?.[1]

const list = (uuid, name) =>
  Array.from(
    objects
      .get(uuid)
      ?.match(new RegExp(`${name} = \\(\\n(.*?)\t*\\);`, "s"))?.[1]
      ?.matchAll(/([0-9A-F]{24})/g) ?? [],
    match => match[1],
  )

const of = isa => [...objects.keys()].filter(uuid => field(uuid, "isa") === isa)

const failures = []

// A build file whose file reference went missing opens fine and drops the file silently.
for (const match of source.matchAll(/([0-9A-F]{24})/g)) {
  if (!objects.has(match[1])) failures.push(`${PROJECT}: reference to undefined object ${match[1]}`)
}

const target = of("PBXNativeTarget")[0]
const phases = list(target, "buildPhases")
const phase = phases.find(uuid => field(uuid, "isa") === "PBXSourcesBuildPhase")
const compiled = new Set(list(phase, "files").map(uuid => field(field(uuid, "fileRef"), "path")))

for (const file of readdirSync(SHELL).filter(name => name.endsWith(".swift"))) {
  if (!compiled.has(file)) failures.push(`${SHELL}/${file}: not in the App target's Sources phase`)
}

// A directory beside the project holding a Package.swift is a local package, ours or Capacitor's.
const referenced = new Set(
  of("XCLocalSwiftPackageReference").map(uuid => field(uuid, "relativePath")),
)
const linked = new Set(
  list(target, "packageProductDependencies").map(uuid => field(uuid, "productName")),
)

for (const entry of readdirSync(PACKAGES, {withFileTypes: true})) {
  if (!entry.isDirectory()) continue
  if (!readdirSync(`${PACKAGES}/${entry.name}`).includes("Package.swift")) continue

  if (!referenced.has(entry.name)) {
    failures.push(`${PACKAGES}/${entry.name}: local package is not referenced by the project`)
  } else if (!linked.has(entry.name)) {
    failures.push(`${PACKAGES}/${entry.name}: referenced but not linked into the App target`)
  }
}

// Opening the project is the whole setup only while it builds the core before it compiles.
const prebuild = field(phases[0], "shellScript")?.match(/scripts\/[\w.-]+/)?.[0]

if (field(phases[0], "isa") !== "PBXShellScriptBuildPhase") {
  failures.push(`${PROJECT}: the App target compiles before it builds the core`)
} else if (!prebuild || !existsSync(prebuild)) {
  failures.push(
    `${PROJECT}: its first build phase runs ${prebuild ?? "no script"}, which is not in the tree`,
  )
}

for (const failure of failures) {
  console.error(failure)
}

if (failures.length) {
  console.error(
    `\n${failures.length} in the Xcode project. Add files and packages in Xcode rather than by ` +
      `hand where you can — see core/README.md#the-xcode-project.`,
  )
  process.exit(1)
}
