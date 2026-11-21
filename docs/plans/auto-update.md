# Plan: in-app updates for Dusty

Status: proposed. To be carried out in its own thread.

## Goal

Dusty checks for new releases on its own, shows a friendly "update available" prompt, and installs the update in place
and relaunches. No trip to GitHub, no Gatekeeper warnings.

## Why notarization comes first

An updater swaps the app bundle for a newly downloaded one. For macOS to open that bundle without the
"unidentified developer" block, and for the updater to trust what it downloaded, each release must be:

1. **Signed** with a *Developer ID Application* certificate, with the hardened runtime on.
2. **Notarized** by Apple (`notarytool`) and **stapled**.

That requires a paid **Apple Developer Program** membership ($99 a year).

> **Account rule:** use the maintainer's personal Apple ID and team. Never sign with an employer's account, team
> or certificate, even if it's in the build machine's keychain. Check the team ID of any identity before using it.

## Recommended approach: Sparkle 2

[Sparkle](https://sparkle-project.org) is the standard updater for macOS apps distributed outside the App Store. It
covers download, EdDSA signature checks, installing over an app in `/Applications` (with authorization when needed),
relaunching, release notes and delta updates. Writing our own would mean re-solving all of that.

Considered and rejected: a hand-rolled Rust updater (GitHub API → zip → verify → swap bundle → relaunch). It's
possible, but it duplicates Sparkle's hardest parts (atomic replace, permissions, relaunch, rollback) and still needs
notarization to avoid Gatekeeper.

## Phases

### 0. Accounts and secrets (manual, done by the owner)

- [ ] Enroll in the Apple Developer Program with a personal Apple ID (individual).
- [ ] Create a **Developer ID Application** certificate and export it as `.p12` with a password.
- [ ] Create an **App Store Connect API key** (Users and Access → Integrations → Team Keys, role *Developer*) for `notarytool`.
- [ ] Add repository secrets. `release.yml` already reads these and skips signing while they're missing:
  - `MACOS_CERT_P12`: base64 of the `.p12`
  - `MACOS_CERT_PASSWORD`
  - `MACOS_SIGN_IDENTITY`: `Developer ID Application: <Name> (<TEAMID>)`
  - `NOTARY_KEY_P8`: base64 of the `.p8`
  - `NOTARY_KEY_ID`, `NOTARY_ISSUER_ID`
- [ ] Generate a Sparkle EdDSA key pair (`generate_keys` from the Sparkle distribution) and store the private key as
  `SPARKLE_ED_PRIVATE_KEY`. Keep an offline backup: losing it means users can't update.

### 1. Signed and notarized builds

- Check that the app runs under the hardened runtime. GPUI compiles Metal shaders at runtime (`runtime_shaders`),
  which should not need JIT entitlements. Verify on a signed build and add `app/Dusty.entitlements` only if
  something breaks.
- Dusty runs `git`, `docker`, `du`, `xcrun` and `tmutil` as subprocesses. Confirm each still works when signed.
- Cut `v0.1.1` once the secrets exist. Check it with `spctl -a -vv Dusty.app` and `xcrun stapler validate Dusty-0.1.1.dmg`.

### 2. Embed Sparkle

- In `bundle.sh`, download a pinned Sparkle 2.x release (checksum verified) and copy `Sparkle.framework` into
  `Dusty.app/Contents/Frameworks/`.
- Link it from Rust. A `build.rs` emits `-framework Sparkle`, `-F <path>` and
  `-Wl,-rpath,@executable_path/../Frameworks`.
- At launch, create an `SPUStandardUpdaterController` through the `objc` crate, which is already a dependency, and keep
  it alive for the whole process.
- Add a **Check for Updates…** item to the Dusty menu that calls `checkForUpdates:`.
- Add to `Info.plist`: `SUFeedURL`, `SUPublicEDKey`, `SUEnableAutomaticChecks = YES`, `SUScheduledCheckInterval = 86400`.
- Sign inside out (Sparkle's XPC services, `Autoupdate`, `Updater.app`, the framework, then the app), each with
  `--options runtime --timestamp`. Never use `codesign --deep`.

### 3. Appcast in CI

- Feed URL: `https://deuspheara.github.io/dusty/appcast.xml`, served from `site/` by the existing Pages workflow.
- In `release.yml`, after notarization:
  1. `sign_update Dusty-X.Y.Z.zip` with `SPARKLE_ED_PRIVATE_KEY` to get the EdDSA signature and length.
  2. Add an `<item>` to `site/appcast.xml` with version, `sparkle:shortVersionString`, minimum OS, enclosure URL (the
     release asset), signature and a link to the release notes.
  3. Commit the appcast to `main` with the `[skip ci]` marker, which triggers the Pages deploy.
- Sparkle installs from the **zip** (the DMG stays for first installs). `CFBundleVersion` must keep rising, and CI
  already sets it from `github.run_number`.

### 4. Make it feel like Dusty

- Start with Sparkle's standard UI.
- Later, a custom `SPUUserDriver` can surface the update in Dusty's speech bubble ("I learned new tricks! Update?"),
  with *Later* and *Update* buttons in the app's chunky style.
- Add a setting for automatic checks, on by default.

### 5. Test plan

- Build `0.1.1` and point a local `SUFeedURL` (e.g. `python3 -m http.server`) at an appcast for a `0.1.2` build.
  Install 0.1.1 to `/Applications`, check for updates, and confirm it lands on 0.1.2 and relaunches.
- Test both install locations: `/Applications` (may ask for an admin password) and `~/Applications`.
- Tamper with the zip and confirm Sparkle refuses it (bad EdDSA signature).
- Run `spctl` and `stapler validate` on each published artifact in CI.

## Open questions for the owner

1. Is there a personal Apple Developer account, or should this wait until one exists?
2. Silent automatic install, or always ask first? (Recommended: ask first.)
3. Keep the CLI out of scope? It updates through `git pull` / npm.
