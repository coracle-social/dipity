#!/usr/bin/env node
// Store screenshots of every screen of the staged street, at each size the App Store and Google Play take.
//
// Serves the view over the simulated core at `?scene` and drives it in the
// installed Chrome. `src/lib/dev/scene.ts` is the street.
//
//   node scripts/screenshots.mjs [--dark] [--store app-store|play] [screen...]

import {mkdirSync} from "node:fs"
import {join} from "node:path"
import {parseArgs} from "node:util"
import puppeteer from "puppeteer-core"
import {createServer} from "vite"

const root = join(import.meta.dirname, "..")

const chrome = process.env.CHROME ?? "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome"

/** Each display the store asks for, as CSS pixels and the scale that makes them its pixel size. */
const displays = [
  {store: "app-store", name: "iphone-6.1", width: 393, height: 852, scale: 3},
  // An iPad drawn at 768 points rather than its own 1032, which is the column the view is laid out for.
  {store: "app-store", name: "ipad-13", width: 768, height: 1024, scale: 2.6875},
  // Play's three are drawn at 9:16, the only portrait shape it takes, rather than at a device's own.
  {store: "play", name: "play-phone", width: 360, height: 640, scale: 3},
  {store: "play", name: "play-tablet-7", width: 576, height: 1024, scale: 2},
  {store: "play", name: "play-tablet-10", width: 720, height: 1280, scale: 2.25},
]

const pause = ms => new Promise(resolve => setTimeout(resolve, ms))

/** Go somewhere through the view's own navigation, which is the same module instance the app runs. */
const go = (page, place) =>
  page.evaluate(async to => (await import("/src/lib/data/nav.ts")).go(to), place)

/** The id of the staged post whose text includes `words`. */
const postSaying = (page, words) =>
  page.evaluate(
    words =>
      window.dip.store.list({}).find(({event}) => event.kind !== 7 && event.content.includes(words))
        ?.event.id,
    words,
  )

const click = (page, selector) => page.locator(selector).click()

/** Every screen, in the order the listing shows them, and how to reach each from the board. */
const screens = [
  {name: "01-board"},
  {
    name: "02-post",
    reach: async page => go(page, {at: "item", id: await postSaying(page, "plum tree")}),
  },
  {
    name: "03-picture",
    reach: async page => go(page, {at: "item", id: await postSaying(page, "First proper harvest")}),
  },
  {
    name: "04-compose",
    reach: async page => {
      await click(page, '[aria-label="Write something"]')
      await page
        .locator('textarea[placeholder="Type it out"]')
        .fill(
          "Spare tomatoes on the wall outside number 12 🍅 Help yourselves before the squirrels do.",
        )
    },
  },
  {name: "05-bookmarks", reach: page => click(page, "nav ::-p-text(Bookmarks)")},
  {name: "06-people", reach: page => click(page, "nav ::-p-text(People)")},
  {
    name: "07-contact",
    reach: async page =>
      go(page, {
        at: "contact",
        pubkey: await page.evaluate(
          async () => (await import("/src/lib/dev/scene.ts")).cast[0].pubkey,
        ),
      }),
  },
  {name: "08-settings", reach: page => click(page, "nav ::-p-text(Settings)")},
]

/** Wait for the screen to settle: pictures decoded, transitions done. */
const settle = async page => {
  await page.waitForNetworkIdle({idleTime: 300})
  await page.evaluate(() =>
    Promise.all([...document.images].map(image => image.decode().catch(() => undefined))),
  )
  await pause(700)
}

const {values: options, positionals: chosen} = parseArgs({
  options: {dark: {type: "boolean", default: false}, store: {type: "string"}},
  allowPositionals: true,
})

const selected = chosen.length
  ? screens.filter(({name}) => chosen.some(c => name.includes(c)))
  : screens
const server = await createServer({root, logLevel: "error", server: {port: 0}})

await server.listen()

const url = `${server.resolvedUrls.local[0]}?scene`
const browser = await puppeteer.launch({executablePath: chrome, headless: true})

try {
  for (const display of displays.filter(({store}) => !options.store || store === options.store)) {
    const folder = join(root, "screenshots", `${display.name}${options.dark ? "-dark" : ""}`)

    mkdirSync(folder, {recursive: true})

    for (const screen of selected) {
      const page = await browser.newPage()

      page.on("pageerror", error => console.error(`  ${screen.name}: ${error.message}`))

      await page.emulateMediaFeatures([
        {name: "prefers-color-scheme", value: options.dark ? "dark" : "light"},
      ])
      await page.setViewport({
        width: display.width,
        height: display.height,
        deviceScaleFactor: display.scale,
        isMobile: true,
        hasTouch: true,
      })
      await page.goto(url, {waitUntil: "networkidle0"})
      await page.waitForFunction(() => window.dip?.started)
      await settle(page)
      await screen.reach?.(page)
      await settle(page)

      const path = join(folder, `${screen.name}.png`)

      await page.screenshot({path})
      await page.close()
      console.log(path.slice(root.length + 1))
    }
  }
} finally {
  await browser.close()
  await server.close()
}
