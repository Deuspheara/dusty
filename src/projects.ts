import { spawn } from "node:child_process";
import { promises as fs } from "node:fs";
import path from "node:path";
import { IgnoreList } from "./safety.js";
import { run, sizeOf } from "./utils.js";

/**
 * A rule says: a directory called `name` is a build artifact when its parent
 * contains one of `markers` (empty = any parent).
 * `redownload: true` means deleting it costs a network re-download (node_modules, venv...).
 */
interface Rule {
  name: string | RegExp;
  markers: string[];
  kind: string;
  redownload?: boolean;
}

const GRADLE = ["build.gradle", "build.gradle.kts", "settings.gradle", "settings.gradle.kts", "gradlew"];
const JS = ["package.json"];
const PY = ["pyproject.toml", "setup.py", "setup.cfg", "requirements.txt", "tox.ini", "noxfile.py"];
const NUXT = ["nuxt.config.ts", "nuxt.config.js", "nuxt.config.mjs"];
/** `*.ext` = any file with that extension. */
const DOTNET = ["*.csproj", "*.fsproj", "*.vbproj"];

// Same rules, same order as the app (app/src/engine.rs). First match wins.
const RULES: Rule[] = [
  // Flutter / Dart
  { name: "build", markers: ["pubspec.yaml"], kind: "Flutter build" },
  { name: ".dart_tool", markers: ["pubspec.yaml"], kind: "Dart tool cache" },
  { name: "ephemeral", markers: ["generated_plugin_registrant.cc", "generated_plugins.cmake", "CMakeLists.txt", "Podfile"], kind: "Flutter ephemeral" },
  // Gradle / Android / Kotlin
  { name: "build", markers: GRADLE, kind: "Gradle build" },
  { name: ".gradle", markers: GRADLE, kind: "Gradle project cache" },
  { name: ".cxx", markers: GRADLE, kind: "Android NDK build" },
  { name: ".externalNativeBuild", markers: GRADLE, kind: "Android NDK build" },
  { name: ".kotlin", markers: GRADLE, kind: "Kotlin cache" },
  // Rust / Maven / Scala
  { name: "target", markers: ["Cargo.toml", "pom.xml"], kind: "Rust/Maven target" },
  { name: "target", markers: ["build.sbt"], kind: "Scala target" },
  { name: ".bloop", markers: ["build.sbt"], kind: "Scala Bloop cache" },
  { name: ".metals", markers: ["build.sbt"], kind: "Scala Metals cache" },
  // Swift / Xcode
  { name: ".build", markers: ["Package.swift"], kind: "SwiftPM build" },
  { name: "DerivedData", markers: [], kind: "Xcode DerivedData" },
  // C / C++
  { name: "build", markers: ["CMakeLists.txt", "meson.build"], kind: "CMake build" },
  { name: /^cmake-build-/, markers: ["CMakeLists.txt"], kind: "CLion build" },
  { name: ".pio", markers: ["platformio.ini"], kind: "PlatformIO build" },
  // Zig
  { name: ".zig-cache", markers: ["build.zig"], kind: "Zig cache" },
  { name: "zig-cache", markers: ["build.zig"], kind: "Zig cache" },
  { name: "zig-out", markers: ["build.zig"], kind: "Zig output" },
  // .NET, Elixir, Haskell, Elm, Godot
  { name: "bin", markers: DOTNET, kind: ".NET build" },
  { name: "obj", markers: DOTNET, kind: ".NET build" },
  { name: "_build", markers: ["mix.exs"], kind: "Elixir build" },
  { name: ".elixir_ls", markers: ["mix.exs"], kind: "ElixirLS cache" },
  { name: ".stack-work", markers: ["stack.yaml"], kind: "Haskell Stack build" },
  { name: "dist-newstyle", markers: ["cabal.project", "*.cabal"], kind: "Haskell Cabal build" },
  { name: "elm-stuff", markers: ["elm.json"], kind: "Elm build" },
  { name: ".godot", markers: ["project.godot"], kind: "Godot import cache" },
  // JS / TS
  { name: ".next", markers: JS, kind: "Next.js build" },
  { name: ".nuxt", markers: JS, kind: "Nuxt build" },
  { name: ".output", markers: NUXT, kind: "Nuxt output" },
  { name: ".svelte-kit", markers: JS, kind: "SvelteKit build" },
  { name: ".astro", markers: JS, kind: "Astro cache" },
  { name: ".docusaurus", markers: JS, kind: "Docusaurus cache" },
  { name: ".turbo", markers: JS, kind: "Turbo cache" },
  { name: ".nx", markers: JS, kind: "Nx cache" },
  { name: ".swc", markers: JS, kind: "SWC cache" },
  { name: ".parcel-cache", markers: JS, kind: "Parcel cache" },
  { name: ".angular", markers: JS, kind: "Angular cache" },
  { name: ".expo", markers: JS, kind: "Expo cache" },
  { name: "dist", markers: JS, kind: "JS dist" },
  { name: "coverage", markers: JS, kind: "Coverage report" },
  { name: "htmlcov", markers: PY, kind: "Coverage report" },
  // Python
  { name: "__pycache__", markers: [], kind: "Python bytecode" },
  { name: ".pytest_cache", markers: [], kind: "Pytest cache" },
  { name: ".mypy_cache", markers: [], kind: "Mypy cache" },
  { name: ".ruff_cache", markers: [], kind: "Ruff cache" },
  // Re-download on next install (opt-in)
  { name: "node_modules", markers: JS, kind: "node_modules", redownload: true },
  { name: "Pods", markers: ["Podfile"], kind: "CocoaPods", redownload: true },
  { name: ".venv", markers: [], kind: "Python venv", redownload: true },
  { name: "venv", markers: ["requirements.txt", "pyproject.toml", "setup.py"], kind: "Python venv", redownload: true },
  { name: ".tox", markers: PY, kind: "tox envs", redownload: true },
  { name: ".nox", markers: PY, kind: "nox envs", redownload: true },
  { name: ".pixi", markers: ["pixi.toml"], kind: "Pixi envs", redownload: true },
  { name: "deps", markers: ["mix.exs"], kind: "Elixir deps", redownload: true },
  { name: "vendor", markers: ["composer.json"], kind: "Composer vendor", redownload: true },
  { name: ".terraform", markers: [".terraform.lock.hcl"], kind: "Terraform providers", redownload: true },
  { name: ".terragrunt-cache", markers: ["terragrunt.hcl"], kind: "Terragrunt cache", redownload: true },
];

const NEVER_DESCEND = new Set([".git", ".hg", ".svn", "Library", ".Trash"]);

export interface Artifact {
  path: string;
  project: string;
  kind: string;
  redownload: boolean;
  bytes: number;
  mtimeMs: number;
}

function matchRule(dirName: string, parentEntries: Set<string>): Rule | undefined {
  for (const r of RULES) {
    const nameOk = typeof r.name === "string" ? r.name === dirName : r.name.test(dirName);
    if (!nameOk) continue;
    if (r.markers.length === 0 || r.markers.some((m) => hasMarker(m, parentEntries))) return r;
  }
  return undefined;
}

function hasMarker(marker: string, entries: Set<string>): boolean {
  if (!marker.startsWith("*")) return entries.has(marker);
  const ext = marker.slice(1);
  for (const n of entries) if (n.length > ext.length && n.endsWith(ext)) return true;
  return false;
}

/** Never follows symlinks and never leaves the volume the root lives on (`dev`). */
async function walk(dir: string, found: { path: string; rule: Rule }[], depth: number, dev: number): Promise<void> {
  if (depth > 12) return;
  let entries: import("node:fs").Dirent[];
  try {
    entries = await fs.readdir(dir, { withFileTypes: true });
  } catch {
    return;
  }
  const names = new Set(entries.map((e) => e.name));
  const subdirs: string[] = [];
  for (const e of entries) {
    if (!e.isDirectory() || e.isSymbolicLink()) continue;
    if (NEVER_DESCEND.has(e.name)) continue;
    const full = path.join(dir, e.name);
    try {
      if ((await fs.lstat(full)).dev !== dev) continue; // a mount point inside a project
    } catch {
      continue;
    }
    const rule = matchRule(e.name, names);
    if (rule) found.push({ path: full, rule });
    else if (e.name !== "node_modules") subdirs.push(full);
  }
  await Promise.all(subdirs.map((d) => walk(d, found, depth + 1, dev)));
}

async function gitRoot(dir: string): Promise<string | null> {
  const res = await run("git", ["-C", dir, "rev-parse", "--show-toplevel"]);
  return res.ok ? res.stdout.trim() : null;
}

/**
 * Safety net: inside a git repo, only keep candidates that git ignores.
 * A tracked `build/` folder is source code, not an artifact.
 */
async function filterGitIgnored(candidates: string[]): Promise<Set<string>> {
  const keep = new Set<string>();
  const byRepo = new Map<string, string[]>();
  const rootCache = new Map<string, string | null>();
  for (const c of candidates) {
    const parent = path.dirname(c);
    if (!rootCache.has(parent)) rootCache.set(parent, await gitRoot(parent));
    const root = rootCache.get(parent)!;
    if (!root) {
      keep.add(c); // not a git repo: rely on the marker rules
      continue;
    }
    if (!byRepo.has(root)) byRepo.set(root, []);
    byRepo.get(root)!.push(c);
  }
  for (const [root, paths] of byRepo) {
    const res = await new Promise<string>((resolve) => {
      const p = spawn("git", ["-C", root, "check-ignore", "--stdin"], { stdio: ["pipe", "pipe", "ignore"] });
      let out = "";
      p.stdout.on("data", (d: Buffer) => (out += d));
      p.on("close", () => resolve(out));
      p.stdin.end(paths.map((x) => x + "/").join("\n") + "\n");
    });
    for (const line of res.split("\n")) {
      const p = line.trim().replace(/\/$/, "");
      if (p) keep.add(path.isAbsolute(p) ? p : path.join(root, p));
    }
  }
  return keep;
}

export async function scanProjects(roots: string[], onProgress?: (msg: string) => void, ignore = IgnoreList.load()): Promise<Artifact[]> {
  let found: { path: string; rule: Rule }[] = [];
  for (const r of roots) {
    onProgress?.(`Walking ${r}`);
    try {
      await walk(r, found, 0, (await fs.stat(r)).dev);
    } catch {}
  }
  found = found.filter((f) => !ignore.blocksPath(f.path));
  // Drop candidates nested inside another candidate (e.g. build/ inside .dart_tool/)
  found.sort((a, b) => a.path.length - b.path.length);
  const kept: typeof found = [];
  for (const f of found) {
    if (!kept.some((k) => f.path.startsWith(k.path + path.sep))) kept.push(f);
  }

  onProgress?.(`Checking .gitignore for ${kept.length} folders`);
  const ignored = await filterGitIgnored(kept.map((k) => k.path));
  const safe = kept.filter((k) => ignored.has(k.path));

  onProgress?.(`Measuring ${safe.length} folders`);
  const sizes = await sizeOf(safe.map((s) => s.path));
  const artifacts: Artifact[] = [];
  for (const s of safe) {
    // a project counts as touched when its artifact or the project folder itself changed
    let mtimeMs = 0;
    for (const p of [s.path, path.dirname(s.path)]) {
      try {
        mtimeMs = Math.max(mtimeMs, (await fs.lstat(p)).mtimeMs);
      } catch {}
    }
    artifacts.push({
      path: s.path,
      project: path.dirname(s.path),
      kind: s.rule.kind,
      redownload: !!s.rule.redownload,
      bytes: sizes.get(s.path) ?? 0,
      mtimeMs,
    });
  }
  return artifacts.filter((a) => a.bytes > 0).sort((a, b) => b.bytes - a.bytes);
}
