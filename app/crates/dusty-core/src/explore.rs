//! Look further: the big folders in your Library and dot folders that no catalog entry knows, as plain
//! facts for a helper (an AI, see the `dusty-ai` crate) to explain, and the gate every suggestion it makes
//! passes before it can be shown, let alone moved.
//!
//! Nothing in this module talks to an AI. It decides what the helper may see and what may be suggested:
//! - [`survey`] lists folders under your home, outside protected and personal data, minus your ignore
//!   list and everything a checkup already covers ([`Known`]). It describes them with names, sizes,
//!   ages and the app that owns them, never what is inside a file;
//! - [`check`] lets a suggestion through only if it is something the survey showed (or inside it),
//!   passes the same safety rules as a clean, holds no code (git repos), isn't a whole data folder of an
//!   app that is still installed, and doesn't belong to an app that is open;
//! - suggestions are never ticked for you, and [`trash`] moves them to the Trash, never `rm`.
//!
//! What was looked at and suggested is kept in `~/.config/dusty/explore.json` ([`Memory`]): the next trip
//! skips folders that haven't grown, and offers earlier suggestions again without asking the helper twice.

use super::history::{HistoryEntry, HistoryItem, HistoryPath, HistorySource, PathAction};
use super::trash::Removal;
use super::uninstall::{default_app_dirs, is_running, read_bundle_info, running_paths};
use super::*;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::os::unix::fs::MetadataExt;

/// Folders smaller than this aren't worth a helper's time.
pub const MIN_BYTES: u64 = 100 * 1024 * 1024;
/// Inside a folder the helper asked to look closer at, entries from this size are listed.
const CLOSER_MIN_BYTES: u64 = 10 * 1024 * 1024;
/// At most this many folders per trip, biggest first.
const MAX_FOLDERS: usize = 40;
/// Biggest entries described inside each folder.
const INSIDE: usize = 8;
/// A folder looked at before is skipped until it grows by a quarter, or this many days pass.
const LOOK_AGAIN_DAYS: u64 = 90;

/// Library folders where each entry belongs to one app (by name or bundle id).
const APP_DIRS: &[&str] = &["Library/Application Support", "Library/Caches", "Library/Containers", "Library/Group Containers", "Library/Logs", "Library/HTTPStorages", "Library/WebKit"];
/// Of those, the ones that hold an app's real data: never suggested whole while the app is installed.
const APP_DATA_DIRS: &[&str] = &["Library/Application Support", "Library/Containers", "Library/Group Containers"];
/// Your own top-level folders: the survey never lists them whole.
const PERSONAL: &[&str] = &["Applications", "Desktop", "Documents", "Downloads", "Library", "Movies", "Music", "Pictures", "Public", "Developer", "Projects"];

// ───────────────────────── what a checkup already covers ─────────────────────────

/// Paths already handled elsewhere (a checkup's items, earlier suggestions): never shown or suggested again.
#[derive(Default, Clone, Debug)]
pub struct Known {
    paths: HashSet<PathBuf>,
    /// every ancestor of a known path (a folder holding something known)
    above: HashSet<PathBuf>,
}

impl Known {
    pub fn new<'a>(paths: impl IntoIterator<Item = &'a PathBuf>) -> Self {
        let mut k = Known::default();
        for p in paths {
            k.add(p);
        }
        k
    }
    /// Every path of a checkup's items.
    pub fn from_targets(targets: &[Target]) -> Self {
        Known::new(targets.iter().flat_map(|t| &t.paths))
    }
    pub fn add(&mut self, p: &Path) {
        self.above.extend(p.ancestors().skip(1).map(Path::to_path_buf));
        self.paths.insert(p.to_path_buf());
    }
    /// `p` is known, or inside something known.
    pub fn covers(&self, p: &Path) -> bool {
        p.ancestors().any(|a| self.paths.contains(a))
    }
    /// `p` holds something known.
    pub fn holds(&self, p: &Path) -> bool {
        self.above.contains(p)
    }
}

// ───────────────────────── who owns a folder ─────────────────────────

#[derive(Clone, Debug, PartialEq)]
pub enum Owner {
    /// Not an app's folder (a dot folder, a tool's cache…).
    Unknown,
    /// An app in /Applications matches the folder's name or bundle id.
    Installed { name: String, running: bool },
    /// The folder sits where apps keep their files, but no installed app matches it.
    NoApp,
    /// macOS's own (`com.apple.*`).
    Apple,
}

/// Installed apps by name and bundle id, read once per trip.
pub struct Apps {
    apps: Vec<(Vec<String>, String, PathBuf)>,
    procs: Vec<String>,
}

impl Apps {
    pub fn load() -> Self {
        let mut apps = vec![];
        for dir in default_app_dirs() {
            for app in list_dir(&dir).into_iter().filter(|p| p.extension().is_some_and(|e| e == "app")) {
                let stem = app.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
                let info = read_bundle_info(&app).unwrap_or_default();
                let keys: Vec<String> = [Some(stem.clone()), info.id, info.name, info.display_name].into_iter().flatten().map(|k| k.to_lowercase()).collect();
                apps.push((keys, stem, app));
            }
        }
        Apps { apps, procs: running_paths() }
    }

    /// Who owns `p`, judged by its first folder under one of [`APP_DIRS`].
    pub fn owner(&self, p: &Path) -> Owner {
        let Some(name) = app_folder(p).map(|(_, n)| n) else { return Owner::Unknown };
        let n = name.to_lowercase();
        // Group Containers: "TEAMID.com.foo.bar" or "group.com.foo.bar"
        let bare = n.strip_prefix("group.").map(str::to_string).unwrap_or_else(|| match n.split_once('.') {
            Some((team, rest)) if team.len() == 10 && team.chars().all(|c| c.is_ascii_alphanumeric()) => rest.to_string(),
            _ => n.clone(),
        });
        if bare.starts_with("com.apple.") || n.starts_with("com.apple.") {
            return Owner::Apple;
        }
        let hit = |k: &String| *k == n || *k == bare || bare.starts_with(&format!("{k}.")) || (k.contains('.') && k.starts_with(&format!("{bare}.")));
        match self.apps.iter().find(|(keys, _, _)| keys.iter().any(hit)) {
            Some((_, name, path)) => Owner::Installed { name: name.clone(), running: is_running(path, &self.procs) },
            None => Owner::NoApp,
        }
    }
}

/// `(app dir, entry name)` when `p` is inside one of [`APP_DIRS`].
fn app_folder(p: &Path) -> Option<(&'static str, String)> {
    APP_DIRS.iter().find_map(|d| {
        let rel = p.strip_prefix(h(d)).ok()?;
        Some((*d, rel.components().next()?.as_os_str().to_string_lossy().into_owned()))
    })
}

// ───────────────────────── the survey ─────────────────────────

/// One folder as the helper sees it.
#[derive(Clone, Debug)]
pub struct Folder {
    pub path: PathBuf,
    pub bytes: u64,
    /// Days since anything directly inside it changed.
    pub untouched_days: u64,
    /// The biggest entries inside, as (name, bytes, is a folder).
    pub inside: Vec<(String, u64, bool)>,
    /// How many smaller entries were left out of `inside`.
    pub more: usize,
    pub owner: Owner,
}

/// What a trip showed the helper. Suggestions must stay inside it.
pub struct Survey {
    pub folders: Vec<Folder>,
    /// Folders skipped because they haven't grown since last time.
    pub unchanged: usize,
    /// The time budget ran out before everything was measured.
    pub partial: bool,
    pub known: Known,
    pub apps: Apps,
    shown: Vec<PathBuf>,
}

impl Survey {
    /// Shown to the helper (in the survey, or in a closer look).
    pub fn showed(&self, p: &Path) -> bool {
        self.shown.iter().any(|s| p.starts_with(s))
    }
}

/// Where to look: entries of the app folders, each app container's caches, the dot folders and loose
/// folders of your home.
fn candidates() -> Vec<PathBuf> {
    let mut out = vec![];
    for d in APP_DIRS.iter().filter(|d| **d != "Library/Containers").chain(&["Library/Developer", ".cache", ".local/share", ".local/state"]) {
        out.extend(list_dir(&h(d)));
    }
    out.extend(list_dir(&h("Library/Containers")).into_iter().map(|c| c.join("Data/Library/Caches")));
    for p in list_dir(&home()) {
        let name = name_of(&p);
        let skip = [".Trash", ".cache", ".local", ".config"].contains(&name.as_str()) || PERSONAL.contains(&name.as_str());
        if !skip {
            out.push(p);
        }
    }
    out.retain(|p| std::fs::symlink_metadata(p).is_ok_and(|m| m.is_dir()));
    out
}

/// May the helper see `p` at all? Inside home, not protected or personal, not ignored, not already known.
fn visible(p: &Path, known: &Known, ignore: &IgnoreList) -> bool {
    assert_safe(p).is_ok() && !protected_below_home(p) && !ignore.blocks_path(p) && !known.covers(p) && !known.holds(p)
}

/// Any folder between home and `p` (or `p`) on the protected-names list.
fn protected_below_home(p: &Path) -> bool {
    p.strip_prefix(home()).map(|rel| rel.components().any(|c| is_protected_name(&c.as_os_str().to_string_lossy()))).unwrap_or(true)
}

/// Describes `dirs`: their visible entries measured in one parallel pass. Folders under `min` are left out.
fn describe(dirs: &[PathBuf], known: &Known, ignore: &IgnoreList, apps: &Apps, min: u64, deadline: Instant, cancel: &AtomicBool) -> Vec<Folder> {
    let dev = std::fs::metadata(home()).map(|m| m.dev()).ok();
    let now = SystemTime::now();
    let days = |t: SystemTime| now.duration_since(t).map(|d| d.as_secs() / 86400).unwrap_or(0);
    // (dir, its entries as (path, is dir), newest change)
    type Listed = (PathBuf, Vec<(PathBuf, bool)>, SystemTime);
    let mut listed: Vec<Listed> = vec![];
    for d in dirs {
        let Ok(rd) = std::fs::read_dir(d) else { continue };
        let mut newest = mtime(d);
        let mut entries = vec![];
        for e in rd.flatten() {
            let Ok(m) = e.metadata() else { continue };
            if dev.is_some_and(|v| v != m.dev()) || is_dataless(&m) || m.file_type().is_symlink() {
                continue;
            }
            if let Ok(t) = m.modified() {
                newest = newest.max(t);
            }
            let p = e.path();
            if visible(&p, known, ignore) {
                entries.push((p, m.is_dir()));
            }
        }
        listed.push((d.clone(), entries, newest));
    }
    let all: Vec<PathBuf> = listed.iter().flat_map(|(_, es, _)| es.iter().map(|(p, _)| p.clone())).collect();
    let sizes = size_of_with(&all, deadline, cancel, &Mutex::new(HashSet::new()));
    let mut out: Vec<Folder> = listed
        .into_iter()
        .filter_map(|(path, entries, newest)| {
            let mut inside: Vec<(String, u64, bool)> = entries.iter().map(|(p, dir)| (name_of(p), sizes.get(p).copied().unwrap_or(0), *dir)).collect();
            let bytes = inside.iter().map(|e| e.1).sum();
            if bytes < min {
                return None;
            }
            inside.sort_by_key(|e| std::cmp::Reverse(e.1));
            let more = inside.len().saturating_sub(INSIDE);
            inside.truncate(INSIDE);
            let owner = apps.owner(&path);
            Some(Folder { path, bytes, untouched_days: days(newest), inside, more, owner })
        })
        .collect();
    out.sort_by_key(|f| std::cmp::Reverse(f.bytes));
    out
}

/// The big folders nobody explained yet, biggest first. Blocking: call it from a background thread.
pub fn survey(known: Known, memory: &Memory, cancel: &AtomicBool) -> Survey {
    let ignore = IgnoreList::load();
    let apps = Apps::load();
    let deadline = Instant::now() + Duration::from_secs(90);
    let dirs: Vec<PathBuf> = candidates().into_iter().filter(|p| visible(p, &known, &ignore)).collect();
    let mut folders = if ignore.broken.is_some() { vec![] } else { describe(&dirs, &known, &ignore, &apps, MIN_BYTES, deadline, cancel) };
    let before = folders.len();
    folders.retain(|f| !memory.unchanged(f));
    let unchanged = before - folders.len();
    folders.truncate(MAX_FOLDERS);
    let shown = folders.iter().map(|f| f.path.clone()).collect();
    Survey { folders, unchanged, partial: Instant::now() > deadline, known, apps, shown }
}

/// The folders inside `paths` (folders the helper asked about), described like the survey. Paths it
/// wasn't shown, or may not see, are skipped.
pub fn look_closer(s: &mut Survey, paths: &[PathBuf], cancel: &AtomicBool) -> Vec<Folder> {
    let ignore = IgnoreList::load();
    let asked: Vec<PathBuf> = paths.iter().filter(|p| s.showed(p) && is_dir(p) && visible(p, &s.known, &ignore)).take(6).cloned().collect();
    let deadline = Instant::now() + Duration::from_secs(45);
    let found = describe(&asked, &s.known, &ignore, &s.apps, CLOSER_MIN_BYTES, deadline, cancel);
    s.shown.extend(found.iter().map(|f| f.path.clone()));
    found
}

// ───────────────────────── suggestions ─────────────────────────

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Risk {
    /// Rebuilt by itself, nothing is lost.
    Low,
    /// Costs a download, a sign-in or a slow first launch.
    Medium,
}

/// Something the helper thinks can go, with its reasons. Never ticked by default.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Suggestion {
    pub path: PathBuf,
    /// Measured by Dusty, never taken from the helper.
    pub bytes: u64,
    /// A short name ("Old Ollama models").
    pub what: String,
    /// What it is and why it can go.
    pub why: String,
    /// What happens once it's gone ("downloaded again the next time you use it").
    pub comes_back: String,
    pub risk: Risk,
    #[serde(with = "group_key")]
    pub group: Group,
    /// Facts the helper based it on ("not opened in 200 days").
    #[serde(default)]
    pub evidence: Vec<String>,
    /// Offered on an earlier trip and still here.
    #[serde(default, skip_serializing)]
    pub seen_before: bool,
    /// Days since anything directly inside it changed, measured by Dusty.
    #[serde(default, skip_serializing)]
    pub untouched_days: Option<u64>,
}

mod group_key {
    use super::Group;
    use serde::{Deserialize, Deserializer, Serializer};
    pub fn serialize<S: Serializer>(g: &Group, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(g.key())
    }
    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Group, D::Error> {
        Ok(Group::from_key(&String::deserialize(d)?).unwrap_or(Group::Apps))
    }
}

/// `~/…` or an absolute path, as the helper writes it.
pub fn expand(s: &str) -> Option<PathBuf> {
    let s = s.trim().trim_end_matches('/');
    match s.strip_prefix("~/") {
        Some(rest) if !rest.is_empty() => Some(home().join(rest)),
        _ if s.starts_with('/') => Some(PathBuf::from(s)),
        _ => None,
    }
}

/// The checks that hold whenever a suggestion is shown or moved: the safety rules, protected names, the
/// ignore list, no code, no whole data folder of an installed app, and the owning app isn't open.
fn safe_to_suggest(p: &Path, apps: &Apps, ignore: &IgnoreList) -> Result<(), String> {
    assert_safe(p)?;
    let shown = tildify(p);
    if protected_below_home(p) {
        return Err(format!("{shown} belongs to an app whose files Dusty never touches"));
    }
    if ignore.blocks_path(p) {
        return Err(ignore_refusal(&shown));
    }
    let m = std::fs::symlink_metadata(p).map_err(|_| format!("{shown} is already gone"))?;
    if m.file_type().is_symlink() || is_dataless(&m) {
        return Err(format!("{shown} isn't stored on this Mac"));
    }
    if p.ancestors().take_while(|a| *a != home()).any(|a| a.join(".git").exists()) || holds_repo(p) {
        return Err(format!("{shown} holds code (a git repository)"));
    }
    if p.parent() == Some(home().as_path()) && !name_of(p).starts_with('.') {
        return Err(format!("{shown} is one of your own folders"));
    }
    let owner = apps.owner(p);
    if let Some((dir, _)) = app_folder(p) {
        let whole = p.parent() == Some(h(dir).as_path());
        if whole && APP_DATA_DIRS.contains(&dir) && owner != Owner::NoApp {
            return Err(format!("{shown} is an app's own data, and the app is still here"));
        }
    }
    if let Owner::Installed { name, running: true } = owner {
        return Err(format!("{name} is open. Quit it and look again."));
    }
    Ok(())
}

/// A `.git` a few levels down (a folder of checkouts).
fn holds_repo(p: &Path) -> bool {
    let mut stack = vec![(p.to_path_buf(), 0)];
    let mut seen = 0;
    while let Some((d, depth)) = stack.pop() {
        for e in list_dir(&d) {
            seen += 1;
            if seen > 4000 {
                return false;
            }
            if name_of(&e) == ".git" {
                return true;
            }
            if depth < 3 && std::fs::symlink_metadata(&e).is_ok_and(|m| m.is_dir()) {
                stack.push((e, depth + 1));
            }
        }
    }
    false
}

/// The gate: may the helper's suggestion for `p` be shown? Returns its size, measured by Dusty.
pub fn check(p: &Path, s: &Survey) -> Result<u64, String> {
    if !s.showed(p) {
        return Err(format!("{} wasn't in what Dusty showed", tildify(p)));
    }
    if s.known.covers(p) || s.known.holds(p) {
        return Err(format!("{} is already in your checkup", tildify(p)));
    }
    safe_to_suggest(p, &s.apps, &IgnoreList::load())?;
    Ok(size_of(&[p.to_path_buf()]).values().sum())
}

/// Days since `p` or anything directly inside it changed (`None` when it can't be read).
pub fn untouched_days(p: &Path) -> Option<u64> {
    let mut newest = std::fs::symlink_metadata(p).and_then(|m| m.modified()).ok()?;
    for e in std::fs::read_dir(p).ok()?.flatten() {
        if let Ok(t) = e.metadata().and_then(|m| m.modified()) {
            newest = newest.max(t);
        }
    }
    Some(SystemTime::now().duration_since(newest).map(|d| d.as_secs() / 86400).unwrap_or(0))
}

/// Moves the suggestions you picked to the Trash and records it in the history (source `explore`). Each
/// one passes the checks again first. `dry_run` checks everything and records a practice run.
pub fn trash(items: &[Suggestion], dry_run: bool) -> CleanReport {
    let dry = force_dry(dry_run);
    let ignore = IgnoreList::load();
    let apps = Apps::load();
    let mut entry = HistoryEntry::new(HistorySource::Explore, dry, None);
    let mut errors = vec![];
    if let Some(why) = &ignore.broken {
        errors.push(format!("Nothing was moved: {why}."));
        return entry.finish(errors, false);
    }
    for s in items {
        let bytes = size_of(std::slice::from_ref(&s.path)).values().sum::<u64>();
        let (action, to, err) = trash_checked(&s.path, safe_to_suggest(&s.path, &apps, &ignore), dry);
        if let Some(e) = &err {
            errors.push(e.clone());
        }
        let freed = if action == PathAction::Skipped { 0 } else { bytes };
        entry.items.push(HistoryItem {
            id: "explore".into(),
            label: s.what.clone(),
            group: Some(s.group),
            bytes: freed,
            removal: Removal::Trash,
            paths: vec![HistoryPath { path: s.path.clone(), action, trashed_to: to, bytes: Some(bytes), error: err, restored: false }],
        });
    }
    entry.finish(errors, false)
}

// ───────────────────────── memory ─────────────────────────

/// What earlier trips looked at and suggested (`~/.config/dusty/explore.json`).
#[derive(Default, Serialize, Deserialize)]
pub struct Memory {
    /// Folder (`~/…`) → (bytes, unix seconds) when it was last shown.
    #[serde(default)]
    looked: BTreeMap<String, (u64, u64)>,
    #[serde(default)]
    suggestions: Vec<Suggestion>,
    /// Paths you asked checkups to look for (written to your catalog).
    #[serde(default)]
    remembered: Vec<PathBuf>,
}

pub fn memory_file() -> PathBuf {
    h(".config/dusty/explore.json")
}

impl Memory {
    /// Never fails: an unreadable or garbled file is a fresh start.
    pub fn load() -> Self {
        std::fs::read_to_string(memory_file()).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
    }

    pub fn save(&self) -> Result<(), String> {
        let text = serde_json::to_string_pretty(self).map_err(|e| e.to_string())?;
        write_atomic(&memory_file(), text.as_bytes())
    }

    /// Paths that are no longer news: earlier suggestions and what you remembered.
    pub fn known_paths(&self) -> impl Iterator<Item = &PathBuf> {
        self.suggestions.iter().map(|s| &s.path).chain(&self.remembered)
    }

    /// Shown recently, and hasn't grown by a quarter since.
    fn unchanged(&self, f: &Folder) -> bool {
        self.looked.get(&tildify(&f.path)).is_some_and(|(bytes, at)| now_secs().saturating_sub(*at) < LOOK_AGAIN_DAYS * 86400 && f.bytes <= bytes + bytes / 4)
    }

    /// Earlier suggestions that are still here and still pass the checks, measured again, biggest first.
    pub fn still_here(&self, known: &Known) -> Vec<Suggestion> {
        let ignore = IgnoreList::load();
        let apps = Apps::load();
        let mut out: Vec<Suggestion> = self.suggestions
            .iter()
            .filter(|s| !known.covers(&s.path) && !self.remembered.contains(&s.path) && safe_to_suggest(&s.path, &apps, &ignore).is_ok())
            .filter_map(|s| {
                let bytes = size_of(std::slice::from_ref(&s.path)).values().sum::<u64>();
                (bytes >= CLOSER_MIN_BYTES).then(|| Suggestion { bytes, seen_before: true, untouched_days: untouched_days(&s.path), ..s.clone() })
            })
            .collect();
        out.sort_by_key(|s| std::cmp::Reverse(s.bytes));
        out
    }

    /// Notes what this trip showed and suggested.
    pub fn record(&mut self, folders: &[Folder], suggested: &[Suggestion]) {
        let now = now_secs();
        for f in folders {
            self.looked.insert(tildify(&f.path), (f.bytes, now));
        }
        for s in suggested {
            self.suggestions.retain(|o| o.path != s.path);
            self.suggestions.push(Suggestion { seen_before: false, ..s.clone() });
        }
    }
}

/// Adds the suggestions to your catalog (`~/.config/dusty/catalog.toml`) so checkups look for them, unticked,
/// from the next launch on. Returns how many were added (ones already there are skipped).
pub fn remember(items: &[Suggestion]) -> Result<usize, String> {
    let file = user_catalog_file();
    let old = match std::fs::read_to_string(&file) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(format!("couldn't read {} ({e})", tildify(&file))),
    };
    let q = |s: &str| toml::Value::String(s.to_string()).to_string();
    let mut add = String::new();
    let mut n = 0;
    for s in items {
        let Ok(rel) = s.path.strip_prefix(home()) else { continue };
        let rel = rel.to_string_lossy();
        if old.contains(&q(&rel)) {
            continue;
        }
        let slug: String = s.what.to_lowercase().chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '-' }).collect();
        let slug = slug.split('-').filter(|w| !w.is_empty()).take(5).collect::<Vec<_>>().join("-");
        let id = format!("found-{slug}-{:x}", now_secs() % 0xfffff + n as u64);
        let guard = if app_folder(&s.path).is_some_and(|(d, _)| d == "Library/Application Support") { "running = \"each-app\"\n" } else { "" };
        let why: String = s.why.chars().filter(|c| !c.is_control()).take(200).collect();
        add.push_str(&format!("\n# Found with Dusty's helper: {why}\n[[target]]\nid = {}\nlabel = {}\ngroup = {}\n{guard}paths = [{}]\n", q(&id), q(&s.what), q(s.group.key()), q(&rel)));
        n += 1;
    }
    if n == 0 {
        return Ok(0);
    }
    let new = format!("{}{add}", if old.is_empty() || old.ends_with('\n') { old.clone() } else { format!("{old}\n") });
    // all or nothing: the file must still load, or it isn't written
    catalog::Catalog::parse(catalog::PROJECTS, catalog::SYSTEM).and_then(|mut c| c.add_user(&new)).map_err(|e| format!("couldn't add to your catalog ({e})"))?;
    write_atomic(&file, new.as_bytes())?;
    let mut m = Memory::load();
    m.remembered.extend(items.iter().map(|s| s.path.clone()));
    m.save()?;
    Ok(n)
}

#[cfg(test)]
mod tests {
    use super::super::tests::{touch, FakeHome};
    use super::*;

    fn mb(n: usize) -> usize {
        n * 1024 * 1024
    }

    #[test]
    fn survey_shows_only_unknown_and_safe_folders() {
        let fh = FakeHome::new("explore-survey");
        let hm = &fh.path;
        touch(&hm.join("Library/Application Support/Gone App/models/big.bin"), mb(120));
        touch(&hm.join("Library/Application Support/Known/cache.bin"), mb(120));
        touch(&hm.join("Library/Application Support/1Password/data.bin"), mb(120));
        touch(&hm.join(".tooldata/blob"), mb(110));
        touch(&hm.join("Library/Application Support/Small/x"), mb(1));
        let known = Known::new(&[hm.join("Library/Application Support/Known")]);
        let s = survey(known, &Memory::default(), &AtomicBool::new(false));
        let names: Vec<String> = s.folders.iter().map(|f| name_of(&f.path)).collect();
        assert!(names.contains(&"Gone App".to_string()) && names.contains(&".tooldata".to_string()), "{names:?}");
        assert!(!names.iter().any(|n| n == "Known" || n == "1Password" || n == "Small"), "{names:?}");
        let gone = s.folders.iter().find(|f| name_of(&f.path) == "Gone App").unwrap();
        assert_eq!(gone.owner, Owner::NoApp);
        assert_eq!(gone.inside[0].0, "models");

        // a folder shown last time that hasn't grown is skipped
        let mut m = Memory::default();
        m.record(&s.folders, &[]);
        let again = survey(Known::default(), &m, &AtomicBool::new(false));
        assert!(again.folders.iter().all(|f| name_of(&f.path) == "Known"), "{:?}", again.folders.iter().map(|f| &f.path).collect::<Vec<_>>());
        assert!(again.unchanged >= 2);
    }

    #[test]
    fn the_gate() {
        let fh = FakeHome::new("explore-gate");
        let hm = &fh.path;
        touch(&hm.join("Library/Application Support/Gone App/models/big.bin"), mb(120));
        touch(&hm.join("Library/Application Support/Gone App/repo/.git/HEAD"), 10);
        touch(&hm.join(".tooldata/cache/blob"), mb(110));
        let s = survey(Known::default(), &Memory::default(), &AtomicBool::new(false));
        let ok = check(&hm.join("Library/Application Support/Gone App/models"), &s).unwrap();
        assert!(ok >= mb(120) as u64);
        assert!(check(&hm.join(".tooldata/cache"), &s).is_ok());
        // code, things it wasn't shown, invented paths, personal data
        assert!(check(&hm.join("Library/Application Support/Gone App/repo"), &s).unwrap_err().contains("code"));
        assert!(check(&hm.join("Library/Application Support/Gone App"), &s).unwrap_err().contains("code"));
        assert!(check(&hm.join("Documents/thesis"), &s).is_err());
        assert!(check(&hm.join(".tooldata/nope"), &s).unwrap_err().contains("gone"));
        assert!(check(&hm.join("Library/Keychains"), &s).is_err());
        assert_eq!(expand("~/Library/Caches/x/"), Some(hm.join("Library/Caches/x")));
        assert_eq!(expand("Library"), None);
    }

    #[test]
    fn suggestions_go_to_the_trash_and_can_be_remembered() {
        let fh = FakeHome::new("explore-trash");
        let hm = &fh.path;
        touch(&hm.join(".tooldata/cache/blob"), mb(11));
        let s = Suggestion {
            path: hm.join(".tooldata/cache"),
            bytes: 0,
            what: "Tool cache".into(),
            why: "Rebuilt on next run".into(),
            comes_back: "rebuilt".into(),
            risk: Risk::Low,
            group: Group::Cache,
            evidence: vec![],
            seen_before: false,
            untouched_days: None,
        };
        let mut m = Memory::default();
        m.record(&[], std::slice::from_ref(&s));
        m.save().unwrap();
        let back = Memory::load().still_here(&Known::default());
        assert!(back.len() == 1 && back[0].seen_before && back[0].bytes >= mb(11) as u64);

        assert_eq!(remember(std::slice::from_ref(&s)).unwrap(), 1);
        assert_eq!(remember(std::slice::from_ref(&s)).unwrap(), 0, "added once");
        let text = std::fs::read_to_string(user_catalog_file()).unwrap();
        assert!(text.contains("paths = [\".tooldata/cache\"]") && text.contains("group = \"cache\""), "{text}");
        assert!(Memory::load().still_here(&Known::default()).is_empty(), "remembered ones are the checkup's now");

        let r = trash(std::slice::from_ref(&s), true);
        assert!(r.entry.practice && r.errors.is_empty() && hm.join(".tooldata/cache").exists());
    }
}
