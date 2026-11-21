//! Large & old files: a review list, never a sweep.
//!
//! Looks through your own folders (Downloads, Desktop, Movies, Documents by default) for single
//! files that are big, or that you haven't opened in a long time. Nothing here is ever preselected:
//! the person picks files, and picked files go to the Trash ([`trash_files`]), never `rm`.
//!
//! Never looked at: hidden files and folders, the inside of packages and bundles (apps, Photos and
//! iMovie libraries, Logic projects, Pages documents, VM bundles…), git repositories (a folder with a
//! `.git` is skipped whole), iCloud placeholders (opening them downloads them), other volumes,
//! symlinks, protected personal data and anything on your ignore list.

use super::history::{HistoryEntry, HistoryItem, HistoryPath, HistorySource, PathAction};
use super::{trash_checked, CleanReport};
use super::trash::Removal;
use super::{assert_safe_allowing, force_dry, home, ignore_refusal, is_dataless, iso_days, name_of, protected_path_except, run_s, tildify, walk_workers, IgnoreList, USER_PICK_ALLOW};
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

pub const DEFAULT_MIN_BYTES: u64 = 200 * 1024 * 1024;
pub const DEFAULT_UNUSED_DAYS: u64 = 180;

#[derive(Clone, Debug)]
pub struct LargeFileOptions {
    pub roots: Vec<PathBuf>,
    /// "Large" means at least this many bytes on disk (default 200 MB).
    pub min_bytes: u64,
    /// "Old" means not opened or changed for this many days (default 180). `None`: size only.
    pub unused_days: Option<u64>,
    /// Old files smaller than this are left out, so the list isn't thousands of tiny files (10 MB).
    pub old_min_bytes: u64,
    /// Only files that are both large and old (default: large or old).
    pub require_both: bool,
    /// Wall-clock budget for the walk.
    pub budget: Duration,
    /// Keep at most this many results (biggest first).
    pub max_results: usize,
}

impl Default for LargeFileOptions {
    fn default() -> Self {
        LargeFileOptions {
            roots: default_large_roots(),
            min_bytes: DEFAULT_MIN_BYTES,
            unused_days: Some(DEFAULT_UNUSED_DAYS),
            old_min_bytes: 10 * 1024 * 1024,
            require_both: false,
            budget: Duration::from_secs(120),
            max_results: 500,
        }
    }
}

/// ~/Downloads, ~/Desktop, ~/Movies and ~/Documents (those that exist).
pub fn default_large_roots() -> Vec<PathBuf> {
    ["Downloads", "Desktop", "Movies", "Documents"].iter().map(|d| home().join(d)).filter(|p| p.is_dir()).collect()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FileKind {
    Video,
    Audio,
    Image,
    DiskImage,
    Installer,
    Archive,
    Document,
    Other,
}

impl FileKind {
    pub fn of(path: &Path) -> FileKind {
        let ext = path.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
        match ext.as_str() {
            "mov" | "mp4" | "m4v" | "mkv" | "avi" | "webm" | "mts" | "braw" | "r3d" | "prores" => FileKind::Video,
            "mp3" | "wav" | "aiff" | "aif" | "flac" | "m4a" | "caf" => FileKind::Audio,
            "jpg" | "jpeg" | "png" | "heic" | "tif" | "tiff" | "psd" | "raw" | "dng" | "cr2" | "cr3" | "nef" | "arw" => FileKind::Image,
            "dmg" | "iso" | "img" | "vmdk" | "qcow2" | "vdi" | "vhdx" | "sparseimage" | "ipsw" => FileKind::DiskImage,
            "pkg" | "mpkg" | "exe" | "msi" | "apk" | "xip" => FileKind::Installer,
            "zip" | "tar" | "gz" | "tgz" | "bz2" | "xz" | "7z" | "rar" | "zst" => FileKind::Archive,
            "pdf" | "key" | "pages" | "numbers" | "docx" | "pptx" | "xlsx" | "epub" => FileKind::Document,
            _ => FileKind::Other,
        }
    }
}

#[derive(Clone, Debug)]
pub struct LargeFile {
    pub path: PathBuf,
    /// Space it takes on disk (allocated blocks): what you get back once the Trash is emptied.
    pub bytes: u64,
    /// Best guess of the last time it was opened: Spotlight's "last opened" when it has one,
    /// otherwise the later of access and modification times.
    pub last_used: SystemTime,
    pub is_large: bool,
    pub is_old: bool,
    pub kind: FileKind,
    /// Other hard links to the same data exist: trashing this one frees nothing until they go too.
    pub hard_linked: bool,
}

impl LargeFile {
    /// Whole days since it was last opened.
    pub fn days_unused(&self) -> u64 {
        SystemTime::now().duration_since(self.last_used).map(|d| d.as_secs() / 86400).unwrap_or(0)
    }
}

/// Live progress for the UI.
#[derive(Default, Clone, Debug)]
pub struct LargeProgress {
    pub message: String,
    pub files_seen: usize,
    /// Results so far (unsorted, before the Spotlight check).
    pub found: Vec<LargeFile>,
    pub done: bool,
    /// The time budget ran out before every folder was seen.
    pub partial: bool,
}

/// Folder extensions macOS shows as a single document or app: never looked inside.
const PACKAGE_EXTS: &[&str] = &[
    "app", "bundle", "framework", "plugin", "kext", "appex", "xpc", "photoslibrary", "photolibrary", "aplibrary", "migratedphotolibrary",
    "fcpbundle", "imovielibrary", "tvlibrary", "musiclibrary", "theater", "logicx", "band", "rtfd", "pages", "numbers", "key", "sparsebundle",
    "xcarchive", "xcodeproj", "xcworkspace", "playground", "dsym", "utm", "pvm", "vmwarevm", "pkg", "mpkg", "docarchive", "lproj", "scptd",
    "sketch", "affinity", "lrlibrary", "lrdata", "cocatalog", "dtbase2", "nib", "storyboardc", "mlmodelc", "abbu", "wdgt", "saver", "prefPane",
    "qlgenerator", "mdimporter", "component", "vst", "vst3", "aaxplugin", "noindex",
];

fn is_package(dir: &Path, name: &str) -> bool {
    let ext = Path::new(name).extension().map(|e| e.to_string_lossy().to_lowercase());
    ext.is_some_and(|e| PACKAGE_EXTS.iter().any(|p| p.eq_ignore_ascii_case(&e))) || dir.join("Contents/Info.plist").exists()
}

const SKIP_DIRS: &[&str] = &["node_modules", "Library", "Pods", "DerivedData", "build", "target", "venv"];

/// Walks `opts.roots` for large and/or old files. Blocking and parallel: call it from a background
/// thread. Results are biggest first.
pub fn find_large_files(opts: &LargeFileOptions, progress: &Mutex<LargeProgress>, cancel: &AtomicBool) -> Vec<LargeFile> {
    let ignore = IgnoreList::load();
    let deadline = Instant::now() + opts.budget;
    let old_cutoff = opts.unused_days.map(|d| SystemTime::now() - Duration::from_secs(d * 86400));
    progress.lock().unwrap().message = "Looking for big files…".into();
    let queue: Mutex<Vec<(PathBuf, u64)>> = Mutex::new(
        opts.roots
            .iter()
            .filter(|r| !protected_path_except(r, USER_PICK_ALLOW) && !ignore.blocks_path(r))
            .filter_map(|r| Some((r.clone(), std::fs::symlink_metadata(r).ok().filter(|m| m.is_dir())?.dev())))
            .collect(),
    );
    let busy = AtomicUsize::new(0);
    let seen = AtomicUsize::new(0);
    let found: Mutex<Vec<LargeFile>> = Mutex::new(vec![]);
    let timed_out = AtomicBool::new(false);
    let workers = walk_workers();
    std::thread::scope(|s| {
        for _ in 0..workers {
            s.spawn(|| loop {
                if cancel.load(Ordering::Relaxed) {
                    break;
                }
                if Instant::now() > deadline {
                    timed_out.store(true, Ordering::Relaxed);
                    break;
                }
                let item = {
                    let mut q = queue.lock().unwrap();
                    let it = q.pop();
                    if it.is_some() {
                        busy.fetch_add(1, Ordering::SeqCst);
                    }
                    it
                };
                let Some((dir, dev)) = item else {
                    if busy.load(Ordering::SeqCst) == 0 {
                        break;
                    }
                    std::thread::sleep(Duration::from_millis(2));
                    continue;
                };
                let entries: Vec<_> = std::fs::read_dir(&dir).map(|rd| rd.flatten().collect()).unwrap_or_default();
                // a git repository: its files are code and history, never a cleanup suggestion
                let is_repo = entries.iter().any(|e| e.file_name() == ".git");
                let mut sub = vec![];
                if !is_repo {
                    for e in &entries {
                        let name = e.file_name().to_string_lossy().into_owned();
                        if name.starts_with('.') {
                            continue;
                        }
                        let Ok(m) = e.metadata() else { continue }; // lstat
                        if m.file_type().is_symlink() || m.dev() != dev || is_dataless(&m) {
                            continue;
                        }
                        let path = e.path();
                        if m.is_dir() {
                            if SKIP_DIRS.contains(&name.as_str()) || is_package(&path, &name) || protected_path_except(&path, USER_PICK_ALLOW) {
                                continue;
                            }
                            sub.push((path, dev));
                        } else if m.is_file() {
                            seen.fetch_add(1, Ordering::Relaxed);
                            if let Some(f) = judge(&path, &m, opts, old_cutoff) {
                                if !ignore.blocks_path(&path) && !protected_path_except(&path, USER_PICK_ALLOW) {
                                    found.lock().unwrap().push(f.clone());
                                    progress.lock().unwrap().found.push(f);
                                }
                            }
                        }
                    }
                }
                {
                    let mut p = progress.lock().unwrap();
                    p.files_seen = seen.load(Ordering::Relaxed);
                    p.message = format!("Looking in {}", tildify(&dir));
                }
                queue.lock().unwrap().extend(sub);
                busy.fetch_sub(1, Ordering::SeqCst);
            });
        }
    });
    let mut out = found.into_inner().unwrap();
    // Spotlight knows when a file was last opened; ask only about the files that look old
    if let Some(cutoff) = old_cutoff {
        progress.lock().unwrap().message = "Checking when they were last opened…".into();
        refine_last_used(&mut out, cancel);
        for f in out.iter_mut() {
            f.is_old = f.last_used < cutoff;
        }
        out.retain(|f| keep(f.is_large, f.is_old, f.bytes, opts));
    }
    out.sort_by(|a, b| b.bytes.cmp(&a.bytes));
    out.truncate(opts.max_results);
    let mut p = progress.lock().unwrap();
    p.done = true;
    p.partial = timed_out.load(Ordering::Relaxed);
    p.message = "Done".into();
    out
}

fn keep(is_large: bool, is_old: bool, bytes: u64, opts: &LargeFileOptions) -> bool {
    if opts.require_both {
        is_large && is_old
    } else {
        is_large || (is_old && bytes >= opts.old_min_bytes)
    }
}

fn judge(path: &Path, m: &std::fs::Metadata, opts: &LargeFileOptions, old_cutoff: Option<SystemTime>) -> Option<LargeFile> {
    let bytes = m.blocks() * 512;
    if bytes < opts.min_bytes.min(if old_cutoff.is_some() { opts.old_min_bytes } else { u64::MAX }) {
        return None; // fast path: too small to matter either way
    }
    let modified = m.modified().unwrap_or(UNIX_EPOCH);
    let accessed = m.accessed().unwrap_or(UNIX_EPOCH);
    let last_used = modified.max(accessed);
    let is_large = bytes >= opts.min_bytes;
    let is_old = old_cutoff.is_some_and(|c| last_used < c);
    // (Spotlight can only make a file look *less* old later, so this verdict is an upper bound)
    if !keep(is_large, is_old, bytes, opts) {
        return None;
    }
    Some(LargeFile { path: path.to_path_buf(), bytes, last_used, is_large, is_old, kind: FileKind::of(path), hard_linked: m.nlink() > 1 })
}

/// Moves `last_used` later when Spotlight saw the file opened more recently (only asked about
/// files that look old: for the others the answer changes nothing).
fn refine_last_used(files: &mut [LargeFile], cancel: &AtomicBool) {
    let next = AtomicUsize::new(0);
    let dates: Vec<Mutex<Option<SystemTime>>> = files.iter().map(|_| Mutex::new(None)).collect();
    let ask: Vec<usize> = (0..files.len()).filter(|i| files[*i].is_old).collect();
    std::thread::scope(|s| {
        for _ in 0..4 {
            s.spawn(|| loop {
                if cancel.load(Ordering::Relaxed) {
                    break;
                }
                let Some(&i) = ask.get(next.fetch_add(1, Ordering::SeqCst)) else { break };
                *dates[i].lock().unwrap() = spotlight_last_used(&files[i].path);
            });
        }
    });
    for (f, d) in files.iter_mut().zip(dates) {
        if let Some(d) = d.into_inner().unwrap() {
            f.last_used = f.last_used.max(d);
        }
    }
}

/// Spotlight's "last opened" date for a file or app, when it knows one.
pub(crate) fn spotlight_last_used(p: &Path) -> Option<SystemTime> {
    let r = run_s("/usr/bin/mdls", &["-raw", "-name", "kMDItemLastUsedDate", &p.to_string_lossy()], 10);
    r.ok.then(|| parse_mdls_date(&r.stdout)).flatten()
}

/// "2026-11-20 09:12:33 +0000" (mdls) → time. `(null)` and anything else → None.
pub(crate) fn parse_mdls_date(s: &str) -> Option<SystemTime> {
    let s = s.trim();
    let days = iso_days(s)?;
    let hms = s.get(11..19)?;
    let mut it = hms.split(':').map(|x| x.parse::<i64>().ok());
    let (hh, mm, ss) = (it.next()??, it.next()??, it.next()??);
    let off = s.get(20..25).and_then(|o| {
        let sign = if o.starts_with('-') { -1 } else { 1 };
        Some(sign * (o.get(1..3)?.parse::<i64>().ok()? * 3600 + o.get(3..5)?.parse::<i64>().ok()? * 60))
    });
    let secs = days * 86400 + hh * 3600 + mm * 60 + ss - off.unwrap_or(0);
    (secs >= 0).then(|| UNIX_EPOCH + Duration::from_secs(secs as u64))
}

/// Moves files the person picked to the Trash and records it in the history (source `large-files`).
/// Each file must still be a regular file on this Mac, pass the safety rules (Desktop, Movies, Music
/// and Pictures are allowed here because you picked the file yourself) and not be on the ignore list.
/// `dry_run` checks everything and records a practice run.
pub fn trash_files(files: &[PathBuf], dry_run: bool) -> CleanReport {
    let dry = force_dry(dry_run);
    let ignore = IgnoreList::load();
    let mut entry = HistoryEntry::new(HistorySource::LargeFiles, dry, None);
    let mut errors = vec![];
    if let Some(why) = &ignore.broken {
        errors.push(format!("Nothing was moved: {why}."));
    } else {
        for p in files {
            let meta = std::fs::symlink_metadata(p);
            let bytes = meta.as_ref().map(|m| m.blocks() * 512).ok();
            let check = || -> Result<(), String> {
                assert_safe_allowing(p, USER_PICK_ALLOW)?;
                let m = meta.as_ref().map_err(|_| format!("{} is already gone", tildify(p)))?;
                if !m.is_file() {
                    return Err(format!("{} isn't a plain file", tildify(p)));
                }
                if ignore.blocks_path(p) {
                    return Err(ignore_refusal(&tildify(p)));
                }
                Ok(())
            };
            let (action, to, err) = trash_checked(p, check(), dry);
            if let Some(e) = &err {
                errors.push(e.clone());
            }
            let freed = if action == PathAction::Skipped { 0 } else { bytes.unwrap_or(0) };
            entry.items.push(HistoryItem {
                id: "large-file".into(),
                label: name_of(p),
                group: None,
                bytes: freed,
                removal: Removal::Trash,
                paths: vec![HistoryPath { path: p.clone(), action, trashed_to: to, bytes, error: err, restored: false }],
            });
        }
    }
    entry.finish(errors, false)
}

#[cfg(test)]
mod tests {
    use super::super::tests::{set_old, touch, FakeHome};
    use super::*;

    #[test]
    fn mdls_dates() {
        assert_eq!(parse_mdls_date("1970-01-02 00:00:10 +0000"), Some(UNIX_EPOCH + Duration::from_secs(86410)));
        assert_eq!(parse_mdls_date("1970-01-02 02:00:00 +0200"), Some(UNIX_EPOCH + Duration::from_secs(86400)));
        assert_eq!(parse_mdls_date("(null)"), None);
    }

    fn sparse(p: &Path, len: u64) {
        // a real allocation is needed (sizes are blocks on disk): write that many bytes
        touch(p, len as usize);
    }

    #[test]
    fn finds_large_and_old_in_a_fake_home() {
        let fake = FakeHome::new("large");
        let hm = fake.path.clone();
        let mb = 1024 * 1024;
        sparse(&hm.join("Downloads/big.dmg"), 3 * mb);
        sparse(&hm.join("Downloads/small.txt"), 1000);
        sparse(&hm.join("Downloads/old.zip"), 2 * mb);
        set_old(&hm.join("Downloads/old.zip"), 400);
        sparse(&hm.join("Downloads/old-tiny.txt"), 10);
        set_old(&hm.join("Downloads/old-tiny.txt"), 400);
        sparse(&hm.join("Movies/clip.mov"), 4 * mb);
        sparse(&hm.join("Movies/Trip.imovielibrary/Media/x.mov"), 5 * mb); // package internals
        sparse(&hm.join("Movies/Pics.photoslibrary/originals/x.heic"), 5 * mb);
        sparse(&hm.join("Documents/repo/.git/HEAD"), 10);
        sparse(&hm.join("Documents/repo/assets/huge.psd"), 5 * mb); // inside a git repo
        sparse(&hm.join("Documents/.hidden/big.bin"), 5 * mb);
        sparse(&hm.join("Documents/node_modules/x/big.bin"), 5 * mb);
        sparse(&hm.join("Documents/keep/big.iso"), 5 * mb);
        sparse(&hm.join("Desktop/demo.mp4"), 3 * mb + mb / 2);
        std::fs::create_dir_all(hm.join(".config/dusty")).unwrap();
        std::fs::write(hm.join(".config/dusty/ignore"), "~/Documents/keep\n").unwrap();
        let opts = LargeFileOptions { roots: default_large_roots(), min_bytes: 2 * mb + mb / 2, old_min_bytes: mb, ..Default::default() };
        let progress = Mutex::new(LargeProgress::default());
        let got = find_large_files(&opts, &progress, &AtomicBool::new(false));
        let rel: Vec<String> = got.iter().map(|f| f.path.strip_prefix(&hm).unwrap().to_string_lossy().into_owned()).collect();
        assert_eq!(rel, ["Movies/clip.mov", "Desktop/demo.mp4", "Downloads/big.dmg", "Downloads/old.zip"], "biggest first");
        let old = got.iter().find(|f| f.path.ends_with("old.zip")).unwrap();
        assert!(old.is_old && !old.is_large && old.days_unused() >= 399);
        assert_eq!(FileKind::of(&got[0].path), FileKind::Video);
        assert!(progress.lock().unwrap().done);
        // both: nothing is large and old here
        let both = LargeFileOptions { require_both: true, ..opts.clone() };
        assert!(find_large_files(&both, &Mutex::new(LargeProgress::default()), &AtomicBool::new(false)).is_empty());
        // size only
        let size_only = LargeFileOptions { unused_days: None, ..opts.clone() };
        assert_eq!(find_large_files(&size_only, &Mutex::new(LargeProgress::default()), &AtomicBool::new(false)).len(), 3);

        // trash: practice first, then for real; ignored, protected and missing files are refused
        let picks = vec![hm.join("Desktop/demo.mp4"), hm.join("Documents/keep/big.iso"), hm.join("Movies/Pics.photoslibrary/originals/x.heic"), hm.join("Downloads/nope")];
        let r = trash_files(&picks, true);
        assert!(r.entry.practice && hm.join("Desktop/demo.mp4").exists());
        assert_eq!(r.errors.len(), 3, "{:?}", r.errors);
        let r = trash_files(&picks, false);
        assert!(!hm.join("Desktop/demo.mp4").exists());
        assert!(hm.join(".Trash/demo.mp4").exists());
        assert!(hm.join("Documents/keep/big.iso").exists());
        assert!(r.entry.bytes >= 3 * mb);
        let pb = super::super::history::put_back(&r.entry.id, None);
        assert_eq!(pb.restored, vec![hm.join("Desktop/demo.mp4")]);
    }
}
