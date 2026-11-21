//! Build outputs inside your projects (`node_modules`, `target`, `build`…): the parallel walk,
//! matching against the catalog's project rules, and the `git check-ignore` filter.

use super::catalog::Rule;
use super::*;

pub(crate) const NEVER_DESCEND: &[&str] = &[".git", ".hg", ".svn", "Library", ".Trash"];

#[derive(Clone, Debug)]
pub struct Artifact {
    pub path: PathBuf,
    pub project: PathBuf,
    pub kind: &'static str,
    pub redownload: bool,
    pub bytes: u64,
    pub mtime: SystemTime,
}

/// The first catalog rule for a folder called `dir_name` whose parent holds the files in `parent`.
pub(crate) fn match_rule(dir_name: &str, parent: &HashSet<String>) -> Option<&'static Rule> {
    catalog().rules.iter().find(|r| r.matches(dir_name, parent))
}

/// Parallel directory walk with a shared work queue. Never follows symlinks, never leaves the volume
/// a root lives on, and never opens iCloud placeholders (that would download them).
pub(crate) fn walk_roots(roots: &[PathBuf], cancel: &AtomicBool) -> Vec<(PathBuf, &'static Rule)> {
    use std::os::unix::fs::MetadataExt;
    // each queued folder carries the device of its root: a mount point inside a project is never entered
    let queue: Mutex<Vec<(PathBuf, usize, u64)>> =
        Mutex::new(roots.iter().filter_map(|r| Some((r.clone(), 0, std::fs::metadata(r).ok()?.dev()))).collect());
    let busy = AtomicUsize::new(0);
    let found = Mutex::new(vec![]);
    let workers = walk_workers();
    std::thread::scope(|s| {
        for _ in 0..workers {
            s.spawn(|| loop {
                if cancel.load(Ordering::Relaxed) {
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
                let Some((dir, depth, dev)) = item else {
                    if busy.load(Ordering::SeqCst) == 0 {
                        break;
                    }
                    std::thread::sleep(Duration::from_millis(2));
                    continue;
                };
                if depth <= 12 {
                    if let Ok(rd) = std::fs::read_dir(&dir) {
                        let entries: Vec<_> = rd.filter_map(|e| e.ok()).collect();
                        let names: HashSet<String> = entries.iter().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
                        let mut sub = vec![];
                        for e in &entries {
                            let Ok(ft) = e.file_type() else { continue };
                            if !ft.is_dir() || ft.is_symlink() {
                                continue;
                            }
                            let name = e.file_name().to_string_lossy().into_owned();
                            if NEVER_DESCEND.contains(&name.as_str()) {
                                continue;
                            }
                            match e.metadata() {
                                Ok(m) if m.dev() == dev && !is_dataless(&m) => {}
                                _ => continue,
                            }
                            if let Some(rule) = match_rule(&name, &names) {
                                found.lock().unwrap().push((e.path(), rule));
                            } else if name != "node_modules" {
                                sub.push((e.path(), depth + 1, dev));
                            }
                        }
                        queue.lock().unwrap().extend(sub);
                    }
                }
                busy.fetch_sub(1, Ordering::SeqCst);
            });
        }
    });
    found.into_inner().unwrap()
}

/// Inside a git repo, keep only candidates git ignores (a tracked `build/` is source code).
pub(crate) fn filter_git_ignored(candidates: &[PathBuf]) -> HashSet<PathBuf> {
    let mut keep = HashSet::new();
    let mut by_repo: HashMap<PathBuf, Vec<PathBuf>> = HashMap::new();
    let mut root_cache: HashMap<PathBuf, Option<PathBuf>> = HashMap::new();
    for c in candidates {
        let parent = c.parent().unwrap_or(c).to_path_buf();
        let root = root_cache
            .entry(parent.clone())
            .or_insert_with(|| {
                let r = run_s("git", &["-C", &parent.to_string_lossy(), "rev-parse", "--show-toplevel"], 30);
                r.ok.then(|| PathBuf::from(r.stdout.trim()))
            })
            .clone();
        match root {
            None => {
                keep.insert(c.clone());
            }
            Some(r) => by_repo.entry(r).or_default().push(c.clone()),
        }
    }
    for (root, paths) in by_repo {
        let input: String = paths.iter().map(|p| format!("{}/\n", p.to_string_lossy())).collect();
        let args = vec!["-C".to_string(), root.to_string_lossy().into_owned(), "check-ignore".into(), "--stdin".into()];
        let r = run("git", &args, Duration::from_secs(120), Some(&input));
        for line in r.stdout.lines() {
            let p = line.trim().trim_end_matches('/');
            if p.is_empty() {
                continue;
            }
            let pb = PathBuf::from(p);
            keep.insert(if pb.is_absolute() { pb } else { root.join(pb) });
        }
    }
    keep
}

/// Project artifacts under `roots`, minus anything on the ignore list.
pub fn scan_projects(roots: &[PathBuf], on_event: &(dyn Fn(ScanEvent) + Sync), cancel: &AtomicBool, ignore: &IgnoreList) -> Vec<Artifact> {
    on_event(ScanEvent::Status("Walking your projects…".into()));
    let mut found = walk_roots(roots, cancel);
    found.retain(|(p, _)| !ignore.blocks_path(p));
    // drop candidates nested inside another candidate (e.g. build/ inside .dart_tool/)
    found.sort_by_key(|(p, _)| p.as_os_str().len());
    let mut kept: Vec<(PathBuf, &Rule)> = vec![];
    for (p, r) in found {
        if !kept.iter().any(|(k, _)| p.starts_with(k) && p != *k) {
            kept.push((p, r));
        }
    }
    on_event(ScanEvent::Status(format!("Asking git about {} folders…", kept.len())));
    let ignored = filter_git_ignored(&kept.iter().map(|(p, _)| p.clone()).collect::<Vec<_>>());
    let safe: Vec<_> = kept.into_iter().filter(|(p, _)| ignored.contains(p)).collect();
    on_event(ScanEvent::Status(format!("Measuring {} folders…", safe.len())));
    let sizes = size_of(&safe.iter().map(|(p, _)| p.clone()).collect::<Vec<_>>());
    let mut out: Vec<Artifact> = safe
        .into_iter()
        .map(|(p, r)| Artifact {
            bytes: *sizes.get(&p).unwrap_or(&0),
            // a project counts as touched when its artifact or the project folder itself changed
            mtime: mtime(&p).max(p.parent().map(mtime).unwrap_or(UNIX_EPOCH)),
            project: p.parent().unwrap_or(&p).to_path_buf(),
            kind: &r.kind,
            redownload: r.redownload,
            path: p,
        })
        .filter(|a| a.bytes > 0)
        .collect();
    out.sort_by(|a, b| b.bytes.cmp(&a.bytes));
    out
}

/// One selectable target per project (and per re-download kind).
pub fn artifacts_to_targets(artifacts: &[Artifact], roots: &[PathBuf], keep_recent_days: u64) -> Vec<Target> {
    let cutoff = SystemTime::now() - Duration::from_secs(keep_recent_days * 86400);
    let mut by_key: Vec<(String, String, Group, Vec<&Artifact>)> = vec![];
    for a in artifacts {
        if keep_recent_days > 0 && a.mtime > cutoff {
            continue;
        }
        let (key, label, group) = if a.redownload {
            (format!("re:{}", a.kind), format!("{} in projects", a.kind), Group::Redownload)
        } else {
            let root = roots.iter().find(|r| a.path.starts_with(r)).cloned().unwrap_or_else(|| a.project.parent().unwrap_or(&a.project).to_path_buf());
            let rel: Vec<String> = a.project.strip_prefix(&root).unwrap_or(&a.project).components().take(2).map(|c| c.as_os_str().to_string_lossy().into_owned()).collect();
            let rel = rel.join("/");
            let label = if root == h("Documents") {
                if rel.is_empty() { "Documents".into() } else { rel.clone() }
            } else {
                format!("{}/{}", tildify(&root), rel)
            };
            (format!("proj:{}/{}", root.to_string_lossy(), rel), label, Group::Build)
        };
        match by_key.iter_mut().find(|e| e.0 == key) {
            Some(e) => e.3.push(a),
            None => by_key.push((key, label, group, vec![a])),
        }
    }
    by_key
        .into_iter()
        .map(|(id, label, group, items)| Target {
            id,
            label,
            group,
            default_on: group == Group::Build,
            bytes: items.iter().map(|i| i.bytes).sum(),
            paths: items.iter().map(|i| i.path.clone()).collect(),
            commands: vec![],
            detail: Some(if items.len() == 1 { "1 folder".into() } else { format!("{} folders", items.len()) }),
            guard: Guard::None,
            wipes_data: false,
        })
        .collect()
}

pub fn default_roots() -> Vec<PathBuf> {
    ["Documents", ".t3", ".codex/worktrees", ".claude/worktrees", "conductor/workspaces", "esp", "Developer", "Projects"]
        .iter()
        .map(|d| h(d))
        .filter(|p| exists(p))
        .collect()
}
