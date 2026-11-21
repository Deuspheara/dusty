//! App uninstaller: installed apps, the files they leave in ~/Library, and moving both to the Trash.
//!
//! Matching is strict, because a false positive is another app's data:
//! - by bundle id: a folder or file named exactly `<bundle id>` (plus its usual extension, such as
//!   `.plist` or `.savedState`), or `<bundle id>.<more>` when the id has at least three parts
//!   (`com.vendor.app.helper`), never a bare vendor prefix (`com.vendor`);
//! - Group Containers only as `group.<bundle id>` or `<TEAMID>.<bundle id>`;
//! - by name: only an exactly-named folder in Application Support, Caches or Logs, never a generic or
//!   vendor name (Google, Microsoft, Adobe…);
//! - never Apple apps (`com.apple.*`), never anything in /System, never Dusty itself, never an app
//!   that is running. Leftovers whose names are on the protected list (password managers, cloud sync,
//!   IDEs…) are found but not preselected.
//!
//! Apps in /Applications that you can't write to (installed by an installer package or the App Store
//! for all users) need an administrator: that is reported, never escalated.

use super::history::{HistoryEntry, HistoryItem, HistoryPath, HistorySource, PathAction};
use super::{trash_checked, CleanReport};
use super::large::spotlight_last_used;
use super::trash::{move_to_trash, Removal};
use super::{assert_safe_allowing, force_dry, home, ignore_refusal, is_protected_name, list_dir, name_of, read_plist_json, run_s, size_of, tildify, IgnoreList, UNINSTALL_ALLOW};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Mutex;
use std::time::SystemTime;

/// Dusty's own bundle id: never offered for uninstall.
pub const DUSTY_BUNDLE_ID: &str = "io.github.deuspheara.dusty";

#[derive(Clone, Debug)]
pub struct InstalledApp {
    /// Display name (CFBundleDisplayName, CFBundleName, or the file name).
    pub name: String,
    pub path: PathBuf,
    pub bundle_id: Option<String>,
    pub version: Option<String>,
    /// Size on disk (0 when not measured).
    pub bytes: u64,
    /// Spotlight's "last opened", when it knows.
    pub last_used: Option<SystemTime>,
    /// Running right now (checked when listed; checked again before uninstalling).
    pub running: bool,
    /// You can move it to the Trash without an administrator password.
    pub writable: bool,
    /// Installed from the App Store.
    pub app_store: bool,
}

impl InstalledApp {
    /// Why Dusty won't uninstall it right now, if anything.
    pub fn blocked(&self) -> Option<String> {
        if self.running {
            return Some(format!("{} is open. Quit it first.", self.name));
        }
        if !self.writable {
            return Some(format!("{} was installed for all users, so removing it needs an administrator. Drag it to the Trash in Finder.", self.name));
        }
        None
    }
}

#[derive(Clone, Debug)]
pub struct AppListOptions {
    /// Measure each app's size (the slow part: a big app has many thousands of files).
    pub measure: bool,
    /// Ask Spotlight when each app was last opened (one `mdls` per app).
    pub last_used: bool,
    /// Folders to list (default: /Applications, /Applications/Utilities, ~/Applications).
    pub dirs: Vec<PathBuf>,
}

impl Default for AppListOptions {
    fn default() -> Self {
        AppListOptions { measure: true, last_used: true, dirs: default_app_dirs() }
    }
}

pub fn default_app_dirs() -> Vec<PathBuf> {
    vec![PathBuf::from("/Applications"), PathBuf::from("/Applications/Utilities"), home().join("Applications")]
}

/// Live progress for the UI.
#[derive(Default, Clone, Debug)]
pub struct AppListProgress {
    pub message: String,
    /// Apps found so far, before sizes are known.
    pub found: usize,
    pub done: bool,
}

// ───────────────────────── listing ─────────────────────────

/// Info.plist keys Dusty reads.
#[derive(Default, Clone, Debug, PartialEq)]
pub(crate) struct BundleInfo {
    pub id: Option<String>,
    pub name: Option<String>,
    pub display_name: Option<String>,
    pub version: Option<String>,
}

const INFO_KEYS: [&str; 4] = ["CFBundleIdentifier", "CFBundleName", "CFBundleDisplayName", "CFBundleShortVersionString"];

pub(crate) fn read_bundle_info(app: &Path) -> Option<BundleInfo> {
    let plist = app.join("Contents/Info.plist");
    if !plist.is_file() {
        return None;
    }
    let ps = plist.to_string_lossy().into_owned();
    let get = |v: &serde_json::Value, k: &str| v.get(k).and_then(|x| x.as_str()).map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
    if let Some(v) = read_plist_json(&plist) {
        return Some(BundleInfo {
            id: get(&v, INFO_KEYS[0]),
            name: get(&v, INFO_KEYS[1]),
            display_name: get(&v, INFO_KEYS[2]),
            version: get(&v, INFO_KEYS[3]),
        });
    }
    // plists holding dates or data can't become JSON: read the keys one by one
    let one = |k: &str| {
        let r = run_s("/usr/bin/plutil", &["-extract", k, "raw", "-o", "-", &ps], 10);
        r.ok.then(|| r.stdout.trim().to_string()).filter(|s| !s.is_empty())
    };
    Some(BundleInfo { id: one(INFO_KEYS[0]), name: one(INFO_KEYS[1]), display_name: one(INFO_KEYS[2]), version: one(INFO_KEYS[3]) })
}

/// Bundle ids Dusty never uninstalls.
pub fn is_apple_or_dusty(bundle_id: &str) -> bool {
    let b = bundle_id.to_lowercase();
    b.starts_with("com.apple.") || b == "com.apple" || b == DUSTY_BUNDLE_ID
}

/// Executable paths of running processes (`ps`), checked once per listing.
fn running_paths() -> Vec<String> {
    run_s("/bin/ps", &["-axww", "-o", "comm="], 10).stdout.lines().map(|l| l.trim().to_string()).filter(|l| !l.is_empty()).collect()
}

fn is_running(app: &Path, procs: &[String]) -> bool {
    let prefix = format!("{}/", app.to_string_lossy());
    procs.iter().any(|p| p.starts_with(&prefix))
}

fn writable(p: &Path) -> bool {
    use std::os::unix::ffi::OsStrExt;
    let Ok(c) = std::ffi::CString::new(p.as_os_str().as_bytes()) else { return false };
    unsafe { libc::access(c.as_ptr(), libc::W_OK) == 0 }
}

/// Moving a folder needs write access to it (its `..` entry changes) and to the folder it's in.
fn can_move(app: &Path) -> bool {
    writable(app) && app.parent().is_some_and(writable)
}

/// `.app` bundles in `dir` and in its plain subfolders ("/Applications/Adobe Photoshop 2025/…").
/// Symlinks (Safari in /Applications points into the system) are skipped.
fn app_bundles(dir: &Path, depth: usize) -> Vec<PathBuf> {
    let mut out = vec![];
    for p in list_dir(dir) {
        let Ok(m) = std::fs::symlink_metadata(&p) else { continue };
        if !m.is_dir() {
            continue;
        }
        let n = name_of(&p);
        if n.starts_with('.') {
            continue;
        }
        if n.to_lowercase().ends_with(".app") {
            out.push(p);
        } else if depth > 0 && n != "Utilities" {
            out.extend(app_bundles(&p, depth - 1));
        }
    }
    out
}

/// Installed apps that Dusty can offer to uninstall (Apple apps and Dusty itself are left out),
/// sorted by name. Blocking and parallel: call it from a background thread.
pub fn list_apps(opts: &AppListOptions, progress: &Mutex<AppListProgress>, cancel: &AtomicBool) -> Vec<InstalledApp> {
    progress.lock().unwrap().message = "Looking for apps…".into();
    let mut bundles: Vec<PathBuf> = vec![];
    let mut seen = HashSet::new();
    for d in &opts.dirs {
        for b in app_bundles(d, 1) {
            if seen.insert(b.clone()) {
                bundles.push(b);
            }
        }
    }
    let procs = running_paths();
    let next = AtomicUsize::new(0);
    let out: Mutex<Vec<InstalledApp>> = Mutex::new(vec![]);
    std::thread::scope(|s| {
        for _ in 0..6 {
            s.spawn(|| loop {
                if cancel.load(Ordering::Relaxed) {
                    break;
                }
                let Some(path) = bundles.get(next.fetch_add(1, Ordering::SeqCst)) else { break };
                let Some(info) = read_bundle_info(path) else { continue };
                if info.id.as_deref().is_some_and(is_apple_or_dusty) || path.starts_with("/System") {
                    continue;
                }
                let stem = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
                let last_used = if opts.last_used { spotlight_last_used(path) } else { None };
                let app = InstalledApp {
                    name: info.display_name.clone().or(info.name.clone()).unwrap_or(stem),
                    bundle_id: info.id,
                    version: info.version,
                    bytes: 0,
                    last_used,
                    running: is_running(path, &procs),
                    writable: can_move(path),
                    app_store: path.join("Contents/_MASReceipt").exists(),
                    path: path.clone(),
                };
                let mut o = out.lock().unwrap();
                o.push(app);
                progress.lock().unwrap().found = o.len();
            });
        }
    });
    let mut apps = out.into_inner().unwrap();
    if opts.measure && !cancel.load(Ordering::Relaxed) {
        progress.lock().unwrap().message = format!("Measuring {} apps…", apps.len());
        let sizes = size_of(&apps.iter().map(|a| a.path.clone()).collect::<Vec<_>>());
        for a in apps.iter_mut() {
            a.bytes = sizes.get(&a.path).copied().unwrap_or(0);
        }
    }
    apps.sort_by_key(|a| a.name.to_lowercase());
    let mut p = progress.lock().unwrap();
    p.done = true;
    p.message = "Done".into();
    apps
}

/// One app by path (for the UI after a drop, or to refresh before uninstalling). `None` if it isn't
/// an app Dusty can offer.
pub fn app_at(path: &Path) -> Option<InstalledApp> {
    let info = read_bundle_info(path)?;
    if info.id.as_deref().is_some_and(is_apple_or_dusty) || path.starts_with("/System") || !path.to_string_lossy().to_lowercase().ends_with(".app") {
        return None;
    }
    let procs = running_paths();
    Some(InstalledApp {
        name: info.display_name.or(info.name).unwrap_or_else(|| path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default()),
        bundle_id: info.id,
        version: info.version,
        bytes: size_of(&[path.to_path_buf()]).values().sum(),
        last_used: None,
        running: is_running(path, &procs),
        writable: can_move(path),
        app_store: path.join("Contents/_MASReceipt").exists(),
        path: path.to_path_buf(),
    })
}

// ───────────────────────── leftovers ─────────────────────────

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum LeftoverKind {
    ApplicationSupport,
    Caches,
    Containers,
    GroupContainers,
    ApplicationScripts,
    Preferences,
    SavedState,
    HttpStorages,
    WebKit,
    Logs,
    LaunchAgents,
    Cookies,
}

impl LeftoverKind {
    pub fn label(self) -> &'static str {
        match self {
            LeftoverKind::ApplicationSupport => "App data",
            LeftoverKind::Caches => "Caches",
            LeftoverKind::Containers => "Sandbox container",
            LeftoverKind::GroupContainers => "Shared container",
            LeftoverKind::ApplicationScripts => "Scripts",
            LeftoverKind::Preferences => "Preferences",
            LeftoverKind::SavedState => "Saved window state",
            LeftoverKind::HttpStorages => "Web storage",
            LeftoverKind::WebKit => "WebKit data",
            LeftoverKind::Logs => "Logs",
            LeftoverKind::LaunchAgents => "Login item / agent",
            LeftoverKind::Cookies => "Cookies",
        }
    }
    pub fn key(self) -> &'static str {
        match self {
            LeftoverKind::ApplicationSupport => "application-support",
            LeftoverKind::Caches => "caches",
            LeftoverKind::Containers => "containers",
            LeftoverKind::GroupContainers => "group-containers",
            LeftoverKind::ApplicationScripts => "application-scripts",
            LeftoverKind::Preferences => "preferences",
            LeftoverKind::SavedState => "saved-state",
            LeftoverKind::HttpStorages => "http-storages",
            LeftoverKind::WebKit => "webkit",
            LeftoverKind::Logs => "logs",
            LeftoverKind::LaunchAgents => "launch-agents",
            LeftoverKind::Cookies => "cookies",
        }
    }
}

/// (kind, ~/Library subfolder, file extension the entry carries, name matches allowed)
const LOCATIONS: &[(LeftoverKind, &str, &str, bool)] = &[
    (LeftoverKind::ApplicationSupport, "Library/Application Support", "", true),
    (LeftoverKind::Caches, "Library/Caches", "", true),
    (LeftoverKind::Containers, "Library/Containers", "", false),
    (LeftoverKind::GroupContainers, "Library/Group Containers", "", false),
    (LeftoverKind::ApplicationScripts, "Library/Application Scripts", "", false),
    (LeftoverKind::Preferences, "Library/Preferences", ".plist", false),
    (LeftoverKind::Preferences, "Library/Preferences/ByHost", ".plist", false),
    (LeftoverKind::SavedState, "Library/Saved Application State", ".savedState", false),
    (LeftoverKind::HttpStorages, "Library/HTTPStorages", "", false),
    (LeftoverKind::HttpStorages, "Library/HTTPStorages", ".binarycookies", false),
    (LeftoverKind::WebKit, "Library/WebKit", "", false),
    (LeftoverKind::Logs, "Library/Logs", "", true),
    (LeftoverKind::LaunchAgents, "Library/LaunchAgents", ".plist", false),
    (LeftoverKind::Cookies, "Library/Cookies", ".binarycookies", false),
];

/// Names too generic (or shared by a vendor's apps) to ever match an app by name.
const GENERIC_NAMES: &[&str] = &[
    "google", "microsoft", "adobe", "apple", "jetbrains", "mozilla", "common", "shared", "helper", "helpers", "updater", "update", "electron",
    "crashreporter", "crashpad", "default", "data", "logs", "cache", "caches", "temp", "tmp", "app", "application", "support", "plugins",
    "extensions", "settings", "preferences", "config", "user", "users", "documents", "library", "dusty", "com", "org", "net", "io",
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MatchedBy {
    BundleId,
    Name,
}

#[derive(Clone, Debug)]
pub struct Leftover {
    pub path: PathBuf,
    pub kind: LeftoverKind,
    pub bytes: u64,
    #[allow(dead_code)] // why it matched: shown by diagnostics and checked by tests
    pub matched_by: MatchedBy,
    /// Suggested for removal. Off for names on the protected list (password managers, cloud sync,
    /// IDEs: their "leftovers" can be the only copy of real data).
    pub preselected: bool,
}

/// Does entry `name` (in a folder whose entries carry extension `ext`) belong to bundle id `bid`?
pub(crate) fn matches_bundle_id(name: &str, ext: &str, bid: &str, kind: LeftoverKind) -> bool {
    let n = name.to_lowercase();
    let b = bid.to_lowercase();
    if b.is_empty() || n.starts_with("com.apple.") || is_apple_or_dusty(&b) {
        return false;
    }
    let Some(stem) = (if ext.is_empty() { Some(n.as_str()) } else { n.strip_suffix(&ext.to_lowercase()) }) else { return false };
    // a prefix match is only safe when the id is more than a vendor ("com.vendor" would take all of them)
    let prefix_ok = b.split('.').filter(|s| !s.is_empty()).count() >= 3;
    let same = |s: &str| s == b || (prefix_ok && s.starts_with(&format!("{b}.")));
    match kind {
        LeftoverKind::GroupContainers => {
            if let Some(rest) = stem.strip_prefix("group.") {
                return same(rest);
            }
            // "<TEAMID>.<bundle id>": a 10-character team id
            match stem.split_once('.') {
                Some((team, rest)) if team.len() == 10 && team.chars().all(|c| c.is_ascii_alphanumeric()) => same(rest),
                _ => false,
            }
        }
        LeftoverKind::Preferences => {
            // ByHost: "<bundle id>.<host UUID>.plist"
            same(stem) || stem.strip_prefix(&format!("{b}.")).is_some_and(|host| host.len() >= 12 && !host.contains('.') && host.chars().all(|c| c.is_ascii_hexdigit() || c == '-'))
        }
        _ => same(stem),
    }
}

/// Does folder `name` match the app's name exactly (and is that name specific enough)?
pub(crate) fn matches_app_name(name: &str, app_names: &[String]) -> bool {
    let n = name.trim().to_lowercase();
    n.chars().count() >= 3 && !GENERIC_NAMES.contains(&n.as_str()) && !n.starts_with("com.apple") && app_names.iter().any(|a| a.trim().to_lowercase() == n)
}

/// Files and folders `app` leaves in ~/Library, biggest first, minus anything on the ignore list.
/// Apps without a bundle id get no leftovers (a name alone is too weak).
pub fn find_leftovers(app: &InstalledApp) -> Vec<Leftover> {
    let Some(bid) = app.bundle_id.as_deref() else { return vec![] };
    if is_apple_or_dusty(bid) {
        return vec![];
    }
    let ignore = IgnoreList::load();
    let stem = app.path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    let names = vec![app.name.clone(), stem];
    let mut found: Vec<(PathBuf, LeftoverKind, MatchedBy)> = vec![];
    let mut seen = HashSet::new();
    for (kind, rel, ext, by_name) in LOCATIONS {
        let dir = home().join(rel);
        for p in list_dir(&dir) {
            let n = name_of(&p);
            let Ok(m) = std::fs::symlink_metadata(&p) else { continue };
            if m.file_type().is_symlink() {
                continue;
            }
            // ByHost is its own location; don't offer the folder itself from Preferences
            if *kind == LeftoverKind::Preferences && m.is_dir() {
                continue;
            }
            let how = if matches_bundle_id(&n, ext, bid, *kind) {
                MatchedBy::BundleId
            } else if *by_name && ext.is_empty() && m.is_dir() && matches_app_name(&n, &names) {
                MatchedBy::Name
            } else {
                continue;
            };
            if ignore.blocks_path(&p) || !seen.insert(p.clone()) {
                continue;
            }
            found.push((p, *kind, how));
        }
    }
    let sizes = size_of(&found.iter().map(|f| f.0.clone()).collect::<Vec<_>>());
    let mut out: Vec<Leftover> = found
        .into_iter()
        .map(|(path, kind, matched_by)| Leftover {
            bytes: sizes.get(&path).copied().unwrap_or(0),
            preselected: !is_protected_name(&name_of(&path)),
            path,
            kind,
            matched_by,
        })
        .collect();
    out.sort_by(|a, b| b.bytes.cmp(&a.bytes));
    out
}

// ───────────────────────── uninstalling ─────────────────────────

#[derive(Clone, Debug)]
pub struct UninstallReport {
    /// What was recorded in the history (source `uninstall`).
    pub report: CleanReport,
    pub app_trashed: bool,
    /// The app needs an administrator to remove: nothing was moved.
    pub needs_admin: bool,
}

/// Checks that `path` is a removable third-party app bundle in an Applications folder.
fn validate_app(app: &InstalledApp) -> Result<(), String> {
    let p = &app.path;
    let show = p.to_string_lossy();
    if !p.is_absolute() || p.components().any(|c| matches!(c, std::path::Component::ParentDir)) || show.chars().any(|c| c.is_control()) {
        return Err(format!("Refusing to remove {show} (not a plain path)"));
    }
    if !show.to_lowercase().ends_with(".app") {
        return Err(format!("Refusing to remove {show} (not an app)"));
    }
    let m = std::fs::symlink_metadata(p).map_err(|_| format!("{} is already gone", app.name))?;
    if m.file_type().is_symlink() || !m.is_dir() {
        return Err(format!("Refusing to remove {show} (a link, not the app itself)"));
    }
    let real = std::fs::canonicalize(p).map_err(|e| format!("{show}: {e}"))?;
    let user_apps = std::fs::canonicalize(home().join("Applications")).unwrap_or_else(|_| home().join("Applications"));
    let in_apps = real.starts_with("/Applications") || real.starts_with(&user_apps);
    if !in_apps || real.starts_with("/System") || real == Path::new("/Applications") {
        return Err(format!("Refusing to remove {show} (only apps in an Applications folder)"));
    }
    // the Info.plist must still say what the list said
    let info = read_bundle_info(p).ok_or_else(|| format!("Refusing to remove {show} (no Info.plist)"))?;
    if info.id != app.bundle_id {
        return Err(format!("Refusing to remove {show} (it changed since the list was made)"));
    }
    if info.id.as_deref().is_some_and(is_apple_or_dusty) {
        return Err(format!("Refusing to remove {} (part of macOS, or Dusty itself)", app.name));
    }
    Ok(())
}

/// Moves `app` and the chosen `leftovers` to the Trash and records it in the history. Every
/// leftover must be one [`find_leftovers`] returns for this app right now (anything else is refused),
/// pass the safety rules and not be on the ignore list. Refuses (moving nothing) while the app is
/// running, and when the app needs an administrator (`needs_admin`). `dry_run` checks everything
/// and records a practice run. Blocking: call it from a background thread.
pub fn uninstall(app: &InstalledApp, leftovers: &[PathBuf], dry_run: bool) -> UninstallReport {
    let dry = force_dry(dry_run);
    let mut entry = HistoryEntry::new(HistorySource::Uninstall, dry, None);
    let mut errors: Vec<String> = vec![];
    let needs_admin = false;
    let ignore = IgnoreList::load();
    let finish = |entry: HistoryEntry, errors: Vec<String>, app_trashed, needs_admin| UninstallReport { report: entry.finish(errors, false), app_trashed, needs_admin };
    if let Some(why) = &ignore.broken {
        errors.push(format!("Nothing was moved: {why}."));
        return finish(entry, errors, false, false);
    }
    if let Err(e) = validate_app(app) {
        errors.push(e);
        return finish(entry, errors, false, false);
    }
    if is_running(&app.path, &running_paths()) {
        errors.push(format!("{} is open, so nothing was moved. Quit it and try again.", app.name));
        return finish(entry, errors, false, false);
    }
    if !can_move(&app.path) {
        errors.push(format!("{} was installed for all users, so removing it needs an administrator. Drag it to the Trash in Finder.", app.name));
        return finish(entry, errors, false, true);
    }
    if ignore.blocks_path(&app.path) {
        errors.push(ignore_refusal(&app.name));
        return finish(entry, errors, false, false);
    }
    // the app first: if it can't go, its settings stay too
    let app_bytes = if app.bytes > 0 { app.bytes } else { size_of(std::slice::from_ref(&app.path)).values().sum() };
    let (action, to, err) = if dry {
        (PathAction::Practice, None, None)
    } else {
        match move_to_trash(&app.path) {
            Ok(to) => (PathAction::Trashed, Some(to), None),
            Err(e) => (PathAction::Skipped, None, Some(format!("{}: {e}", app.name))),
        }
    };
    let app_trashed = action == PathAction::Trashed;
    entry.items.push(HistoryItem {
        id: format!("app:{}", app.bundle_id.clone().unwrap_or_default()),
        label: app.name.clone(),
        group: None,
        bytes: if action == PathAction::Skipped { 0 } else { app_bytes },
        removal: Removal::Trash,
        paths: vec![HistoryPath { path: app.path.clone(), action, trashed_to: to, bytes: Some(app_bytes), error: err.clone(), restored: false }],
    });
    if let Some(e) = err {
        errors.push(e);
        return finish(entry, errors, app_trashed, needs_admin);
    }
    let known: Vec<Leftover> = find_leftovers(app);
    for p in leftovers {
        let Some(l) = known.iter().find(|l| &l.path == p) else {
            errors.push(format!("Left {} alone: it isn't one of {}'s files.", tildify(p), app.name));
            continue;
        };
        let checked = assert_safe_allowing(p, UNINSTALL_ALLOW).and_then(|_| {
            if ignore.blocks_path(p) {
                Err(ignore_refusal(&tildify(p)))
            } else {
                Ok(())
            }
        });
        let (action, to, err) = trash_checked(p, checked, dry);
        if let Some(e) = &err {
            errors.push(e.clone());
        }
        entry.items.push(HistoryItem {
            id: format!("leftover:{}", l.kind.key()),
            label: format!("{} ({})", app.name, l.kind.label()),
            group: None,
            bytes: if action == PathAction::Skipped { 0 } else { l.bytes },
            removal: Removal::Trash,
            paths: vec![HistoryPath { path: p.clone(), action, trashed_to: to, bytes: Some(l.bytes), error: err, restored: false }],
        });
    }
    finish(entry, errors, app_trashed, needs_admin)
}

#[cfg(test)]
mod tests {
    use super::super::tests::{touch, FakeHome};
    use super::*;

    #[test]
    fn bundle_id_matching_is_strict() {
        use LeftoverKind::*;
        let b = "com.vendor.App";
        assert!(matches_bundle_id("com.vendor.App", "", b, ApplicationSupport));
        assert!(matches_bundle_id("com.vendor.app", "", b, Caches), "case-insensitive");
        assert!(matches_bundle_id("com.vendor.App.helper", "", b, Caches));
        assert!(!matches_bundle_id("com.vendor.AppPro", "", b, Caches), "dot boundary");
        assert!(!matches_bundle_id("com.vendor.Other", "", b, Caches));
        assert!(!matches_bundle_id("com.vendor", "", b, Caches));
        assert!(matches_bundle_id("com.vendor.App.plist", ".plist", b, Preferences));
        assert!(matches_bundle_id("com.vendor.App.0A1B2C3D-1111-2222-3333-444455556666.plist", ".plist", b, Preferences), "ByHost");
        assert!(!matches_bundle_id("com.vendor.App", ".plist", b, Preferences), "needs the extension");
        assert!(matches_bundle_id("com.vendor.App.savedState", ".savedState", b, SavedState));
        assert!(matches_bundle_id("group.com.vendor.App", "", b, GroupContainers));
        assert!(matches_bundle_id("ABCDE12345.com.vendor.App", "", b, GroupContainers));
        assert!(!matches_bundle_id("ABCDE12345.com.vendor.shared", "", b, GroupContainers), "a vendor-wide container isn't this app's");
        assert!(!matches_bundle_id("com.vendor.App", "", b, GroupContainers));
        // two-part ids: exact only
        assert!(matches_bundle_id("vendor.app", "", "vendor.app", Caches));
        assert!(!matches_bundle_id("vendor.app.other", "", "vendor.app", Caches));
        assert!(matches_bundle_id("vendor.app.0A1B2C3D-1111-2222.plist", ".plist", "vendor.app", Preferences), "ByHost is still this app's");
        // never Apple's, never Dusty's
        assert!(!matches_bundle_id("com.apple.Safari", "", "com.apple.Safari", Caches));
        assert!(!matches_bundle_id(DUSTY_BUNDLE_ID, "", DUSTY_BUNDLE_ID, Caches));
        assert!(is_apple_or_dusty("com.apple.dt.Xcode"));
        assert!(!is_apple_or_dusty("com.applevendor.x"));
    }

    #[test]
    fn name_matching_is_exact_and_specific() {
        let names = vec!["Fork".to_string(), "Fork".to_string()];
        assert!(matches_app_name("Fork", &names));
        assert!(matches_app_name("fork", &names));
        assert!(!matches_app_name("Fork Helper", &names));
        assert!(!matches_app_name("Google", &["Google".into()]), "vendor names never match");
        assert!(!matches_app_name("Go", &["Go".into()]), "too short");
    }

    /// A fake app in a fake ~/Applications with leftovers (and lookalikes) in a fake ~/Library.
    #[test]
    fn leftovers_and_uninstall_in_a_fake_home() {
        let fake = FakeHome::new("uninstall");
        let hm = fake.path.clone();
        let app_path = hm.join("Applications/Sparkly.app");
        std::fs::create_dir_all(app_path.join("Contents/MacOS")).unwrap();
        std::fs::write(
            app_path.join("Contents/Info.plist"),
            r#"<?xml version="1.0" encoding="UTF-8"?><!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd"><plist version="1.0"><dict><key>CFBundleIdentifier</key><string>com.example.sparkly</string><key>CFBundleName</key><string>Sparkly</string><key>CFBundleShortVersionString</key><string>1.2</string></dict></plist>"#,
        )
        .unwrap();
        touch(&app_path.join("Contents/MacOS/Sparkly"), 50_000);
        let lib = hm.join("Library");
        for f in [
            "Application Support/com.example.sparkly/db.sqlite",
            "Application Support/Sparkly/state.json", // by name
            "Application Support/Sparkly Pro/x",      // not this app
            "Application Support/com.example.sparklyplus/x",
            "Caches/com.example.sparkly/blob",
            "Caches/com.example.sparkly.helper/blob",
            "Containers/com.example.sparkly/Data/x",
            "Group Containers/ABCDE12345.com.example.sparkly/x",
            "Group Containers/ABCDE12345.com.example.shared/x",
            "Preferences/com.example.sparkly.plist",
            "Preferences/com.example.other.plist",
            "Saved Application State/com.example.sparkly.savedState/x",
            "HTTPStorages/com.example.sparkly/x",
            "WebKit/com.example.sparkly/x",
            "Logs/Sparkly/x.log",
            "LaunchAgents/com.example.sparkly.updater.plist",
            "Cookies/com.example.sparkly.binarycookies",
            "Caches/com.apple.Safari/x",
        ] {
            touch(&lib.join(f), 2000);
        }
        let opts = AppListOptions { dirs: vec![hm.join("Applications")], last_used: false, measure: true };
        let progress = Mutex::new(AppListProgress::default());
        let apps = list_apps(&opts, &progress, &AtomicBool::new(false));
        assert_eq!(apps.len(), 1);
        let app = &apps[0];
        assert_eq!(app.name, "Sparkly");
        assert_eq!(app.bundle_id.as_deref(), Some("com.example.sparkly"));
        assert_eq!(app.version.as_deref(), Some("1.2"));
        assert!(app.bytes >= 50_000 && app.writable && !app.running && app.blocked().is_none());

        let left = find_leftovers(app);
        let mut rel: Vec<String> = left.iter().map(|l| l.path.strip_prefix(&lib).unwrap().to_string_lossy().into_owned()).collect();
        rel.sort();
        assert_eq!(
            rel,
            [
                "Application Support/Sparkly",
                "Application Support/com.example.sparkly",
                "Caches/com.example.sparkly",
                "Caches/com.example.sparkly.helper",
                "Containers/com.example.sparkly",
                "Cookies/com.example.sparkly.binarycookies",
                "Group Containers/ABCDE12345.com.example.sparkly",
                "HTTPStorages/com.example.sparkly",
                "LaunchAgents/com.example.sparkly.updater.plist",
                "Logs/Sparkly",
                "Preferences/com.example.sparkly.plist",
                "Saved Application State/com.example.sparkly.savedState",
                "WebKit/com.example.sparkly",
            ]
        );
        assert!(left.iter().find(|l| l.path.ends_with("Application Support/Sparkly")).unwrap().matched_by == MatchedBy::Name);

        // practice run: nothing moves
        let picks: Vec<PathBuf> = left.iter().map(|l| l.path.clone()).chain([lib.join("Preferences/com.example.other.plist")]).collect();
        let r = uninstall(app, &picks, true);
        assert!(r.report.entry.practice && app_path.exists() && !r.app_trashed);
        assert_eq!(r.report.errors.len(), 1, "the stranger is refused: {:?}", r.report.errors);
        // for real
        let r = uninstall(app, &picks, false);
        assert!(r.app_trashed, "{:?}", r.report.errors);
        assert!(!app_path.exists() && hm.join(".Trash/Sparkly.app").exists());
        assert!(!lib.join("Preferences/com.example.sparkly.plist").exists());
        assert!(lib.join("Preferences/com.example.other.plist").exists());
        assert!(lib.join("Application Support/Sparkly Pro/x").exists());
        assert!(lib.join("Group Containers/ABCDE12345.com.example.shared/x").exists());
        assert_eq!(r.report.entry.items.len(), 14);
        assert_eq!(r.report.errors.len(), 1, "{:?}", r.report.errors);
        // and it all comes back
        let pb = super::super::history::put_back(&r.report.entry.id, None);
        assert_eq!(pb.restored.len(), 14, "{:?}", pb.errors);
        assert!(app_path.join("Contents/Info.plist").exists());
    }
}
