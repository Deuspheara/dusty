//! Cleaning: the path safety checks, running-app guards, and removing (or trashing) each path.

use super::history::PathAction;
use super::*;

/// Path validation adapted from Mole's `validate_path_for_deletion`: absolute, no control characters,
/// no `..` components, inside $HOME but never a top-level folder or personal-data folder, and the
/// same verdict for the real location after resolving symlinked ancestors (deny-only).
pub fn assert_safe(target: &Path) -> Result<(), String> {
    assert_safe_allowing(target, &[])
}

/// Folders a person may still pick single files from by hand (large-file review): automatic sweeps
/// never go there, but a file you chose yourself can go to the Trash.
pub const USER_PICK_ALLOW: &[&str] = &["Desktop", "Movies", "Music", "Pictures"];
/// Preference and cookie files an app uninstall may move to the Trash (matched by bundle id).
pub const UNINSTALL_ALLOW: &[&str] = &["Library/Preferences", "Library/Cookies"];

/// [`assert_safe`] with some personal-data folders exempted (`allow` lists entries of the protected
/// list, home-relative, such as [`USER_PICK_ALLOW`]). Every other rule still applies: top-level folders
/// themselves, Keychains, Mail, iCloud, Photos libraries, other volumes, symlinked ancestors.
pub fn assert_safe_allowing(target: &Path, allow: &[&str]) -> Result<(), String> {
    let hm = home();
    let bad = |why: &str| Err(format!("Refusing to delete {} ({why})", tildify(target)));
    if !target.is_absolute() {
        return bad("not an absolute path");
    }
    if target.to_string_lossy().chars().any(|c| c.is_control()) {
        return bad("control characters in path");
    }
    if target.components().any(|c| matches!(c, std::path::Component::ParentDir)) {
        return bad("'..' in path");
    }
    const TOP: &[&str] = &[
        "Documents", "Desktop", "Downloads", "Library", "Library/Caches", "Library/Application Support", "Library/Containers", "Library/Group Containers",
        "Library/Logs", "Library/Preferences", "Pictures", "Movies", "Music", "Public", "Applications", ".Trash", ".config", ".cache", ".local",
    ];
    let verdict = |p: &Path| -> Option<&'static str> {
        if !p.starts_with(&hm) || p == hm {
            return Some("outside your home folder");
        }
        if TOP.iter().any(|t| p == h(t)) {
            return Some("a top-level folder");
        }
        if protected_path_except(p, allow) {
            return Some("personal data");
        }
        None
    };
    if let Some(why) = verdict(target) {
        return bad(why);
    }
    if let Ok(m) = std::fs::symlink_metadata(target) {
        use std::os::unix::fs::MetadataExt;
        if std::fs::metadata(&hm).is_ok_and(|hmeta| hmeta.dev() != m.dev()) {
            return bad("on another volume");
        }
        if is_dataless(&m) {
            return bad("stored in iCloud, not on this Mac");
        }
    }
    // a symlinked ancestor could redirect the delete somewhere else: check where it really points
    if let (Some(parent), Some(name)) = (target.parent(), target.file_name()) {
        if let Ok(real_parent) = std::fs::canonicalize(parent) {
            let real_home = std::fs::canonicalize(&hm).unwrap_or_else(|_| hm.clone());
            match real_parent.join(name).strip_prefix(&real_home) {
                Ok(rel) => {
                    if let Some(why) = verdict(&hm.join(rel)) {
                        return bad(why);
                    }
                }
                Err(_) => return bad("resolves outside your home folder"),
            }
        }
    }
    Ok(())
}

pub(crate) fn app_running(pattern: &str, cache: &mut HashMap<String, bool>) -> bool {
    *cache.entry(pattern.to_string()).or_insert_with(|| run_s("/usr/bin/pgrep", &["-if", pattern], 10).ok)
}

/// The guard that applies to one path of a target, as (display name, process regex).
pub(crate) fn guard_for(t: &Target, p: &Path) -> Option<(String, String)> {
    match &t.guard {
        Guard::None => None,
        Guard::App(name, pat) => Some((name.to_string(), pat.to_string())),
        Guard::PerAppFolder => {
            let rel = p.strip_prefix(h("Library/Application Support")).ok()?;
            let app = rel.components().next()?.as_os_str().to_string_lossy().into_owned();
            let esc: String = app.chars().map(|c| if c.is_alphanumeric() || c == ' ' { c.to_string() } else { format!("\\{c}") }).collect();
            Some((app, format!("/{esc}\\.app/Contents/MacOS/")))
        }
    }
}

/// Why a path is left alone.
enum Kept {
    /// The safety rules or the ignore list said no: reported as an error.
    Refused(String),
    /// Its app is running: reported once per app, as "quit it and run again".
    AppOpen(String),
}

/// The checks every path passes right before it goes, in order; the first one that objects wins.
fn check_path(t: &Target, p: &Path, ignore: &IgnoreList, running: &mut HashMap<String, bool>) -> Result<(), Kept> {
    assert_safe(p).map_err(Kept::Refused)?;
    // the ignore list is checked on the path as given and where it really is (symlinked folders)
    let real = p.parent().and_then(|d| std::fs::canonicalize(d).ok()).zip(p.file_name()).map(|(d, n)| d.join(n));
    if ignore.blocks_path(p) || real.is_some_and(|r| ignore.blocks_path(&r)) {
        return Err(Kept::Refused(ignore_refusal(&tildify(p))));
    }
    if let Some((app, pattern)) = guard_for(t, p) {
        if app_running(&pattern, running) {
            return Err(Kept::AppOpen(app));
        }
    }
    Ok(())
}

/// What happened to one path of a target during a clean (reported through `on_path`).
#[derive(Clone, Debug, PartialEq)]
pub enum PathOutcome {
    /// Removed for good (`rm -rfx`).
    Deleted(PathBuf),
    /// Moved to the Trash; `to` is where it now lives (used by "put back").
    Trashed { from: PathBuf, to: PathBuf },
    /// Practice run: would have been removed this way.
    Practice(PathBuf, Removal),
    /// Left alone: refused by the safety rules, the ignore list, a running app, or the removal failed.
    Skipped(PathBuf, String),
}

/// Cleans one target with a removal mode (delete or move to the Trash), reporting each path through
/// `on_path`. Returns human-readable errors (in use / protected), never panics.
/// Commands (Docker prune, `brew cleanup`, `simctl delete`…) always run as-is: they can't be trashed.
pub fn clean_target_with(t: &Target, dry_run: bool, removal: Removal, on_path: &mut dyn FnMut(PathOutcome)) -> Vec<String> {
    let dry_run = force_dry(dry_run);
    let mut errors = vec![];
    let mut running = HashMap::new();
    let mut skipped_apps: Vec<String> = vec![];
    let ignore = IgnoreList::load();
    if let Some(why) = &ignore.broken {
        errors.push(format!("Nothing was deleted: {why}."));
        return errors;
    }
    for c in &t.commands {
        if dry_run {
            continue;
        }
        let r = run(&c.cmd, &c.args, Duration::from_secs(600), None);
        // pkill returns 1 when nothing matched: not an error
        if !r.ok && c.cmd != "pkill" {
            errors.push(format!("{} {}: {}", c.cmd, c.args.join(" "), r.stderr.lines().next().unwrap_or("failed")));
        }
    }
    for p in &t.paths {
        match check_path(t, p, &ignore, &mut running) {
            Ok(()) => {}
            Err(Kept::Refused(e)) => {
                on_path(PathOutcome::Skipped(p.clone(), e.clone()));
                errors.push(e);
                continue;
            }
            Err(Kept::AppOpen(app)) => {
                on_path(PathOutcome::Skipped(p.clone(), format!("{app} is open")));
                if !skipped_apps.contains(&app) {
                    skipped_apps.push(app);
                }
                continue;
            }
        }
        if dry_run {
            on_path(PathOutcome::Practice(p.clone(), removal));
            continue;
        }
        match removal {
            Removal::Trash => match trash::move_to_trash(p) {
                Ok(to) => on_path(PathOutcome::Trashed { from: p.clone(), to }),
                Err(e) => {
                    let e = format!("{}: {e}", tildify(p));
                    on_path(PathOutcome::Skipped(p.clone(), e.clone()));
                    errors.push(e);
                }
            },
            Removal::Delete => match remove_path(p) {
                Ok(()) => on_path(PathOutcome::Deleted(p.clone())),
                Err(e) => {
                    let e = format!("{}: {e}", tildify(p));
                    on_path(PathOutcome::Skipped(p.clone(), e.clone()));
                    errors.push(e);
                }
            },
        }
    }
    for app in skipped_apps {
        errors.push(format!("{app} is open, so its cache was left alone. Quit it and run Dusty again."));
    }
    errors
}

/// `rm -rfx` one already-validated path, making read-only caches writable first if needed.
pub(crate) fn remove_path(p: &Path) -> Result<(), String> {
    // -x: never cross into a volume mounted somewhere inside the folder
    let args = vec!["-rfx".to_string(), "--".into(), p.to_string_lossy().into_owned()];
    let mut r = run("/bin/rm", &args, Duration::from_secs(1800), None);
    if !r.ok && r.stderr.contains("Permission denied") {
        // read-only caches (Go modules, some package stores): make our own files writable, then retry.
        // -P: never follow a symlink out of the folder
        let chmod = vec!["-R".to_string(), "-P".into(), "u+w".into(), "--".into(), p.to_string_lossy().into_owned()];
        run("/bin/chmod", &chmod, Duration::from_secs(600), None);
        r = run("/bin/rm", &args, Duration::from_secs(1800), None);
    }
    if r.ok { Ok(()) } else { Err(r.stderr.lines().next().unwrap_or("could not remove").to_string()) }
}

/// Moves `p` to the Trash if `check` passed (only pretends in a practice run). Returns what
/// happened, where it went, and the error when it was left alone.
pub(crate) fn trash_checked(p: &Path, check: Result<(), String>, dry: bool) -> (PathAction, Option<PathBuf>, Option<String>) {
    match check {
        Err(e) => (PathAction::Skipped, None, Some(e)),
        Ok(()) if dry => (PathAction::Practice, None, None),
        Ok(()) => match trash::move_to_trash(p) {
            Ok(to) => (PathAction::Trashed, Some(to), None),
            Err(e) => (PathAction::Skipped, None, Some(format!("{}: {e}", tildify(p)))),
        },
    }
}
