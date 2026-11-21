import { execFile } from "node:child_process";
import { promises as fs } from "node:fs";
import os from "node:os";
import path from "node:path";

export const HOME = os.homedir();
export const home = (...p: string[]) => path.join(HOME, ...p);

export interface RunResult {
  ok: boolean;
  stdout: string;
  stderr: string;
}

export function run(cmd: string, args: string[], timeoutMs = 10 * 60_000): Promise<RunResult> {
  return new Promise((resolve) => {
    execFile(cmd, args, { maxBuffer: 64 * 1024 * 1024, timeout: timeoutMs }, (err, stdout, stderr) => {
      resolve({ ok: !err, stdout: String(stdout), stderr: String(stderr) });
    });
  });
}

export async function exists(p: string): Promise<boolean> {
  try {
    await fs.access(p);
    return true;
  } catch {
    return false;
  }
}

export async function isDir(p: string): Promise<boolean> {
  try {
    return (await fs.lstat(p)).isDirectory();
  } catch {
    return false;
  }
}

export async function listDir(p: string): Promise<string[]> {
  try {
    return (await fs.readdir(p)).map((n) => path.join(p, n));
  } catch {
    return [];
  }
}

export async function which(bin: string): Promise<boolean> {
  return (await run("/usr/bin/which", [bin])).ok;
}

/** Real on-disk size (KB from `du -sk`, so APFS clones/sparse files are counted correctly). */
export async function sizeOf(paths: string[]): Promise<Map<string, number>> {
  const out = new Map<string, number>();
  const chunks: string[][] = [];
  for (let i = 0; i < paths.length; i += 200) chunks.push(paths.slice(i, i + 200));
  await mapLimit(chunks, 4, async (chunk) => {
    // `-x` stays on one filesystem: never count through mounted volumes.
    const res = await run("/usr/bin/du", ["-skx", ...chunk]);
    for (const line of res.stdout.split("\n")) {
      const m = line.match(/^(\d+)\t(.+)$/);
      if (m) out.set(m[2], Number(m[1]) * 1024);
    }
  });
  return out;
}

export async function totalSize(paths: string[]): Promise<number> {
  const sizes = await sizeOf(paths);
  let total = 0;
  for (const v of sizes.values()) total += v;
  return total;
}

export async function mapLimit<T, R>(items: T[], limit: number, fn: (item: T, i: number) => Promise<R>): Promise<R[]> {
  const results: R[] = new Array(items.length);
  let next = 0;
  const workers = Array.from({ length: Math.min(limit, items.length) }, async () => {
    while (next < items.length) {
      const i = next++;
      results[i] = await fn(items[i], i);
    }
  });
  await Promise.all(workers);
  return results;
}

export function formatBytes(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  const units = ["KB", "MB", "GB", "TB"];
  let v = bytes / 1024;
  let u = 0;
  while (v >= 1024 && u < units.length - 1) {
    v /= 1024;
    u++;
  }
  return `${v.toFixed(v >= 100 ? 0 : 1)} ${units[u]}`;
}

/** Parse docker-style sizes like "6.745GB", "512MB", "0B". */
export function parseDockerSize(s: string): number {
  const m = s.trim().match(/^([\d.]+)\s*([KMGT]?B)/i);
  if (!m) return 0;
  const mult: Record<string, number> = { B: 1, KB: 1e3, MB: 1e6, GB: 1e9, TB: 1e12 };
  return Number(m[1]) * (mult[m[2].toUpperCase()] ?? 1);
}

export async function freeSpace(): Promise<number> {
  const st = await fs.statfs("/System/Volumes/Data");
  return st.bavail * st.bsize;
}

export function tildify(p: string): string {
  return p.startsWith(HOME) ? "~" + p.slice(HOME.length) : p;
}
