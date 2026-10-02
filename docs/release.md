# Release

A release is built, signed and published from one Mac by `just release`, so no signing key or store credential leaves it. The pipeline is `scripts/release/`, and its steps run in order.

| Step | Does |
| --- | --- |
| `sync` | `just sync`: the core, the bindings, the web bundle, and both native projects |
| `apk` | Builds the APK, signed with the distribution key |
| `play` | Builds the AAB with the upload key and makes it a draft release on Google Play |
| `ios` | Archives the app, uploads it to App Store Connect, and attaches it to the version with its release notes |
| `gitea` | Publishes the gitea release for the tag, with the APK attached |
| `zapstore` | Publishes the APK to zapstore |

Obtainium reads the gitea release, so it needs no step of its own. `.gitea/workflows/mirror.yml` pushes `master` and the tags to [GitHub](https://github.com/coracle-social/dipity) and copies the latest published release there, for Obtainium users who add the app by its GitHub url. It needs a `GH_MIRROR_TOKEN` secret on the gitea repo. Play and the App Store finish in their consoles: the run ends by listing the review and rollout left to do by hand.

## Cutting one

1. Add a `# x.y.z` section at the top of `CHANGELOG.md`. Its text is the release notes everywhere, and Play takes the first 500 characters.
2. `just bump x.y.z`, which sets the version in `package.json`, Android and iOS, and advances each build number.
3. Commit, then `git tag x.y.z && git push origin master x.y.z`.
4. `just release --check` lists every step and anything a step is missing, and runs nothing.
5. `just release`.

A step that fails names the command that picks up from it, such as `just release ios gitea zapstore`. Play and App Store Connect never accept a build number twice, so their steps reuse a build already uploaded rather than building again.

## Credentials

Everything goes in `.env.local` at the repo root, which is gitignored. `just release --check` prints how to obtain each value a step is missing.

| Variables | For |
| --- | --- |
| `ANDROID_KEYSTORE_PATH`, `ANDROID_KEYSTORE_PASSWORD`, `ANDROID_KEYSTORE_ALIAS`, `ANDROID_KEYSTORE_ALIAS_PASSWORD` | The distribution key the APK is signed with |
| `PLAY_KEYSTORE_PATH`, `PLAY_KEYSTORE_PASSWORD`, `PLAY_KEYSTORE_ALIAS`, `PLAY_KEYSTORE_ALIAS_PASSWORD`, `PLAY_SERVICE_ACCOUNT` | The Play upload key, and the service account that uploads |
| `ASC_KEY_ID`, `ASC_ISSUER_ID`, `ASC_KEY_PATH`, `APPLE_TEAM_ID` | The App Store Connect API key, and the team the archive is signed for |
| `GITEA_TOKEN` | Publishing the gitea release |
| `SIGN_WITH` | The nostr key `zsp` signs the zapstore release with |

**The distribution key can never change.** Android refuses an update signed with a different key, so every install from gitea, Obtainium or zapstore is tied to it. Back it up somewhere other than this Mac.
