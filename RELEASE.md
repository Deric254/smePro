# Release process

## How it works now: push to main = a release

Every push to `main` automatically:
1. Bumps the version (patch by default — see below for minor/major)
2. Commits that bump and tags it (`vX.Y.Z`)
3. Builds installers for **Windows, macOS (Intel + Apple Silicon), and
   Linux**, AND an **Android APK**
4. Attaches all of them to one **draft** GitHub Release

No `git tag` step needed for normal use. Control the bump size with
your commit message:

```
git commit -m "fix: correct rounding in reports"                  # patch: 0.1.0 -> 0.1.1
git commit -m "feat: add units of measure [minor]"                # minor: 0.1.0 -> 0.2.0
git commit -m "feat!: new module format, breaking change [major]" # major: 0.1.0 -> 1.0.0
git commit -m "docs: fix typo [skip ci]"                          # no release at all
```

`[skip ci]` is GitHub's own built-in convention (not custom to this
repo) — use it for anything that shouldn't trigger a build (README
edits, comments, etc).

You can still cut an exact version manually the old way if you ever
need to (a hotfix, re-releasing a specific commit):
```
git tag v1.2.3 && git push --tags
```
This skips the auto-bump and releases exactly that tag.

## One-time setup (before your first release)

1. **Generate your own updater signing key** — do not reuse any key
   that ever appeared in a shared/AI-assisted session (like this one's
   demo key, which must be treated as compromised since its private
   half touched disk somewhere you didn't fully control):
   ```
   npm run tauri signer generate -- -w ~/.tauri/sme-pro.key
   ```
   This prints a public key — put it in
   `src-tauri/tauri.conf.json` under `plugins.updater.pubkey`,
   replacing the placeholder. **Desktop auto-update will silently not
   work correctly until you do this** — it's not optional.

2. **Add GitHub repo secrets** (Settings → Secrets and variables →
   Actions):
   - `TAURI_SIGNING_PRIVATE_KEY` — contents of the `.key` file from step 1
   - `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` — the password you set when
     generating it (blank is allowed but not recommended)

3. **Updater endpoint** in `tauri.conf.json` — already pointed at
   `https://github.com/Deric254/smePro/releases/latest/download/latest.json`
   for this repo. If you fork or rename it, update this to match.

4. **Real app icons** — the icons currently in `src-tauri/icons/` are
   placeholders generated during development. Replace them:
   ```
   npm run tauri icon path/to/your/logo.png
   ```
   This generates every platform-specific format (`.ico`, `.icns`, PNGs)
   from one source image.

5. **Android signing (optional)** — without any setup, the Android APK
   is built with Android's standard debug signing, which is a
   completely normal, installable, working APK — just not eligible for
   the Play Store. For that, add these repo secrets and the workflow
   automatically switches to using them:
   - `ANDROID_KEYSTORE_BASE64` — your release keystore file, base64-encoded
     (`base64 -i release.keystore | pbcopy` on macOS, or `base64 -w0 release.keystore` on Linux)
   - `ANDROID_KEYSTORE_PASSWORD`, `ANDROID_KEY_ALIAS`, `ANDROID_KEY_PASSWORD`

   No keystore yet? `keytool -genkey -v -keystore release.keystore -alias upload -keyalg RSA -keysize 2048 -validity 10000`

## Verifying it worked

After the workflow completes, the release page should have: an
installer/bundle per desktop platform (`.dmg`, `.msi`,
`.AppImage`/`.deb`), a detached signature file for each, an Android
`.apk`, and `latest.json` at the release level (what
`UpdateChecker.tsx` checks on every desktop launch).

The release is a **draft** on purpose (`releaseDraft: true` /
`draft: true`) — review the built artifacts and write real release
notes before publishing it to users. Flip both to `false` in
`.github/workflows/release.yml` once you're comfortable
auto-publishing without a manual check.

## What was NOT verified in the environment this was built in

This workflow is written against Tauri's documented v2 conventions and
real, widely-used GitHub Actions (`tauri-apps/tauri-action`,
`android-actions/setup-android`, `softprops/action-gh-release`). Every
piece of shell logic in it (the version-bump math, the JSON/TOML
rewriting, the network-security-config patch) was tested directly in
the sandbox this was built in and produces byte-for-byte the expected
result — see the commit history / testing notes for that. What could
**not** be verified: an actual run on a live GitHub Actions runner,
since that needs a real repo with Actions minutes, which isn't
available in this sandbox. **Test the first push carefully** — watch
the Actions tab, and don't flip `draft: false` until you've seen a
release complete successfully at least once.

## Known Android caveat

The Android build genuinely reflects `MOBILE.md`'s own disclosed risk:
Android is more aggressive than desktop about suspending backgrounded
apps, which could affect the background HTTP server thread this app
relies on. Not tested on a real device — if the APK installs fine but
feels unresponsive after being backgrounded, that's the known
architectural risk to investigate first.
