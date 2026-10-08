// Nothing outside a Mac compiles the iOS shell — see core/README.md#the-xcode-project
// and coracle/dip#66 — so a name it gets wrong against the generated bindings is
// found by running the app or not at all. uniffi capitalizes an error enum's
// cases and lowercases every other enum's, which is the mismatch this catches.

import {readdirSync, readFileSync} from "node:fs"

const BINDINGS = "core/target/ffi/swift/dip_ffi.swift"
const SHELL = "ios/App/App"

const source = readFileSync(BINDINGS, "utf8")

// An enum is declared and closed at the left margin, and its cases sit one level in.
const enums = new Map()

let open = null

for (const line of source.split("\n")) {
  const declared = line.match(/^(?:public )?enum (\w+)/)
  const member = line.match(/^ {4}case (\w+)/)

  if (declared) enums.set((open = declared[1]), new Set())
  if (line === "}") open = null
  if (open && member) enums.get(open).add(member[1])
}

// The bindings compile into the App module, where a plugin method of the same name shadows one — `coreVersion` is both.
const globals = new Set(
  Array.from(source.matchAll(/^public func (\w+)\(/gm), match => match[1]).filter(
    name => !name.startsWith("FfiConverter") && !name.startsWith("uniffi"),
  ),
)

const spellings = new Map()

for (const [type, members] of enums) {
  for (const member of members) {
    const key = member.toLowerCase()

    spellings.set(key, (spellings.get(key) ?? new Set()).add(`${type}.${member}`))
  }
}

const nearest = (members, member) =>
  [...members].find(name => name.toLowerCase() === member.toLowerCase())

const failures = []

for (const file of readdirSync(SHELL).filter(name => name.endsWith(".swift"))) {
  readFileSync(`${SHELL}/${file}`, "utf8")
    .split("\n")
    .forEach((line, index) => {
      const where = `${SHELL}/${file}:${index + 1}`

      // A case named through its type, which is the only reference that says what it belongs to.
      for (const [, type, member] of line.matchAll(/\b([A-Z]\w*)\.(\w+)\b/g)) {
        const members = enums.get(type)
        const suggestion = members && nearest(members, member)

        if (members && !members.has(member)) {
          failures.push(
            `${where}: ${type} has no case ${member}` +
              (suggestion ? `, did you mean ${type}.${suggestion}?` : ""),
          )
        }
      }

      // A call with nothing in front of it is unqualified, and a declaration is not a call.
      for (const [, qualifier, name] of line.matchAll(/(\.|\bfunc )?(\w+)\(/g)) {
        if (globals.has(name) && !qualifier) {
          failures.push(
            `${where}: ${name}() is shadowed by anything of that name — spell it App.${name}()`,
          )
        }
      }

      // A bare pattern is held only to its spelling, because it carries no type.
      for (const [, member] of line.matchAll(/\bcase\s+\.(\w+)\b/g)) {
        const spelled = spellings.get(member.toLowerCase())

        if (spelled && ![...spelled].some(name => name.endsWith(`.${member}`))) {
          failures.push(`${where}: no case .${member}, did you mean ${[...spelled].join(" or ")}?`)
        }
      }
    })
}

for (const failure of failures) console.error(failure)

process.exit(failures.length ? 1 : 0)
