import {root} from "../lib/context.mjs"
import {run} from "../lib/shell.mjs"

export default {
  name: "sync",
  title: "Build the core, the bindings and the web bundle, and sync both native projects",
  run: () => run("just", ["sync"], {cwd: root}),
}
