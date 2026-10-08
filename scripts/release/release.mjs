#!/usr/bin/env node
// Builds, signs and publishes a release from this machine so that no signing key leaves it
import {release} from "./lib/pipeline.mjs"
import apk from "./steps/apk.mjs"
import gitea from "./steps/gitea.mjs"
import ios from "./steps/ios.mjs"
import play from "./steps/play.mjs"
import sync from "./steps/sync.mjs"
import zapstore from "./steps/zapstore.mjs"

await release("just release", [sync, apk, play, ios, gitea, zapstore])
