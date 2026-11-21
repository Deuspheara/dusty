#!/usr/bin/env node
import * as p from "@clack/prompts";
import { Command } from "commander";
import { existsSync } from "node:fs";
import path from "node:path";
import pc from "picocolors";
import { scanProjects, type Artifact } from "./projects.js";
import { IGNORE_FILE, IgnoreList, appRunning, assertSafe } from "./safety.js";
import { GROUP_LABELS, scanTargets, type Group, type Target } from "./targets.js";
import { HOME, formatBytes, freeSpace, home, mapLimit, run, tildify, totalSize } from "./utils.js";

interface Options {
  roots: string[];
  dryRun: boolean;
  yes: boolean;
  all: boolean;
  keepRecent: number;
  minSize: number;
  json: boolean;
}

const GROUP_ORDER: Group[] = ["build", "cache", "docker", "redownload"];
const GROUP_COLOR: Record<Group, (s: string) => string> = {
  build: pc.red,
  cache: pc.yellow,
  docker: pc.cyan,
  redownload: pc.magenta,
};

// ───────────────────────── scanning ─────────────────────────

/** Turn per-folder artifacts into one selectable target per project (and per re-download kind). */
function artifactsToTargets(artifacts: Artifact[], roots: string[], keepRecentDays: number): Target[] {
  const cutoff = Date.now() - keepRecentDays * 86400_000;
  const byKey = new Map<string, { label: string; group: Group; items: Artifact[] }>();

  for (const a of artifacts) {
    if (keepRecentDays > 0 && a.mtimeMs > cutoff) continue;
    let key: string;
    let label: string;
    let group: Group;
    if (a.redownload) {
      key = `re:${a.kind}`;
      label = `${a.kind} in projects`;
      group = "redownload";
    } else {
      const root = roots.find((r) => a.path.startsWith(r + path.sep)) ?? path.dirname(a.project);
      const rel = path.relative(root, a.project).split(path.sep).slice(0, 2).join("/");
      key = `proj:${root}/${rel}`;
      // ~/Documents is implied; other roots keep their short prefix (e.g. ~/.t3/…)
      label = root === home("Documents") ? rel || "Documents" : path.join(tildify(root), rel);
      group = "build";
    }
    if (!byKey.has(key)) byKey.set(key, { label, group, items: [] });
    byKey.get(key)!.items.push(a);
  }

  return [...byKey.entries()].map(([key, v]) => ({
    id: key,
    label: v.label,
    group: v.group,
    defaultOn: v.group === "build",
    bytes: v.items.reduce((s, i) => s + i.bytes, 0),
    paths: v.items.map((i) => i.path),
    commands: [],
    detail: v.items.length === 1 ? "1 dir" : `${v.items.length} dirs`,
  }));
}

/** Drops ignored items/paths, and paths another target already covers (never counted twice). */
async function settle(targets: Target[], ignore: IgnoreList): Promise<Target[]> {
  const claimed = new Set<string>();
  const above = new Set<string>();
  const ancestors = (q: string) => {
    const out: string[] = [];
    for (let d = path.dirname(q); d !== path.dirname(d); d = path.dirname(d)) out.push(d);
    return out;
  };
  const out: Target[] = [];
  for (const t of targets) {
    if (ignore.blocksTarget(t.id)) continue;
    const paths = t.paths.filter((q) => !ignore.blocksPath(q) && !claimed.has(q) && !ancestors(q).some((a) => claimed.has(a)) && !above.has(q));
    if (paths.length !== t.paths.length) {
      if (paths.length === 0 && t.commands.length === 0) continue;
      t.paths = paths;
      t.bytes = await totalSize(paths);
    }
    for (const q of paths) {
      claimed.add(q);
      for (const a of ancestors(q)) above.add(a);
    }
    out.push(t);
  }
  return out;
}

async function scanAll(opts: Options): Promise<Target[]> {
  const s = p.spinner();
  s.start("Scanning projects for build artifacts…");
  const ignore = IgnoreList.load();
  const [artifacts, system] = await Promise.all([
    scanProjects(opts.roots, (m) => s.message(m), ignore),
    scanTargets({ roots: opts.roots }),
  ]);
  const targets = (await settle([...artifactsToTargets(artifacts, opts.roots, opts.keepRecent), ...system], ignore))
    .filter((t) => t.bytes >= opts.minSize * 1024 * 1024 || (t.bytes === 0 && t.commands.length > 0))
    .sort((a, b) => GROUP_ORDER.indexOf(a.group) - GROUP_ORDER.indexOf(b.group) || b.bytes - a.bytes);
  const total = targets.reduce((s, t) => s + t.bytes, 0);
  s.stop(`Found ${pc.bold(formatBytes(total))} across ${targets.length} items`);
  return targets;
}

// ───────────────────────── reporting ─────────────────────────

function fit(s: string, width: number): string {
  if (width < 8) return "";
  return s.length <= width ? s : s.slice(0, width - 1) + "…";
}

function bar(bytes: number, max: number, width = 20): string {
  const n = max > 0 ? Math.max(bytes > 0 ? 1 : 0, Math.round((bytes / max) * width)) : 0;
  return "█".repeat(n) + pc.dim("░".repeat(width - n));
}

function printReport(targets: Target[]) {
  const max = Math.max(...targets.map((t) => t.bytes), 1);
  for (const g of GROUP_ORDER) {
    const items = targets.filter((t) => t.group === g);
    if (items.length === 0) continue;
    const sum = items.reduce((s, t) => s + t.bytes, 0);
    const color = GROUP_COLOR[g];
    // note box padding + mark + bar + size ≈ 42 columns
    // clack wraps note boxes at ~80 columns
    const room = 40;
    const lines = items.map((t) => {
      const size = formatBytes(t.bytes).padStart(8);
      const mark = t.defaultOn ? pc.green("●") : pc.dim("○");
      const label = fit(t.label, room);
      const detail = t.detail ? fit(t.detail, room - label.length - 2) : "";
      return `${mark} ${color(bar(t.bytes, max))} ${pc.bold(size)}  ${label}${detail ? "  " + pc.dim(detail) : ""}`;
    });
    p.note(lines.join("\n"), `${color(GROUP_LABELS[g])} ${pc.dim("—")} ${pc.bold(formatBytes(sum))}`);
  }
  p.log.message(`${pc.green("●")} selected by default   ${pc.dim("○")} opt-in`);
}

function printManualHints() {
  p.note(
    [
      `${pc.bold("Old simulator runtimes")} not managed by simctl (iOS 18.4, watchOS 11.4…):`,
      `  Xcode → Settings → Components → delete the ones you don't need`,
      `${pc.bold("Docker.raw")} keeps its size after pruning on older Docker Desktop versions:`,
      `  Docker Desktop → Settings → Resources → lower the disk limit, or Troubleshoot → Clean / Purge data`,
      `${pc.bold("Android")} unused system images / NDKs: Android Studio → SDK Manager`,
      `${pc.bold("Time Machine local snapshots")}: ${pc.cyan("tmutil listlocalsnapshots /")}`,
      `${pc.bold("Old backups")} like ${pc.cyan("~/.codex.backup-*")}: delete if you don't need them`,
      `${pc.bold("Claude desktop VM")} (~/Library/Application Support/Claude/vm_bundles): re-downloaded by the app`,
    ].join("\n"),
    "Manual cleanups worth checking",
  );
}

// ───────────────────────── cleaning ─────────────────────────

async function cleanTarget(t: Target, dryRun: boolean): Promise<string[]> {
  const errors: string[] = [];
  const ignore = IgnoreList.load();
  if (ignore.broken) return [`Nothing was deleted: ${ignore.broken}.`];
  for (const c of t.commands) {
    if (dryRun) continue;
    const r = await run(c.cmd, c.args);
    // pkill returns 1 when nothing matched: not an error
    if (!r.ok && c.cmd !== "pkill") errors.push(`${c.cmd} ${c.args.join(" ")}: ${r.stderr.trim().split("\n")[0]}`);
  }
  // an app's cache is left alone while the app is open
  if (t.guard && (await appRunning(t.guard.pattern))) {
    return [...errors, `${t.guard.name} is open, so its cache was left alone. Quit it and run dusty again.`];
  }
  await mapLimit(t.paths, 4, async (target) => {
    try {
      assertSafe(target);
    } catch (e) {
      errors.push((e as Error).message);
      return;
    }
    if (ignore.blocksPath(target)) {
      errors.push(`Left ${tildify(target)} alone: it's on your ignore list (${tildify(IGNORE_FILE)}).`);
      return;
    }
    if (dryRun) return;
    // -x: never cross into a volume mounted somewhere inside the folder
    let r = await run("/bin/rm", ["-rfx", "--", target]);
    if (!r.ok && r.stderr.includes("Permission denied")) {
      // read-only caches (Go modules…): make our own files writable (never following symlinks), then retry
      await run("/bin/chmod", ["-R", "-P", "u+w", "--", target]);
      r = await run("/bin/rm", ["-rfx", "--", target]);
    }
    if (!r.ok) errors.push(`${tildify(target)}: ${r.stderr.trim().split("\n")[0]}`);
  });
  return errors;
}

async function clean(targets: Target[], opts: Options) {
  const before = await freeSpace();
  const total = targets.reduce((s, t) => s + t.bytes, 0);
  const s = p.spinner();
  s.start(opts.dryRun ? "Dry run…" : "Cleaning…");
  const errors: string[] = [];
  let done = 0;
  for (const t of targets) {
    s.message(`${pc.dim(`[${++done}/${targets.length}]`)} ${t.label} ${pc.dim(formatBytes(t.bytes))}`);
    errors.push(...(await cleanTarget(t, opts.dryRun)));
  }
  const after = await freeSpace();
  if (opts.dryRun) {
    s.stop(`Dry run: would free about ${pc.bold(pc.green(formatBytes(total)))}`);
  } else {
    const freed = Math.max(0, after - before);
    s.stop(`Freed ${pc.bold(pc.green(formatBytes(freed)))} ${pc.dim(`(estimated ${formatBytes(total)})`)}`);
  }
  if (errors.length) {
    p.log.warn(`${errors.length} item(s) could not be removed (in use or protected):\n` + errors.slice(0, 8).map((e) => pc.dim("  " + e)).join("\n"));
  }
  p.log.info(`Free space now: ${pc.bold(formatBytes(after))}`);
}

// ───────────────────────── commands ─────────────────────────

async function cmdScan(opts: Options) {
  if (opts.json) {
    const ignore = IgnoreList.load();
    const artifacts = await scanProjects(opts.roots, undefined, ignore);
    const targets = await settle([...artifactsToTargets(artifacts, opts.roots, opts.keepRecent), ...(await scanTargets({ roots: opts.roots }))], ignore);
    console.log(JSON.stringify(targets, null, 2));
    return;
  }
  p.intro(pc.bgCyan(pc.black(" dusty — scan ")));
  p.log.info(`Free space: ${pc.bold(formatBytes(await freeSpace()))}`);
  const targets = await scanAll(opts);
  printReport(targets);
  printManualHints();
  p.outro(`Run ${pc.cyan("dusty clean")} to choose what to remove.`);
}

async function cmdClean(opts: Options) {
  p.intro(pc.bgRed(pc.white(" dusty ")));
  p.log.info(`Free space: ${pc.bold(formatBytes(await freeSpace()))}`);
  const targets = await scanAll(opts);
  if (targets.length === 0) return p.outro("Nothing to clean 🎉");
  printReport(targets);

  let selected: Target[];
  if (opts.yes) {
    selected = targets.filter((t) => opts.all || t.defaultOn);
  } else {
    const options: Record<string, { value: string; label: string; hint?: string }[]> = {};
    for (const g of GROUP_ORDER) {
      const items = targets.filter((t) => t.group === g);
      if (!items.length) continue;
      options[GROUP_LABELS[g]] = items.map((t) => ({
        value: t.id,
        // never truncate here: a cut-off label makes the choice unreadable
        label: `${formatBytes(t.bytes).padStart(8)}  ${t.label}`,
        hint: t.detail,
      }));
    }
    const picked = await p.groupMultiselect({
      message: "What should be removed? (space = toggle, enter = confirm)",
      options,
      initialValues: targets.filter((t) => opts.all || t.defaultOn).map((t) => t.id),
      required: false,
    });
    if (p.isCancel(picked)) return p.cancel("Cancelled.");
    const set = new Set(picked);
    selected = targets.filter((t) => set.has(t.id));
  }

  if (selected.length === 0) return p.outro("Nothing selected.");
  const total = selected.reduce((s, t) => s + t.bytes, 0);

  if (!opts.yes && !opts.dryRun) {
    const ok = await p.confirm({
      message: `Delete ${selected.length} item(s), about ${pc.bold(formatBytes(total))}?`,
      initialValue: true,
    });
    if (p.isCancel(ok) || !ok) return p.cancel("Cancelled.");
  }

  await clean(selected, opts);
  printManualHints();
  p.outro(opts.dryRun ? "Nothing was deleted (dry run)." : "Done ✨");
}

/** Your projects, plus the folders where coding agents (t3code, Codex, Conductor…) keep worktrees. */
const DEFAULT_ROOTS = [
  "Documents",
  ".t3",
  ".codex/worktrees",
  ".claude/worktrees",
  "conductor/workspaces",
  "esp",
  "Developer",
  "Projects",
].map((d) => home(d)).filter((d) => existsSync(d));

const program = new Command()
  .name("dusty")
  .description("Reclaim disk space on macOS: build artifacts, dev caches, Xcode, Android, Docker.")
  .version("0.1.0-beta.1")
  .option("-r, --roots <dirs...>", "folders to scan for project build artifacts", DEFAULT_ROOTS)
  .option("-n, --dry-run", "show what would be deleted without deleting", false)
  .option("-y, --yes", "no prompts: clean everything selected by default", false)
  .option("-a, --all", "also pre-select opt-in items (re-download, Docker images/volumes…)", false)
  .option("-k, --keep-recent <days>", "skip project artifacts modified in the last N days", (v) => Number(v), 0)
  .option("-m, --min-size <mb>", "hide items smaller than N MB", (v) => Number(v), 1)
  .option("--json", "machine-readable output (scan only)", false);

const getOpts = (): Options => {
  const o = program.opts();
  return { ...o, roots: (o.roots as string[]).map((r) => path.resolve(r.replace(/^~/, HOME))) } as Options;
};

program.command("scan").description("analyse disk usage, delete nothing").action(() => cmdScan(getOpts()));
program.command("clean", { isDefault: true }).description("interactively pick and delete (default)").action(() => cmdClean(getOpts()));

program.parseAsync().catch((e) => {
  p.log.error(String(e?.message ?? e));
  process.exit(1);
});
