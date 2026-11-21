import { existsSync, lstatSync, readFileSync, realpathSync, statSync } from "node:fs";
import path from "node:path";
import { HOME, home, run, tildify } from "./utils.js";

/**
 * Folder names whose caches hold real data or belong to sensitive apps: never swept.
 * Same list as the app (app/src/engine.rs, PROTECTED_NAMES).
 */
export const PROTECTED_NAMES = [
  // music & media libraries with offline content
  "com.spotify.client", "Spotify", "com.apple.Music", "com.apple.podcasts", "com.apple.iBooksX", "com.apple.iBooks", "com.apple.BKAgentService",
  // password managers, auth, keychains
  "com.1password.*", "com.agilebits.*", "1Password*", "com.lastpass.*", "com.dashlane.*", "com.bitwarden.*", "Bitwarden", "com.keepassx.*", "org.keepassxc.*",
  "com.authy.*", "com.yubico.*", "*keychain*", "*Keychain*", "com.proton.pass*", "*ProtonPass*", "Enpass*", "in.sinew.*", "com.nordpass.*", "com.keepersecurity.*",
  // cloud sync & backup
  "com.apple.bird", "com.apple.CloudDocs*", "CloudDocs", "com.dropbox.*", "com.getdropbox.*", "*Dropbox*", "com.google.GoogleDrive", "*GoogleDrive*", "Google Drive",
  "com.microsoft.OneDrive*", "*OneDrive*", "com.box.desktop*", "com.backblaze.*", "*Backblaze*", "com.synology.*", "com.amazon.drive",
  "com.nextcloud.*", "Nextcloud", "com.owncloud.*", "*pCloud*", "mega.mac*", "MEGAsync", "com.seafile.*", "com.resilio.*", "*Syncthing*",
  // VPNs & network tools
  "*tailscale*", "io.tailscale.*", "com.wireguard.*", "*WireGuard*", "*openvpn*", "*nordvpn*", "*expressvpn*", "*protonvpn*", "*mullvad*", "*surfshark*",
  "*cloudflare*warp*", "*zerotier*", "*clash*", "*Clash*", "com.nssurge.*",
  "PanGPS", "com.paloaltonetworks.*", "*GlobalProtect*", "com.cisco.*anyconnect*", "*AnyConnect*", "com.fortinet.*", "*FortiClient*", "*windscribe*", "com.privateinternetaccess.*",
  // input methods
  "*inputmethod*", "*InputMethod*", "com.apple.inputmethod.*", "im.rime.*",
  // editors, IDEs, AI apps keep indexes and conversation state here
  "com.jetbrains.*", "JetBrains*", "com.microsoft.VSCode*", "Code", "Code - Insiders", "Cursor", "Windsurf", "com.exafunction.*", "VSCodium",
  "com.vscodium*", "com.todesktop.*", "Zed", "dev.zed.*", "com.anthropic.*", "Claude", "com.openai.codex", "Codex",
  "com.ollama.ollama", "Ollama", "com.lmstudio.lmstudio", "LM Studio", "lm-studio",
  // virtualization & containers (live disk images)
  "com.docker.docker", "Docker Desktop", "dev.orbstack.*", "OrbStack", "com.utmapp.UTM", "com.parallels.*", "com.vmware.fusion", "org.virtualbox.*",
  "rancher-desktop", "io.rancherdesktop.*", "com.github.containers.*", "Podman Desktop", "com.vmware.*",
  // audio plugins, licenses
  "com.native-instruments.*", "com.paceap.*", "com.izotope.*", "iZotope", "com.avid.*",
  // browser automation runtimes that other tools depend on
  "ms-playwright", "Cypress", "camoufox", "puppeteer",
  // system components
  "com.apple.HomeKit", "com.apple.homed", "FamilyCircle", "com.apple.containermanagerd", "com.apple.ap.adprivacyd", "GeoServices",
  "CloudKit", "com.apple.akd", "com.apple.nsurlsessiond", "com.apple.coreaudio*", "*coreaudiod*",
  // other cleaners (avoid fighting them) and Sparkle updaters mid-download
  "com.macpaw.*", "com.nektony.*", "com.daisydiskapp.*", "org.sparkle-project.*",
];

/** Case-insensitive glob with `*` wildcards (never crosses `/`: callers match one name at a time). */
export function globMatch(pattern: string, name: string): boolean {
  const re = new RegExp("^" + pattern.split("*").map((s) => s.replace(/[.+?^${}()|[\]\\]/g, "\\$&")).join(".*") + "$", "is");
  return re.test(name);
}

export const isProtectedName = (name: string) => PROTECTED_NAMES.some((p) => globMatch(p, name));

/** Folders that hold personal data: nothing inside them is ever deleted, whatever a rule says. */
const NEVER = [
  "Library/Keychains", "Library/Mail", "Library/Accounts", "Library/Calendars", "Library/Contacts", "Library/Messages", "Library/Mobile Documents",
  "Library/Application Support/MobileSync", "Library/Application Support/AddressBook", "Library/Photos", "Library/Cookies", "Library/Preferences",
  "Library/Group Containers/group.com.apple.notes", "Library/CloudStorage", "Library/Application Support/FileProvider", ".ssh", ".gnupg", ".aws", ".kube",
  ".config/dusty", "Pictures", "Movies", "Music", "Desktop",
];
const TOP = [
  "Documents", "Desktop", "Downloads", "Library", "Library/Caches", "Library/Application Support", "Library/Containers", "Library/Group Containers",
  "Library/Logs", "Library/Preferences", "Pictures", "Movies", "Music", "Public", "Applications", ".Trash", ".config", ".cache", ".local",
];

const inside = (p: string, dir: string) => p === dir || p.startsWith(dir + path.sep);

function verdict(p: string): string | null {
  if (!inside(p, HOME) || p === HOME) return "outside your home folder";
  if (TOP.some((t) => p === home(t))) return "a top-level folder";
  if (NEVER.some((n) => inside(p, home(n))) || p.toLowerCase().includes(".photoslibrary")) return "personal data";
  return null;
}

/**
 * Same rules as the app's `assert_safe`: absolute, no control characters, no `..`, inside your home
 * folder but never a top-level or personal-data folder, on the same volume as your home folder, and the
 * same verdict for the real location after resolving symlinked ancestors. Throws on refusal.
 */
export function assertSafe(target: string): void {
  const bad = (why: string) => new Error(`Refusing to delete ${tildify(target)} (${why})`);
  if (!path.isAbsolute(target)) throw bad("not an absolute path");
  // eslint-disable-next-line no-control-regex
  if (/[\u0000-\u001f\u007f]/.test(target)) throw bad("control characters in path");
  if (target.split(path.sep).includes("..")) throw bad("'..' in path");
  const why = verdict(target);
  if (why) throw bad(why);
  try {
    if (lstatSync(target).dev !== statSync(HOME).dev) throw bad("on another volume");
  } catch (e) {
    if (e instanceof Error && e.message.startsWith("Refusing")) throw e;
  }
  // a symlinked ancestor could redirect the delete somewhere else: check where it really points
  let realParent: string | null = null;
  try {
    realParent = realpathSync(path.dirname(target));
  } catch {}
  if (realParent) {
    let realHome = HOME;
    try {
      realHome = realpathSync(HOME);
    } catch {}
    const real = path.join(realParent, path.basename(target));
    if (!inside(real, realHome)) throw bad("resolves outside your home folder");
    const w = verdict(path.join(HOME, path.relative(realHome, real)));
    if (w) throw bad(w);
  }
}

// ───────────────────────── your ignore list ─────────────────────────

export const IGNORE_FILE = home(".config/dusty/ignore");

/**
 * `~/.config/dusty/ignore`, shared with the app (same format, see README):
 * path globs (`~/…` or `/…`, `*` within a name, `**` across folders), bare names, and `id:<target-id>`.
 * A file that exists but can't be read makes every delete refuse.
 */
export class IgnoreList {
  private paths: string[][] = [];
  private ids: string[] = [];
  broken: string | null = null;

  static load(): IgnoreList {
    if (!existsSync(IGNORE_FILE)) return new IgnoreList();
    try {
      return IgnoreList.parse(readFileSync(IGNORE_FILE, "utf8"));
    } catch (e) {
      const l = new IgnoreList();
      l.broken = `couldn't read ${tildify(IGNORE_FILE)} (${(e as Error).message})`;
      return l;
    }
  }

  static parse(text: string): IgnoreList {
    const l = new IgnoreList();
    for (const raw of text.split("\n")) {
      const line = raw.split("#")[0].trim();
      if (!line) continue;
      if (line.startsWith("id:")) {
        l.ids.push(line.slice(3).trim());
        continue;
      }
      let full: string;
      if (line === "~") full = HOME;
      else if (line.startsWith("~/")) full = home(line.slice(2));
      else if (line.startsWith("/")) full = line;
      else if (!line.includes("/")) {
        l.paths.push(["**", line]);
        continue;
      } else full = home(line);
      l.paths.push(parts(full));
    }
    return l;
  }

  blocksTarget(id: string): boolean {
    return this.ids.some((p) => globMatch(p, id));
  }

  /** True when deleting `p` would delete something on the list. */
  blocksPath(p: string): boolean {
    if (!this.paths.length) return false;
    const ps = parts(p);
    return this.paths.some((pat) => overlaps(pat, ps, false));
  }
}

const parts = (p: string) => p.split(path.sep).filter((s) => s && s !== ".");

/** Same matcher as the app: the pattern matches the path or an ancestor, or the path holds a match. */
function overlaps(pat: string[], p: string[], afterGlobstar: boolean): boolean {
  if (pat.length === 0) return true;
  if (pat[0] === "**") return overlaps(pat.slice(1), p, true) || (p.length > 0 && overlaps(pat, p.slice(1), true));
  if (p.length === 0) return !afterGlobstar;
  return globMatch(pat[0], p[0]) && overlaps(pat.slice(1), p.slice(1), afterGlobstar);
}

// ───────────────────────── running-app guard ─────────────────────────

/** Never clean an app's cache while it's open: name to show, `pgrep -if` regex. */
export interface Guard {
  name: string;
  pattern: string;
}

const running = new Map<string, Promise<boolean>>();
export function appRunning(pattern: string): Promise<boolean> {
  if (!running.has(pattern)) running.set(pattern, run("/usr/bin/pgrep", ["-if", pattern], 10_000).then((r) => r.ok));
  return running.get(pattern)!;
}
