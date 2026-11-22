//! Cleaning with a removal policy, and the clean history.
//!
//! Every clean (app, scheduled, uninstall, large files; practice runs too) is appended as one JSON
//! line to `~/Library/Application Support/Dusty/history.jsonl`. "Put back" appends a small
//! `put_back` line instead of rewriting the file, and [`load_history`] folds those in.
//!
//! One line per clean:
//! ```json
//! {"type":"clean","id":"1732190000123-4242-0","ts":1732190000,"date":"2026-11-21T10:33:20Z",
//!  "source":"app","practice":false,"policy":"trash-unless-cache","bytes":123,
//!  "items":[{"id":"xcode-deriveddata","label":"Xcode DerivedData","group":"cache","bytes":123,
//!            "removal":"delete","paths":[{"path":"/Users/me/…","action":"deleted"}]}],
//!  "errors":[]}
//! {"type":"put_back","entry":"1732190000123-4242-0","path":"/Users/me/…","ts":1732190100}
//! ```

use super::plan::CleanReport;
use super::trash::{self, CleanPolicy, Removal};
use super::{exists, home, now_secs, tildify, Group};
use serde_json::{json, Value};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::time::{SystemTime, UNIX_EPOCH};

// ───────────────────────── model ─────────────────────────

/// Who started a clean.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HistorySource {
    App,
    Scheduled,
    Uninstall,
    LargeFiles,
    /// Suggestions from Dusty's helper ("Look further").
    Explore,
    Cli,
}

impl HistorySource {
    pub fn key(self) -> &'static str {
        match self {
            HistorySource::App => "app",
            HistorySource::Scheduled => "scheduled",
            HistorySource::Uninstall => "uninstall",
            HistorySource::LargeFiles => "large-files",
            HistorySource::Explore => "explore",
            HistorySource::Cli => "cli",
        }
    }
    pub fn from_key(k: &str) -> Self {
        match k {
            "scheduled" => HistorySource::Scheduled,
            "uninstall" => HistorySource::Uninstall,
            "large-files" => HistorySource::LargeFiles,
            "explore" => HistorySource::Explore,
            "cli" => HistorySource::Cli,
            _ => HistorySource::App,
        }
    }
}

/// What happened to one path.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PathAction {
    Deleted,
    Trashed,
    /// Practice run: nothing happened.
    Practice,
    /// Left alone (see `error`).
    Skipped,
}

impl PathAction {
    fn key(self) -> &'static str {
        match self {
            PathAction::Deleted => "deleted",
            PathAction::Trashed => "trashed",
            PathAction::Practice => "practice",
            PathAction::Skipped => "skipped",
        }
    }
    fn from_key(k: &str) -> Self {
        match k {
            "deleted" => PathAction::Deleted,
            "trashed" => PathAction::Trashed,
            "practice" => PathAction::Practice,
            _ => PathAction::Skipped,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct HistoryPath {
    pub path: PathBuf,
    pub action: PathAction,
    /// Where it lives in the Trash (only for `Trashed`).
    pub trashed_to: Option<PathBuf>,
    /// Size of this path when known (uninstall, large files); targets only know their total.
    pub bytes: Option<u64>,
    pub error: Option<String>,
    /// Put back since (folded in from `put_back` lines).
    pub restored: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct HistoryItem {
    /// Target id (`xcode-deriveddata`, `proj:…`), `app:<bundle id>`, `leftover:<kind>`, `large-file`…
    pub id: String,
    pub label: String,
    pub group: Option<Group>,
    /// Bytes freed (or, in a practice run, that would be freed) by this item.
    pub bytes: u64,
    pub removal: Removal,
    pub paths: Vec<HistoryPath>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct HistoryEntry {
    pub id: String,
    /// Unix seconds.
    pub ts: u64,
    /// ISO 8601, UTC ("2026-11-21T10:33:20Z").
    pub date: String,
    pub source: HistorySource,
    pub practice: bool,
    pub policy: Option<CleanPolicy>,
    /// Total of `items[].bytes`.
    pub bytes: u64,
    pub items: Vec<HistoryItem>,
    pub errors: Vec<String>,
}

impl HistoryEntry {
    /// Paths that are still in the Trash and can be put back, as (original, in-trash).
    pub fn restorable(&self) -> Vec<(PathBuf, PathBuf)> {
        self.items
            .iter()
            .flat_map(|i| &i.paths)
            .filter(|p| p.action == PathAction::Trashed && !p.restored)
            .filter_map(|p| Some((p.path.clone(), p.trashed_to.clone()?)))
            .filter(|(_, t)| exists(t))
            .collect()
    }
    /// Bytes that went to the Trash (not freed until the Trash is emptied).
    pub fn bytes_trashed(&self) -> u64 {
        self.items.iter().filter(|i| i.removal == Removal::Trash).map(|i| i.bytes).sum()
    }
    /// Bytes deleted outright.
    pub fn bytes_deleted(&self) -> u64 {
        self.items.iter().filter(|i| i.removal == Removal::Delete).map(|i| i.bytes).sum()
    }

    /// Totals the items, records the entry in the history (when it has any) and builds the report.
    pub(crate) fn finish(mut self, errors: Vec<String>, cancelled: bool) -> CleanReport {
        self.bytes = self.items.iter().map(|i| i.bytes).sum();
        self.errors = errors.clone();
        let history_error = if self.items.is_empty() { None } else { append_entry(&self).err() };
        CleanReport { entry: self, errors, history_error, cancelled }
    }

    pub(crate) fn new(source: HistorySource, practice: bool, policy: Option<CleanPolicy>) -> Self {
        static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default();
        HistoryEntry {
            // unique even for two cleans in the same millisecond (a practice run, then the real one)
            id: format!("{}-{}-{}", now.as_millis(), std::process::id(), SEQ.fetch_add(1, Ordering::Relaxed)),
            ts: now.as_secs(),
            date: iso_utc(now.as_secs()),
            source,
            practice,
            policy,
            bytes: 0,
            items: vec![],
            errors: vec![],
        }
    }

    /// The entry as it is stored in the history file (one JSON line).
    pub fn to_json(&self) -> Value {
        json!({
            "type": "clean",
            "id": self.id,
            "ts": self.ts,
            "date": self.date,
            "source": self.source.key(),
            "practice": self.practice,
            "policy": self.policy.map(|p| p.key()),
            "bytes": self.bytes,
            "items": self.items.iter().map(|i| json!({
                "id": i.id,
                "label": i.label,
                "group": i.group.map(|g| g.key()),
                "bytes": i.bytes,
                "removal": i.removal.key(),
                "paths": i.paths.iter().map(|p| {
                    let mut o = json!({ "path": p.path.to_string_lossy(), "action": p.action.key() });
                    if let Some(t) = &p.trashed_to { o["trashed_to"] = json!(t.to_string_lossy()); }
                    if let Some(b) = p.bytes { o["bytes"] = json!(b); }
                    if let Some(e) = &p.error { o["error"] = json!(e); }
                    o
                }).collect::<Vec<_>>(),
            })).collect::<Vec<_>>(),
            "errors": self.errors,
        })
    }

    fn from_json(v: &Value) -> Option<Self> {
        let s = |v: &Value, k: &str| v.get(k).and_then(|x| x.as_str()).map(String::from);
        let items = v
            .get("items")?
            .as_array()?
            .iter()
            .filter_map(|i| {
                Some(HistoryItem {
                    id: s(i, "id")?,
                    label: s(i, "label").unwrap_or_default(),
                    group: s(i, "group").and_then(|g| Group::from_key(&g)),
                    bytes: i.get("bytes").and_then(|b| b.as_u64()).unwrap_or(0),
                    removal: if s(i, "removal").as_deref() == Some("trash") { Removal::Trash } else { Removal::Delete },
                    paths: i
                        .get("paths")
                        .and_then(|p| p.as_array())
                        .map(|ps| {
                            ps.iter()
                                .filter_map(|p| {
                                    Some(HistoryPath {
                                        path: PathBuf::from(s(p, "path")?),
                                        action: PathAction::from_key(&s(p, "action").unwrap_or_default()),
                                        trashed_to: s(p, "trashed_to").map(PathBuf::from),
                                        bytes: p.get("bytes").and_then(|b| b.as_u64()),
                                        error: s(p, "error"),
                                        restored: false,
                                    })
                                })
                                .collect()
                        })
                        .unwrap_or_default(),
                })
            })
            .collect();
        Some(HistoryEntry {
            id: s(v, "id")?,
            ts: v.get("ts").and_then(|t| t.as_u64()).unwrap_or(0),
            date: s(v, "date").unwrap_or_default(),
            source: HistorySource::from_key(&s(v, "source").unwrap_or_default()),
            practice: v.get("practice").and_then(|p| p.as_bool()).unwrap_or(false),
            policy: s(v, "policy").and_then(|p| CleanPolicy::from_key(&p)),
            bytes: v.get("bytes").and_then(|b| b.as_u64()).unwrap_or(0),
            items,
            errors: v.get("errors").and_then(|e| e.as_array()).map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect()).unwrap_or_default(),
        })
    }
}

// ───────────────────────── files ─────────────────────────

/// `~/Library/Application Support/Dusty`, Dusty's own data folder.
pub fn support_dir() -> PathBuf {
    home().join("Library/Application Support/Dusty")
}

pub fn history_file() -> PathBuf {
    support_dir().join("history.jsonl")
}

fn append_line(v: &Value) -> Result<(), String> {
    let f = history_file();
    std::fs::create_dir_all(f.parent().unwrap()).map_err(|e| format!("couldn't create {} ({e})", tildify(f.parent().unwrap())))?;
    let mut line = v.to_string();
    line.push('\n');
    let mut file = std::fs::OpenOptions::new().create(true).append(true).open(&f).map_err(|e| format!("couldn't open {} ({e})", tildify(&f)))?;
    // one write per line: concurrent writers (app + scheduled run) never interleave inside a line
    file.write_all(line.as_bytes()).map_err(|e| format!("couldn't write {} ({e})", tildify(&f)))
}

/// Appends one entry to the history file.
pub fn append_entry(e: &HistoryEntry) -> Result<(), String> {
    append_line(&e.to_json())
}

/// The whole history, newest first, with put-backs folded in. Unreadable lines are skipped.
pub fn load_history() -> Vec<HistoryEntry> {
    let Ok(text) = std::fs::read_to_string(history_file()) else { return vec![] };
    parse_history(&text)
}

fn parse_history(text: &str) -> Vec<HistoryEntry> {
    let mut entries: Vec<HistoryEntry> = vec![];
    for line in text.lines().filter(|l| !l.trim().is_empty()) {
        let Ok(v) = serde_json::from_str::<Value>(line) else { continue };
        match v.get("type").and_then(|t| t.as_str()) {
            Some("put_back") => {
                let (Some(id), Some(path)) = (v.get("entry").and_then(|x| x.as_str()), v.get("path").and_then(|x| x.as_str())) else { continue };
                if let Some(e) = entries.iter_mut().find(|e| e.id == id) {
                    for p in e.items.iter_mut().flat_map(|i| i.paths.iter_mut()).filter(|p| p.path == Path::new(path)) {
                        p.restored = true;
                    }
                }
            }
            _ => entries.extend(HistoryEntry::from_json(&v)),
        }
    }
    entries.reverse();
    entries
}

// ───────────────────────── put back ─────────────────────────

#[derive(Clone, Debug, Default)]
pub struct PutBackReport {
    pub restored: Vec<PathBuf>,
    pub errors: Vec<String>,
}

/// Puts the trashed items of history entry `entry_id` back where they were. `only` limits it to
/// some original paths (`None` = everything still in the Trash). An item whose original location
/// is taken again is left in the Trash and reported.
pub fn put_back(entry_id: &str, only: Option<&[PathBuf]>) -> PutBackReport {
    let mut report = PutBackReport::default();
    let Some(entry) = load_history().into_iter().find(|e| e.id == entry_id) else {
        report.errors.push("That clean isn't in the history any more.".into());
        return report;
    };
    for (original, trashed) in entry.restorable() {
        if only.is_some_and(|o| !o.contains(&original)) {
            continue;
        }
        match trash::put_back_item(&trashed, &original) {
            Ok(()) => {
                let now = now_secs();
                if let Err(e) = append_line(&json!({ "type": "put_back", "entry": entry.id, "path": original.to_string_lossy(), "ts": now })) {
                    report.errors.push(e);
                }
                report.restored.push(original);
            }
            Err(e) => report.errors.push(e),
        }
    }
    report
}

// ───────────────────────── dates ─────────────────────────

/// Unix seconds → "YYYY-MM-DDTHH:MM:SSZ".
pub fn iso_utc(secs: u64) -> String {
    let days = (secs / 86400) as i64;
    let rem = secs % 86400;
    // civil-from-days (Howard Hinnant)
    let z = days + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!("{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z", rem / 3600, rem % 3600 / 60, rem % 60)
}

#[cfg(test)]
mod tests {
    use super::super::tests::{touch, FakeHome};
    use super::super::{execute, CleanPlan, CleanProgress, Guard, Target};
    use super::*;
    use std::sync::atomic::AtomicBool;

    fn target(id: &str, group: Group, paths: Vec<PathBuf>, bytes: u64) -> Target {
        Target { id: id.into(), label: id.into(), group, default_on: true, bytes, paths, commands: vec![], detail: None, guard: Guard::None, wipes_data: false }
    }

    #[test]
    fn dates() {
        assert_eq!(iso_utc(0), "1970-01-01T00:00:00Z");
        assert_eq!(iso_utc(1_700_000_000), "2023-11-14T22:13:20Z");
        assert_eq!(iso_utc(951_782_400), "2000-02-29T00:00:00Z");
    }

    #[test]
    fn clean_with_policy_records_history_and_puts_back() {
        let fake = FakeHome::new("history");
        let cache = fake.path.join("Library/Caches/com.example.App/blob");
        let log = fake.path.join("Library/Logs/com.example.App/old.log");
        let kept = fake.path.join("Library/Logs/keep-me/x.log");
        touch(&cache, 5000);
        touch(&log, 300_000);
        touch(&kept, 10);
        std::fs::create_dir_all(fake.path.join(".config/dusty")).unwrap();
        std::fs::write(fake.path.join(".config/dusty/ignore"), "keep-me\n").unwrap();
        let targets = vec![
            target("library-caches", Group::Apps, vec![cache.parent().unwrap().to_path_buf()], 5000),
            target("library-logs", Group::Junk, vec![log.parent().unwrap().to_path_buf(), kept.parent().unwrap().to_path_buf()], 300_010),
            target("outside", Group::Junk, vec![PathBuf::from("/tmp/dusty-never")], 1),
        ];
        let clean = |practice, progress: &mut CleanProgress| {
            let plan = CleanPlan::new(&targets, CleanPolicy::TrashUnlessCache, HistorySource::App, practice);
            execute(&plan, &mut |e| progress.apply(&e), &AtomicBool::new(false))
        };
        // practice run first: nothing moves, but it's recorded
        let r = clean(true, &mut CleanProgress::default());
        assert!(r.entry.practice && cache.exists() && log.exists());
        let mut progress = CleanProgress::default();
        let r = clean(false, &mut progress);
        assert!(r.history_error.is_none(), "{:?}", r.history_error);
        assert!(!cache.exists(), "caches are deleted outright");
        assert!(fake.path.join(".Trash/com.example.App/old.log").exists(), "logs go to the Trash");
        assert!(kept.exists(), "ignore list wins");
        assert_eq!(r.errors.len(), 2, "{:?}", r.errors); // ignore list + outside home
        assert_eq!((progress.done, progress.total, progress.item_errors.len()), (3, 3, 3));

        let h = load_history();
        assert_eq!(h.len(), 2);
        let e = &h[0];
        assert!(!e.practice && h[1].practice, "newest first");
        assert_eq!(e.items[0].removal, Removal::Delete);
        assert_eq!(e.items[0].paths[0].action, PathAction::Deleted);
        assert_eq!(e.items[1].removal, Removal::Trash);
        assert_eq!(e.items[1].paths[0].action, PathAction::Trashed);
        assert_eq!(e.items[1].paths[1].action, PathAction::Skipped);
        assert!(e.items[1].bytes > 0 && e.items[1].bytes < 300_010, "what stayed behind isn't counted: {}", e.items[1].bytes);
        assert_eq!(e.items[2].bytes, 0);
        assert_eq!(e.restorable().len(), 1);

        // put back: refuses when the spot is taken, works once it's free, and is remembered
        touch(&log, 1);
        let pb = put_back(&e.id, None);
        assert!(pb.restored.is_empty() && pb.errors.len() == 1);
        std::fs::remove_dir_all(log.parent().unwrap()).unwrap();
        let pb = put_back(&e.id, None);
        assert_eq!(pb.restored, vec![log.parent().unwrap().to_path_buf()], "{:?}", pb.errors);
        assert_eq!(std::fs::metadata(&log).unwrap().len(), 300_000);
        let h = load_history();
        assert!(h[0].restorable().is_empty());
        assert!(h[0].items[1].paths[0].restored);
    }

    #[test]
    fn history_tolerates_garbage() {
        let h = parse_history("not json\n{\"type\":\"put_back\",\"entry\":\"nope\",\"path\":\"/x\"}\n{\"type\":\"clean\",\"id\":\"a\",\"items\":[]}\n");
        assert_eq!(h.len(), 1);
        assert_eq!(h[0].id, "a");
    }
}
