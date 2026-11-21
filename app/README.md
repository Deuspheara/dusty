# Dusty (native app)

A native macOS app for the `dusty` CLI, written in Rust with [GPUI](https://gpui.rs).
Same engine and safety rules as the CLI (ported to Rust in `src/engine.rs`), with Dusty the cat.

```bash
./bundle.sh            # release build → target/Dusty.app
./bundle.sh universal  # Apple silicon + Intel
open target/Dusty.app
```

- **Flow:** Disk checkup → results (one tap cleans the safe picks) → optional *Choose items* → confirm → cleaning → summary.
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
- **Testing:** `DUSTY_FORCE_DRY=1` forces every clean into a practice run at the engine level
  (`open --env DUSTY_FORCE_DRY=1 target/Dusty.app`). `Dusty --scan` prints a headless scan, `Dusty --bench` measures Dusty (CPU vs GPU), `DUSTY_TRACE=1` logs
  frame rates and startup time.
- Builds without Xcode's Metal toolchain (GPUI `runtime_shaders`).
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
