# Dusty (native app)

A native macOS app, written in Rust with [GPUI](https://gpui.rs), with Dusty the cat.

## Layout

```
app/
├─ src/                      the app (GPUI): ui/ screens and tools, dusty/ the mascot, platform.rs, updater.rs
└─ crates/
   ├─ dusty-core/            the engine, no UI: everything the app and the CLI do goes through it
   │  ├─ catalog/*.toml      what can be cleaned, as data (project rules, system targets)
   │  └─ src/                scan (projects, catalog, providers) → plan → execute (clean, trash, history)
   └─ dusty-cli/             the `dusty` command: argument parsing and output only
```

- **Adding a project type or a cache** is an entry in `crates/dusty-core/catalog/*.toml`; the tests check every
  entry (valid fields, known group and provider, files found in a fake home, markers required). Only something a
  list of paths can't express needs a provider: a function in `src/providers.rs`, named from the catalog.
- **Cleaning** is `CleanPlan::new` (pure: what will happen to each item) then `execute` (re-checks every path
  right before it goes, reports `CleanEvent`s, records the history). Scans report `ScanEvent`s the same way.
- **Safety** lives in the core only: `protected.rs` (names and folders never touched), `clean.rs` (`assert_safe`,
  the ignore list, running-app guards, in that order for every path).

```bash
./bundle.sh                      # release build → target/Dusty.app
./bundle.sh universal            # Apple silicon + Intel
open target/Dusty.app
cargo test --workspace           # engine, CLI and app tests
cargo run -p dusty-cli -- scan   # the CLI from source
```

## Features

- **Flow:** Disk checkup → results (one tap cleans the safe picks) → optional *Choose items* → confirm → cleaning → summary.
  Cleaning goes through `dusty_core::execute` with `CleanPolicy::TrashUnlessCache`: caches and build output are
  deleted, logs and re-downloadables go to the Trash. The summary splits the two ("X GB is in the Trash, empty it to
  get the space back") and offers **Put back**.
- **Tools** (`src/ui/tools.rs` plus one file per tool in `src/ui/tools/`, a row of tiles under *Start checkup*; ⚙ Settings in the top band): *Uninstall apps*
  (search, real app icons, leftovers preselected by the engine, open apps say "quit it first", admin-only apps say so),
  *Big & old files* (nothing preselected, Show in Finder, Move to Trash), *What's using space* (squarified treemap of
  your home, depth 2, tap to drill in, breadcrumb back, honest "partial" when the time budget runs out), *History*
  (Put back) and *Settings* (weekly safe clean, menu bar item, practice run, ignore list). Every slow step runs on a
  background thread with a Stop; removals go through the confirm sheet and land in the Trash.
- **Put back needs the Trash:** macOS keeps `~/.Trash` private, so without Full Disk Access Dusty can't move things
  back out and says so (drag it back in Finder instead).
- **Window:** content runs edge to edge under a transparent titlebar; the top 54 px band drags the window (double-click zooms).
- **Dusty:** the character-sheet art (`assets/dusty-sheet.png`) is cut into parts and animated on the CPU every frame
  (`src/dusty/`): springs, hops, head lag, ears that bend like a warp deformer, a wagging tail, gaze, blinks, chomping.
- **Rendering:** Dusty is drawn on the GPU with Metal (`src/dusty/gpu.rs`): rig layers are textures, the
  bending ears are textured bands, expressions are anti-aliased shader shapes, 4× MSAA. ~0.1 ms CPU per frame.
  The same drawing code also runs on the CPU (`DUSTY_CPU=1`), and `--bench` checks both match pixel-for-pixel.
  Dusty runs at 60 fps while moving, 30 fps while breathing, 12 fps when the window isn't focused.
- **Splash:** purple badge + dust sweep, then an iris opens on Dusty and the home screen builds in a stagger
  (under a second, click to skip; `DUSTY_SPLASH_SLOW=10` plays it in slow motion).
- **Practice run:** toggle it on *Choose items*; nothing is deleted.
- **Keyboard:** Return presses the screen's primary button (start, clean, confirm, done), Esc closes the confirm
  sheet or goes back (stops a checkup, leaves *Choose items*; a first Esc there just hides the focus ring),
  ⌘R checks again. On *Choose items*, ↑ ↓ move a grape focus ring between items (the list scrolls along) and
  Space ticks the focused one. In the tools, Esc goes back (clears the app search first, climbs the treemap's
  breadcrumb), ↑ ↓ + Return open an app, ↑ ↓ + Space tick a big file, ⌘R looks again, and typing searches apps.
- **Row actions:** hovering an item (on *Results* or *Choose items*) shows 📂 **Show in Finder** (the folder itself,
  all of a target's folders when they sit side by side, otherwise their closest common folder) and 🙈
  **Never clean this**. That adds `id:<item>` to `~/.config/dusty/ignore` through `engine::ignore_edit` (folders and
  the file are created if needed, existing lines are kept), the row fades out, and the toast offers **Undo**
  (or ⌘Z) for 5 seconds, which takes the line back out. *Settings* lists the entries with a remove button.
- **Last checkup:** every finished checkup saves when it ran and what it found to
  `~/Library/Application Support/Dusty/last-scan.json` (`engine::summary`); the home screen says "Last checked 3 days ago · ~12.4 GB
  found". `DUSTY_LAST_SCAN=<path>` points it at another file.
- **Before → after:** the summary shows the disk bar before cleaning with the freed slice in mint, which melts
  away, and "X free → Y free" (for a practice run: what it would be).
- **Menu bar:** a 🐾 item shows the free space (re-measured every 3 minutes and after each clean) with *Open Dusty*,
  *Check up now* and *Quit Dusty*. *Dusty → Show in Menu Bar* turns it off (saved in user defaults);
  `DUSTY_MENU_BAR=0|1` forces it.
- **Updates:** [Sparkle 2](https://sparkle-project.org) does the checking, verifying and installing; Dusty draws the UI
  (`src/updater.rs` is the Sparkle user driver, the sheet and the pill live in `src/ui/update_sheet.rs`). `scripts/fetch-sparkle.sh`
  downloads the pinned framework into `vendor/`; without it the app still builds, with updates off.
  `DUSTY_UPDATE_DEMO=found` (or `checking`, `downloading`, `ready`, `uptodate`, `error`…, add `-hidden` for the pill)
  shows a state without a feed; `DUSTY_UPDATE_FEED=<url> DUSTY_UPDATE_CHECK=1` checks a test feed at launch.
- **Reduce motion:** follows the macOS setting (no splash, confetti or slides); `DUSTY_REDUCE_MOTION=1|0` forces it.
- **Testing:** `DUSTY_FORCE_DRY=1` forces every clean into a practice run at the engine level
  (`open --env DUSTY_FORCE_DRY=1 target/Dusty.app`). `DUSTY_OPEN=apps|large|usage|history|settings` opens a tool
  at launch. `dusty scan` (the CLI) prints a scan, `Dusty --bench` measures Dusty (CPU vs GPU), `DUSTY_TRACE=1` logs
  frame rates and startup time.
- **Engine extras** (`crates/dusty-core/src/`): move to Trash + clean history with put back (`trash.rs`, `history.rs`,
  `~/Library/Application Support/Dusty/history.jsonl`), app uninstaller with strict leftover matching (`uninstall.rs`),
  large & old files review (`large.rs`), disk usage tree + squarified treemap (`usage.rs`), last scan summary
  (`summary.rs`, `last-scan.json`), ignore list editing (`ignore_edit.rs`), the weekly LaunchAgent (`schedule.rs`)
  and your own catalog additions (`~/.config/dusty/catalog.toml`, `catalog.rs`).
- **Scheduled clean:** `Dusty --scheduled-clean [--policy=trash-unless-cache|trash|delete]` scans and cleans the safe
  picks headless (never re-downloadable items, leaves build folders touched in the last 7 days), records it in the
  history and exits. The LaunchAgent `~/Library/LaunchAgents/io.github.deuspheara.dusty.scheduled-clean.plist`
  runs it weekly; output goes to `~/Library/Application Support/Dusty/scheduled.log`.
- Builds without Xcode's Metal toolchain (GPUI `runtime_shaders`).
- **App icon:** *Dusty → App Icon* switches the Dock icon between purple, mint and cream (saved in user defaults).
- **Haptics:** on Force Touch trackpads, primary buttons give a firm click, toggles/checkboxes a soft tick,
  the destructive confirm a double, and each item Dusty eats a tick.

## Safety (ideas adapted from [Mole](https://github.com/tw93/Mole))

Mole is GPL-3.0. No Mole code is included here: its safety approach was reimplemented in Rust, and the
cache locations / app bundle IDs it documents were used as reference.


- **Path validation** (`assert_safe`): absolute, no control characters, no `..`, inside your home folder,
  never a top-level folder (Documents, Library, Downloads…) or personal data (Keychains, Mail, Contacts,
  Calendars, iCloud Drive, iPhone backups, Photos libraries, `.ssh`…), and symlinked ancestors are resolved
  so a link can't redirect a delete.
- **Protected names:** cache sweeps skip apps whose "cache" holds real data or state (Spotify and Apple Music
  offline media, password managers, cloud sync, VPNs, IDEs and AI apps, virtualization, audio plugins,
  system components).
- **Running-app guard:** an app's cache (browsers, Slack, Discord, Zoom, Teams, Figma, Adobe…) is skipped while
  that app is open and reported as busy.
- Everyday-Mac targets beyond dev tools: browser and chat caches, media caches, logs and crash reports,
  unfinished downloads, old Mail attachment copies; old installers and iPhone update files are opt-in.
