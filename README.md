# Dip

Svelte + Vite + TypeScript nostr app built on [welshman](https://github.com/coracle-social/welshman), wrapped in [Capacitor](https://capacitorjs.com) for iOS and Android.

- **App ID:** `social.coracle.dip`
- **App name:** `Dip`
- **Web build output:** `dist/` (Capacitor's `webDir`)

## Scripts

| Command | What it does |
| --- | --- |
| `npm run dev` | Vite dev server with HMR (browser only) |
| `npm run build` | Production build into `dist/` |
| `npm run check` | `svelte-check` + `tsc` |
| `npm run sync` | `vite build` then `cap sync` — copies web assets into both native projects |
| `npm run ios` | Sync, then open the Xcode project |
| `npm run android` | Sync, then open Android Studio |

## Native workflow

Native projects live in `ios/` and `android/` and are committed to the repo (the Capacitor convention). After changing web code, run `npm run sync` before building natively — the native shells load the *built* assets from `dist/`, not the dev server.

To add a plugin:

```sh
npm install @capacitor/<plugin>
npm run sync
```

### Live reload on device

Point the native shell at your dev server by adding to `capacitor.config.ts`:

```ts
server: {
  url: 'http://<your-lan-ip>:5173',
  cleartext: true,
}
```

Run `npm run dev -- --host`, `npx cap sync`, then build. Remove the `server` block before shipping.

## welshman

All packages track `0.9.x`. `@welshman/app` is the batteries-included entry point; its siblings are declared as *peer* deps, so they are listed explicitly in `package.json` rather than installed transitively.

Not installed (add if needed): `@welshman/content` (note parsing/rendering) and `@welshman/editor` (Svelte rich-text composer).

Agent skills for all 11 packages live in `.agents/skills/`, symlinked into `.claude/skills/`. Refresh them with `npx skills add coracle-social/welshman`.

## Toolchain requirements

- **iOS** — Xcode. Capacitor 8 uses Swift Package Manager, so there is no CocoaPods / `pod install` step.
- **Android** — Android Studio with the SDK for `compileSdk 36`, plus JDK 21. Gradle needs `ANDROID_HOME` set (or a `local.properties` containing `sdk.dir`).
