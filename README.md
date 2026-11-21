<div align="center">

<img src="assets/brand/dusty-icon.png" width="128" height="128" alt="Dusty app icon: Dusty the cat peeking over the edge">

# Dusty

**Your Mac is full of crumbs. Dusty eats them.**

A small macOS app that sniffs out build folders, dev caches and app junk,<br>
then clears them without touching anything you'd have to download again.

[![CI](https://github.com/Deuspheara/dusty/actions/workflows/ci.yml/badge.svg)](https://github.com/Deuspheara/dusty/actions/workflows/ci.yml)
[![Release](https://img.shields.io/github/v/release/Deuspheara/dusty?color=7C5CFF&label=release)](https://github.com/Deuspheara/dusty/releases/latest)
[![License: MIT](https://img.shields.io/badge/license-MIT-22C993)](LICENSE)
![macOS 13+](https://img.shields.io/badge/macOS-13%2B-2B2340?logo=apple)

[**Download**](https://github.com/Deuspheara/dusty/releases/latest) ·
[**Website**](https://deuspheara.github.io/dusty/) ·
[What it cleans](#what-dusty-cleans) ·
[Safety](#house-rules) ·
[CLI](#the-cli)

<br>

<table>
  <tr>
    <td><img src="site/img/screen-home.png" width="250" alt="Home: disk checkup with free space"></td>
    <td><img src="site/img/screen-checkup.png" width="250" alt="Checkup in progress with the latest finds"></td>
    <td><img src="site/img/screen-results.png" width="250" alt="Results grouped by kind, safe picks selected"></td>
  </tr>
  <tr>
    <td align="center"><b>Sniff</b><br><sub>a one-tap disk checkup</sub></td>
    <td align="center"><b>Find</b><br><sub>leftovers add up live</sub></td>
    <td align="center"><b>Munch</b><br><sub>safe picks already selected</sub></td>
  </tr>
</table>

</div>

## Install

1. Grab `Dusty-x.y.z.dmg` from the [latest release](https://github.com/Deuspheara/dusty/releases/latest).
2. Drag **Dusty** to Applications.
3. Releases aren't notarized yet, so the first time, right-click Dusty → **Open** (or run
   `xattr -dr com.apple.quarantine /Applications/Dusty.app`).

Runs on Apple silicon and Intel, macOS 13 Ventura or later.

## What Dusty cleans

| | Kind | Picked by default | Examples |
|---|---|---|---|
| 🧱 | **Build folders** | yes | Flutter `build/` `.dart_tool/`, Gradle `build/` `.gradle/` `.cxx/`, SwiftPM `.build/`, Rust / Maven / sbt `target/`, CMake, Zig, .NET `bin/` `obj/`, Elixir `_build/`, Haskell `.stack-work/` `dist-newstyle/`, Elm, Godot, `.next` `.nuxt` `.svelte-kit` `.astro` `.turbo` `.nx` `dist` `coverage`, `__pycache__` and Python tool caches |
| 🗄️ | **Dev caches** | yes | Xcode DerivedData, DeviceSupport and caches, unavailable simulators, Gradle transforms, Dart analysis cache, Go / Zig / ESLint / Vite / Ruff caches, emulator snapshots, old Android Studio / JetBrains data, Homebrew. Current JetBrains indexes are opt-in (a reindex is slow) |
| 🌐 | **App caches** | yes | Browser, chat, media, VS Code / Cursor / Windsurf caches (skipped while the app is open) |
| 🧾 | **Junk & logs** | yes | Logs and crash reports, npm logs, old iPhone device logs, unfinished downloads, old Mail attachment copies. Xcode archives older than 6 months are opt-in (they hold the dSYMs for those builds' crash logs) |
| 🐳 | **Docker** | build cache, stopped containers | Images and volumes are opt-in |
| 📦 | **Re-downloadable** | no | `node_modules`, `.venv`, Pods, tox / nox / Pixi envs, Elixir `deps`, Composer `vendor`, Terraform providers, Gradle `modules-2`, npm / pnpm / Yarn / Bun / Deno / pip / uv / Poetry / Maven / Cargo / Go / Composer / NuGet caches, SwiftPM / CocoaPods / Carthage caches, Playwright / Puppeteer browsers, old simulator runtimes, old NDKs, unused FVM Flutter versions, pinned Rust toolchains |

Projects are looked for in `~/Documents`, `~/Developer`, `~/Projects` and the worktree folders that coding agents
use (`~/.codex/worktrees`, `~/.claude/worktrees`, `~/conductor/workspaces`…).

## House rules

A cleaner you can't trust is worse than a full disk. Before anything is deleted:

- **Never your code.** A project folder is cleared only when its parent has a matching marker (`pubspec.yaml`,
  `build.gradle`, `Cargo.toml`…) **and**, inside a git repo, `git check-ignore` says it's ignored. A tracked `build/`
  folder holding real source is never touched.
- **Home only.** Paths must be absolute, inside your home folder, free of `..` and control characters. Symlinked
  ancestors are resolved so a link can't redirect a delete. Walks and deletes never follow symlinks and never cross
  into another volume (`rm -x`), even one mounted inside a project.
- **Hands off personal data.** Never a top-level folder (Documents, Library, Downloads…), Keychains, Mail, Contacts,
  Calendars, iCloud Drive, Dropbox / Google Drive / OneDrive folders (`~/Library/CloudStorage`), iPhone backups,
  Photos libraries or `.ssh`.
- **Not on this Mac, not touched.** iCloud placeholders (files kept only in the cloud) are never opened, which would
  download them, and never deleted, which would delete the cloud copy and free nothing.
- **Some caches aren't caches.** Offline music, password managers, cloud sync, VPNs, IDEs, virtualization and audio
  plugins keep real state in their cache folders, so they're skipped.
- **Downloads stay opt-in.** Package-manager and browser-runtime caches are never swept up as "app caches": each has
  its own item, off until you tick it.
- **Counted once.** Each folder belongs to one item, and a hard-linked file is counted once, so the total is what you
  actually get back.
- **Busy apps wait.** An app's cache is skipped while that app is running, and Dusty tells you to quit it first.
- **Nothing goes until you say so.** Every clean ends on a confirm screen, and *Practice run* shows the whole thing
  without deleting a byte.
- **Your ignore list wins.** List anything Dusty must never touch in `~/.config/dusty/ignore`. The app and the CLI
  both read it. If the file exists but can't be read, nothing is deleted.

```bash
# ~/.config/dusty/ignore
~/Documents/big-game/build      # this folder, what's inside it, and any folder that contains it
~/Developer/*/fixtures          # * matches one folder name, ** any number of folders
Secret Stuff                    # a bare name: any file or folder called that, anywhere
id:docker-images                # hide a whole item (ids are in `dusty scan --json`)
```

The safety approach was inspired by [Mole](https://github.com/tw93/Mole) and reimplemented from scratch in Rust; no
Mole code is included.

## The CLI

Same engine, same rules, in a terminal. Needs Node 20+.

```bash
npm install && npm run build && npm link   # installs the `dusty` command
dusty scan                 # report only
dusty                      # interactive: pick, confirm, delete
dusty --dry-run --yes      # what the default selection would remove
dusty --yes                # non-interactive, default selection (good for a cron)
dusty -k 3                 # keep artifacts of projects touched in the last 3 days
dusty -r ~/Code ~/Work     # scan these roots instead
```

## Build from source

```bash
cd app
./bundle.sh              # release build → app/target/Dusty.app
./bundle.sh universal    # Apple silicon + Intel
open target/Dusty.app
```

The app is Rust on [GPUI](https://gpui.rs). Dusty is a rigged character drawn on the GPU with Metal: springs, hops,
bendy ears, a wagging tail, blinks and chomps. See [app/README.md](app/README.md) for internals and debug switches,
and [DESIGN.md](DESIGN.md) for the visual language.

## Releasing

```bash
scripts/release.sh 0.2.0
```

That bumps the version in `app/Cargo.toml`, `package.json` and the CLI, commits, tags `v0.2.0` and pushes. The
[Release workflow](.github/workflows/release.yml) builds a universal `Dusty.app`, packs a DMG and a zip, and publishes
them with generated notes. Signing and notarization switch on by themselves once the `MACOS_*` and `NOTARY_*`
repository secrets exist (see the workflow header).

Every push and pull request runs [gitleaks](https://github.com/gitleaks/gitleaks), the Rust tests and the CLI build.
Run the same secret scan locally before you commit:

```bash
brew install gitleaks pre-commit && pre-commit install
```

## License

[MIT](LICENSE)
