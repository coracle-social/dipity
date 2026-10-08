// The one-line comment rule in AGENTS.md, which a reviewer would otherwise have
// to notice by eye.
//
// A run of two or more comment-only `//` lines is a comment that outgrew the
// thing it sits on. `///` and `//!` are documentation and are exempt: rustdoc is
// the API's text, not a note about the code under it.

import {execFileSync} from "node:child_process"
import {readFileSync} from "node:fs"

const SOURCE = /\.(rs|ts|js|svelte)$/
const VENDORED = /^(src\/lib\/components\/ui|ios|android|ref)\//

const isComment = line => line.trim().startsWith("//") && !/^\/\/[/!]/.test(line.trim())

const runsIn = file => {
  const lines = readFileSync(file, "utf8").split("\n")
  const found = []

  for (let i = 0; i < lines.length; i++) {
    if (isComment(lines[i])) {
      const start = i

      while (isComment(lines[i + 1] ?? "")) i++

      // A header runs as long as it needs to, because it describes the file it opens.
      const header = lines.slice(0, start).every(line => !line.trim() || line.startsWith("#!"))

      if (i > start && !header) {
        found.push({file, line: start + 1, length: i - start + 1})
      }
    }
  }

  return found
}

const violations = execFileSync("git", ["ls-files", "-z"], {encoding: "utf8"})
  .split("\0")
  .filter(path => SOURCE.test(path) && !VENDORED.test(path))
  .flatMap(runsIn)

for (const {file, line, length} of violations) {
  console.error(`${file}:${line}: comment runs ${length} lines`)
}

if (violations.length) {
  console.error(
    `\n${violations.length} over one line. A comment is one line (AGENTS.md); anything ` +
      `needing more is a function that wants splitting, a doc comment, or a line in docs/.`,
  )
  process.exit(1)
}
