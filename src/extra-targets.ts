import { promises as fs } from "node:fs";
import path from "node:path";
import { isProtectedName, type Guard } from "./safety.js";
import { children, pathsTarget, type ScanContext, type TargetDef } from "./targets.js";
import { exists, home, isDir, listDir, run, totalSize, which } from "./utils.js";

const byVersionDesc = (a: string, b: string) =>
  path.basename(b).localeCompare(path.basename(a), undefined, { numeric: true });

/** All entries of `dir` except the `keep` newest (by version-like name). */
async function allButNewest(dir: string, keep = 1, filter: (name: string) => boolean = () => true): Promise<string[]> {
  const entries = (await listDir(dir)).filter((p) => filter(path.basename(p)));
  const dirs: string[] = [];
  for (const e of entries) if (await isDir(e)) dirs.push(e);
  return dirs.sort(byVersionDesc).slice(keep);
}

async function olderThan(p: string, days: number): Promise<boolean> {
  try {
    return (await fs.stat(p)).mtimeMs < Date.now() - days * 86400_000;
  } catch {
    return false;
  }
}

/** Subfolders Electron/Chromium apps use purely as caches (rebuilt on launch). */
const ELECTRON_CACHE_DIRS = [
  "Cache",
  "Code Cache",
  "GPUCache",
  "CachedData",
  "CachedExtensionVSIXs",
  "DawnGraphiteCache",
  "DawnWebGPUCache",
  "ShaderCache",
  "GrShaderCache",
  "Service Worker/CacheStorage",
  "Service Worker/ScriptCache",
  "Crashpad/completed",
  "logs",
];

/** Cache folders of a VS Code-family editor; the rest of its Application Support folder is never touched. */
const EDITOR_CACHE_DIRS = ["Cache", "Code Cache", "GPUCache", "CachedData", "CachedExtensionVSIXs", "DawnGraphiteCache", "DawnWebGPUCache", "logs", "Crashpad/completed"];

const guarded = (def: TargetDef, guard: Guard): TargetDef => ({ ...def, guard });

function editorCache(id: string, label: string, folders: string[], name: string, pattern: string): TargetDef {
  return guarded(
    pathsTarget({ id, label, group: "cache", defaultOn: true }, async () => {
      const out: string[] = [];
      for (const f of folders) for (const c of EDITOR_CACHE_DIRS) out.push(...(await listDir(home("Library/Application Support", f, c))));
      return out;
    }),
    { name, pattern },
  );
}

/** Versioned IDE data dirs, e.g. AndroidStudio2025.3.4 → product "AndroidStudio". */
async function oldIdeDirs(parent: string): Promise<string[]> {
  const byProduct = new Map<string, string[]>();
  for (const p of await listDir(parent)) {
    const m = path.basename(p).match(/^([A-Za-z]+?)(\d{4}\.\d+(?:\.\d+)?)$/);
    if (!m || !(await isDir(p))) continue;
    if (!byProduct.has(m[1])) byProduct.set(m[1], []);
    byProduct.get(m[1])!.push(p);
  }
  return [...byProduct.values()].flatMap((dirs) => dirs.sort(byVersionDesc).slice(1));
}

/** Flutter versions referenced by `.fvmrc` / `.fvm/fvm_config.json` in the scanned roots. */
async function fvmVersionsInUse(ctx: ScanContext): Promise<Set<string>> {
  const used = new Set<string>();
  for (const root of ctx.roots) {
    const res = await run("/usr/bin/find", [root, "-maxdepth", "6", "(", "-name", ".fvmrc", "-o", "-name", "fvm_config.json", ")", "-not", "-path", "*/node_modules/*"], 120_000);
    for (const f of res.stdout.split("\n").filter(Boolean)) {
      try {
        const j = JSON.parse(await fs.readFile(f, "utf8"));
        const v = j.flutter ?? j.flutterSdkVersion;
        if (typeof v === "string") used.add(v);
      } catch {}
    }
  }
  return used;
}

export function extraTargets(): TargetDef[] {
  return [
    // ───────────── safe, rebuilt locally ─────────────
    pathsTarget(
      { id: "dart-analysis", label: "Dart analysis server cache", group: "cache", defaultOn: true },
      async () => [home(".dartServer/.analysis-driver")],
    ),
    pathsTarget(
      { id: "dev-build-caches", label: "Compiler & linter caches (Go, Zig, ESLint, Vite, Ruff…)", group: "cache", defaultOn: true },
      // local build/lint results only: nothing in here was downloaded
      async () =>
        ["Library/Caches/go-build", ".cache/go-build", ".cache/zig", ".cache/eslint", ".cache/prettier", ".cache/vite", ".cache/webpack", ".cache/mypy", ".cache/ruff", ".android/build-cache"].map(
          (p) => home(p),
        ),
    ),
    guarded(
      pathsTarget({ id: "xcode-caches", label: "Xcode & simulator caches", group: "cache", defaultOn: true }, async () => [
        ...(await listDir(home("Library/Caches/com.apple.dt.Xcode"))),
        ...(await listDir(home("Library/Developer/CoreSimulator/Caches"))),
      ]),
      { name: "Xcode or Simulator", pattern: "/(Xcode|Simulator).app/Contents/MacOS/" },
    ),
    guarded(
      pathsTarget({ id: "jetbrains-caches", label: "JetBrains IDE caches & indexes (rebuilt on open)", group: "cache", defaultOn: false }, async () => {
        // the newest folder per IDE (older versions are `old-ide-data`): like File → Invalidate Caches
        const parent = home("Library/Caches/JetBrains");
        const old = new Set(await oldIdeDirs(parent));
        const out: string[] = [];
        for (const d of await listDir(parent)) if (!old.has(d) && (await isDir(d))) out.push(...(await listDir(d)));
        return out;
      }),
      { name: "a JetBrains IDE", pattern: "\\.app/Contents/MacOS/(idea|pycharm|webstorm|goland|clion|rustrover|rider|phpstorm|rubymine|datagrip|dataspell|aqua|fleet)( |$)" },
    ),
    editorCache("vscode-cache", "VS Code cache", ["Code", "Code - Insiders"], "VS Code", "/Visual Studio Code( - Insiders)?.app/Contents/MacOS/"),
    editorCache("cursor-cache", "Cursor cache", ["Cursor"], "Cursor", "/Cursor.app/Contents/MacOS/"),
    editorCache("windsurf-cache", "Windsurf cache", ["Windsurf"], "Windsurf", "/Windsurf.app/Contents/MacOS/"),
    pathsTarget({ id: "dev-logs", label: "Dev tool logs (npm, old iPhone device logs)", group: "cache", defaultOn: true }, async () => {
      const out = await listDir(home(".npm/_logs"));
      for (const p of await listDir(home("Library/Developer/Xcode/iOS Device Logs"))) if (await olderThan(p, 30)) out.push(p);
      return out;
    }),
    pathsTarget({ id: "xcode-archives-old", label: "Xcode archives older than 6 months ⚠ dSYMs", group: "redownload", defaultOn: false }, async () => {
      // Archives/<date>/<name>.xcarchive: each one holds the dSYMs needed to read that build's crash logs
      const out: string[] = [];
      for (const d of await listDir(home("Library/Developer/Xcode/Archives")))
        for (const a of await listDir(d)) if (a.endsWith(".xcarchive") && (await olderThan(a, 180))) out.push(a);
      return out;
    }),
    pathsTarget(
      { id: "electron-caches", label: "App caches (Slack, Discord, Notion…)", group: "cache", defaultOn: true },
      async () => {
        const out: string[] = [];
        for (const app of await listDir(home("Library/Application Support"))) {
          if (isProtectedName(path.basename(app))) continue;
          for (const c of ELECTRON_CACHE_DIRS) {
            const p = path.join(app, c);
            if (await isDir(p)) out.push(p);
          }
        }
        return out;
      },
    ),
    pathsTarget(
      { id: "old-ide-data", label: "Old Android Studio/JetBrains versions", group: "cache", defaultOn: true },
      async () => [
        ...(await oldIdeDirs(home("Library/Application Support/Google"))),
        ...(await oldIdeDirs(home("Library/Application Support/JetBrains"))),
        ...(await oldIdeDirs(home("Library/Caches/Google"))),
        ...(await oldIdeDirs(home("Library/Caches/JetBrains"))),
        ...(await oldIdeDirs(home("Library/Logs/Google"))),
      ],
    ),
    pathsTarget(
      { id: "codex-install-leftovers", label: "Codex runtime install leftovers", group: "cache", defaultOn: true },
      async () => {
        const dirs = await children(home(".cache/codex-runtimes"), (n) => !n.startsWith("codex-runtime-install-"));
        const old: string[] = [];
        for (const d of dirs) if (await olderThan(d, 1)) old.push(d);
        return old;
      },
    ),

    // ───────────── old SDK / toolchain versions (re-download if needed again) ─────────────
    pathsTarget(
      { id: "esp-old-toolchains", label: "Old ESP-IDF toolchains (keeps newest)", group: "redownload", defaultOn: false },
      async () => {
        const out: string[] = [];
        for (const tool of await listDir(home(".espressif/tools"))) out.push(...(await allButNewest(tool, 1)));
        return out;
      },
    ),
    pathsTarget(
      { id: "android-old-sdk", label: "Old Android NDKs & build-tools", group: "redownload", defaultOn: false },
      async () => [
        ...(await allButNewest(home("Library/Android/sdk/ndk"), 1)),
        ...(await allButNewest(home("Library/Android/sdk/build-tools"), 2)),
      ],
    ),
    pathsTarget(
      { id: "android-unused-images", label: "Android system images used by no AVD", group: "redownload", defaultOn: false },
      async () => {
        const used = new Set<string>();
        for (const avd of await listDir(home(".android/avd"))) {
          try {
            const cfg = await fs.readFile(path.join(avd, "config.ini"), "utf8");
            const m = cfg.match(/image\.sysdir\.1\s*=\s*system-images\/([^/\s]+)/);
            if (m) used.add(m[1]);
          } catch {}
        }
        return (await listDir(home("Library/Android/sdk/system-images"))).filter((p) => !used.has(path.basename(p)));
      },
    ),
    pathsTarget(
      { id: "fvm-unused", label: "FVM Flutter versions no project uses", group: "redownload", defaultOn: false },
      async (ctx) => {
        const used = await fvmVersionsInUse(ctx);
        return (await listDir(home("fvm/versions"))).filter((p) => !used.has(path.basename(p)));
      },
    ),
    {
      id: "rust-old-toolchains",
      label: "Pinned Rust toolchains (keeps default)",
      group: "redownload",
      defaultOn: false,
      scan: async () => {
        if (!(await which("rustup"))) return null;
        const res = await run("rustup", ["toolchain", "list"]);
        const extra = res.stdout
          .split("\n")
          .map((l) => l.trim())
          .filter((l) => l && !l.includes("(") && /^\d/.test(l));
        if (!extra.length) return null;
        const bytes = await totalSize(extra.map((t) => home(".rustup/toolchains", t)));
        return {
          bytes,
          paths: [],
          commands: extra.map((t) => ({ cmd: "rustup", args: ["toolchain", "uninstall", t] })),
          detail: extra.map((t) => t.split("-")[0]).join(", "),
        };
      },
    },
    pathsTarget(
      { id: "zed-lsp", label: "Zed downloaded language servers", group: "redownload", defaultOn: false },
      async () => [home("Library/Application Support/Zed/extensions/work")],
    ),

    // ───────────── emulator / simulator data (wipes their contents) ─────────────
    pathsTarget(
      { id: "avd-userdata", label: "Wipe Android emulator data", group: "redownload", defaultOn: false },
      async () => {
        const out: string[] = [];
        for (const avd of await listDir(home(".android/avd"))) {
          const img = path.join(avd, "userdata-qemu.img.qcow2");
          if (await exists(img)) out.push(img);
        }
        return out;
      },
      { commands: [{ cmd: "pkill", args: ["-f", "qemu-system"] }] },
    ),
    {
      id: "sim-erase",
      label: "Erase all iOS simulators' data",
      group: "redownload",
      defaultOn: false,
      scan: async () => {
        if (!(await which("xcrun"))) return null;
        const bytes = await totalSize([home("Library/Developer/CoreSimulator/Devices")]);
        if (bytes < 200 * 1024 * 1024) return null;
        return {
          bytes,
          paths: [],
          commands: [
            { cmd: "xcrun", args: ["simctl", "shutdown", "all"] },
            { cmd: "xcrun", args: ["simctl", "erase", "all"] },
          ],
        };
      },
    },
  ];
}
