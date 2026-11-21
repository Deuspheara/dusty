//! Move to Trash, and put back.
//!
//! Order of preference: `NSFileManager trashItemAtURL:` (what Finder uses: right Trash for the volume,
//! Finder-style unique names, and it tells us where the item landed, which "put back" needs), then a
//! plain `rename` into `~/.Trash` with a unique name (same volume only, never a copy).
//! Test builds only use the rename into `$HOME/.Trash`, so tests never touch the real Trash.

use super::{exists, home, tildify, Group, Target};
use std::path::{Component, Path, PathBuf};

/// How a single path is removed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Removal {
    /// `rm -rfx`: the space comes back right away.
    Delete,
    /// Moved to the Trash: recoverable, but the space only comes back when the Trash is emptied.
    Trash,
}

impl Removal {
    pub fn key(self) -> &'static str {
        match self {
            Removal::Delete => "delete",
            Removal::Trash => "trash",
        }
    }
}

/// Which items of a clean go to the Trash.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum CleanPolicy {
    /// Everything is deleted outright (Dusty's classic behaviour).
    DeleteAll,
    /// Everything that is a file or folder goes to the Trash.
    TrashAll,
    /// Pure caches and build output (rebuilt on their own) are deleted, so the space really comes
    /// back; logs, leftovers and anything re-downloadable go to the Trash. The default.
    #[default]
    TrashUnlessCache,
}

impl CleanPolicy {
    /// The removal used for one target under this policy.
    pub fn removal_for(self, t: &Target) -> Removal {
        match self {
            CleanPolicy::DeleteAll => Removal::Delete,
            CleanPolicy::TrashAll => Removal::Trash,
            CleanPolicy::TrashUnlessCache => match t.group {
                Group::Build | Group::Cache | Group::Apps | Group::Docker => Removal::Delete,
                Group::Junk | Group::Redownload => Removal::Trash,
            },
        }
    }
    pub fn key(self) -> &'static str {
        match self {
            CleanPolicy::DeleteAll => "delete",
            CleanPolicy::TrashAll => "trash",
            CleanPolicy::TrashUnlessCache => "trash-unless-cache",
        }
    }
    pub fn from_key(k: &str) -> Option<Self> {
        [CleanPolicy::DeleteAll, CleanPolicy::TrashAll, CleanPolicy::TrashUnlessCache].into_iter().find(|p| p.key() == k)
    }
}

/// Moves `p` to the Trash and returns where it now lives. The caller validates `p` first
/// (`assert_safe`, ignore list): this function only moves, it never decides what is safe.
pub fn move_to_trash(p: &Path) -> Result<PathBuf, String> {
    if !exists(p) {
        return Err("it's already gone".into());
    }
    #[cfg(all(target_os = "macos", not(test)))]
    {
        match ns_trash(p) {
            Ok(to) => return Ok(to),
            Err(ns_err) => return rename_into_trash(p).map_err(|e| format!("{ns_err}; {e}")),
        }
    }
    #[allow(unreachable_code)]
    rename_into_trash(p)
}

/// The user's Trash folder (`~/.Trash`).
pub fn user_trash() -> PathBuf {
    home().join(".Trash")
}

/// Fallback: `rename` into `~/.Trash` under a Finder-style unique name ("report 2.pdf").
/// Fails (without copying) if the item is on another volume.
pub(crate) fn rename_into_trash(p: &Path) -> Result<PathBuf, String> {
    let trash = user_trash();
    std::fs::create_dir_all(&trash).map_err(|e| format!("couldn't open the Trash ({e})"))?;
    let name = p.file_name().ok_or("no file name")?.to_string_lossy().into_owned();
    let dest = unique_in(&trash, &name);
    std::fs::rename(p, &dest).map_err(|e| {
        if e.raw_os_error() == Some(libc::EXDEV) {
            "it's on another disk, so it can't go to this Trash".to_string()
        } else {
            format!("couldn't move it to the Trash ({e})")
        }
    })?;
    Ok(dest)
}

/// `dir/name`, or `dir/stem 2.ext`, `dir/stem 3.ext`… whichever is free.
pub(crate) fn unique_in(dir: &Path, name: &str) -> PathBuf {
    let first = dir.join(name);
    if !exists(&first) {
        return first;
    }
    // "archive.tar.gz" keeps ".tar.gz"? Finder keeps only the last extension; so do we. Folders and
    // dotfiles keep the whole name as the stem.
    let (stem, ext) = match name.rfind('.') {
        Some(i) if i > 0 => (&name[..i], &name[i..]),
        _ => (name, ""),
    };
    (2..)
        .map(|n| dir.join(format!("{stem} {n}{ext}")))
        .find(|c| !exists(c))
        .unwrap()
}

#[cfg(target_os = "macos")]
pub(crate) fn ns_trash(p: &Path) -> Result<PathBuf, String> {
    use objc::runtime::{Class, Object, BOOL, NO};
    use objc::{msg_send, sel, sel_impl};
    use std::os::unix::ffi::OsStrExt;
    let c = std::ffi::CString::new(p.as_os_str().as_bytes()).map_err(|_| "bad path")?;
    let (Some(ns_string), Some(ns_url), Some(ns_fm)) = (Class::get("NSString"), Class::get("NSURL"), Class::get("NSFileManager")) else {
        return Err("Foundation isn't available".into());
    };
    objc::rc::autoreleasepool(|| unsafe {
        let s: *mut Object = msg_send![ns_string, stringWithUTF8String: c.as_ptr()];
        if s.is_null() {
            return Err("bad path".to_string());
        }
        let url: *mut Object = msg_send![ns_url, fileURLWithPath: s];
        // a fresh manager per call: safe to use from any background thread
        let fm: *mut Object = msg_send![ns_fm, new];
        let mut out: *mut Object = std::ptr::null_mut();
        let mut err: *mut Object = std::ptr::null_mut();
        let ok: BOOL = msg_send![fm, trashItemAtURL: url resultingItemURL: &mut out error: &mut err];
        let _: () = msg_send![fm, release];
        if ok == NO {
            let why = if err.is_null() { "the Trash said no".to_string() } else { ns_to_string(msg_send![err, localizedDescription]) };
            return Err(why);
        }
        if out.is_null() {
            return Err("moved to the Trash, but macOS didn't say where".into());
        }
        let path: *mut Object = msg_send![out, path];
        Ok(PathBuf::from(ns_to_string(path)))
    })
}

#[cfg(target_os = "macos")]
unsafe fn ns_to_string(s: *mut objc::runtime::Object) -> String {
    use objc::{msg_send, sel, sel_impl};
    if s.is_null() {
        return String::new();
    }
    let p: *const std::os::raw::c_char = msg_send![s, UTF8String];
    if p.is_null() { String::new() } else { std::ffi::CStr::from_ptr(p).to_string_lossy().into_owned() }
}

/// Moves a trashed item back to `original`. Refuses if something already lives there, if the
/// trashed item isn't inside a Trash folder, or if `original` isn't somewhere Dusty cleans
/// (your home folder, or an app folder for uninstalled apps). Missing parent folders inside your
/// home are recreated.
pub fn put_back_item(trashed: &Path, original: &Path) -> Result<(), String> {
    let plain = |p: &Path| p.is_absolute() && !p.components().any(|c| matches!(c, Component::ParentDir)) && !p.to_string_lossy().chars().any(|c| c.is_control());
    if !plain(trashed) || !plain(original) {
        return Err("not a plain absolute path".into());
    }
    let in_trash = trashed.components().any(|c| matches!(c, Component::Normal(n) if n == ".Trash" || n == ".Trashes"));
    if !in_trash {
        return Err(format!("{} isn't in the Trash", tildify(trashed)));
    }
    if !exists(trashed) {
        return Err(format!("{} is no longer in the Trash (emptied?)", tildify(trashed)));
    }
    if exists(original) {
        return Err(format!("something new already lives at {}", tildify(original)));
    }
    let hm = home();
    let app_dest = original.extension().is_some_and(|e| e == "app")
        && (original.starts_with("/Applications") || original.starts_with(hm.join("Applications")));
    if !original.starts_with(&hm) && !app_dest {
        return Err(format!("{} is outside your home folder", tildify(original)));
    }
    let parent = original.parent().ok_or("no parent folder")?;
    // folders we had to recreate, deepest first, so a failed move can tidy them away again
    let mut created: Vec<PathBuf> = Vec::new();
    if !parent.exists() {
        if parent.starts_with(&hm) && parent != hm {
            created = parent.ancestors().take_while(|a| !a.exists()).map(Path::to_path_buf).collect();
            std::fs::create_dir_all(parent).map_err(|e| put_back_error(&e, &format!("couldn't recreate {}", tildify(parent))))?;
        } else {
            return Err(format!("{} doesn't exist any more", tildify(parent)));
        }
    }
    std::fs::rename(trashed, original).map_err(|e| {
        for dir in &created {
            // remove_dir only removes empty folders, so nothing else can be lost here
            let _ = std::fs::remove_dir(dir);
        }
        put_back_error(&e, &format!("couldn't put {} back", tildify(original)))
    })
}

/// Without Full Disk Access, macOS refuses to let apps take things out of the Trash: say what to do
/// instead of passing on "Operation not permitted".
fn put_back_error(e: &std::io::Error, what: &str) -> String {
    if e.kind() == std::io::ErrorKind::PermissionDenied {
        "macOS keeps the Trash private. Drag it back in Finder, or give Dusty Full Disk Access.".into()
    } else {
        format!("{what} ({e})")
    }
}

#[cfg(test)]
mod tests {
    use super::super::tests::{touch, FakeHome};
    use super::*;

    #[test]
    fn trash_and_put_back_in_a_fake_home() {
        let fake = FakeHome::new("trash");
        let f = fake.path.join("Downloads/report.pdf");
        touch(&f, 1000);
        let to = move_to_trash(&f).unwrap();
        assert_eq!(to, fake.path.join(".Trash/report.pdf"));
        assert!(!f.exists());
        // a second item with the same name gets a Finder-style unique name
        touch(&f, 10);
        let to2 = move_to_trash(&f).unwrap();
        assert_eq!(to2, fake.path.join(".Trash/report 2.pdf"));
        // put back refuses while something lives at the original path
        touch(&f, 5);
        assert!(put_back_item(&to, &f).unwrap_err().contains("already lives"));
        std::fs::remove_file(&f).unwrap();
        put_back_item(&to, &f).unwrap();
        assert_eq!(std::fs::metadata(&f).unwrap().len(), 1000);
        // recreates a missing parent inside home
        let deep = fake.path.join("gone/sub/report.pdf");
        put_back_item(&to2, &deep).unwrap();
        assert!(deep.exists());
        // never outside home / never from outside the Trash
        assert!(put_back_item(&deep, &fake.path.join("x")).unwrap_err().contains("isn't in the Trash"));
        assert!(move_to_trash(&fake.path.join("missing")).is_err());
    }

    #[test]
    fn unique_names() {
        let fake = FakeHome::new("unique");
        let d = fake.path.join("t");
        touch(&d.join("a.tar.gz"), 1);
        touch(&d.join(".env"), 1);
        touch(&d.join("Folder/x"), 1);
        assert_eq!(unique_in(&d, "a.tar.gz"), d.join("a.tar 2.gz"));
        assert_eq!(unique_in(&d, ".env"), d.join(".env 2"));
        assert_eq!(unique_in(&d, "Folder"), d.join("Folder 2"));
        assert_eq!(unique_in(&d, "new"), d.join("new"));
    }

    #[test]
    fn policies() {
        let t = |g| Target { id: "x".into(), label: "x".into(), group: g, default_on: true, bytes: 0, paths: vec![], commands: vec![], detail: None, guard: super::super::Guard::None, wipes_data: false };
        assert_eq!(CleanPolicy::TrashUnlessCache.removal_for(&t(Group::Cache)), Removal::Delete);
        assert_eq!(CleanPolicy::TrashUnlessCache.removal_for(&t(Group::Junk)), Removal::Trash);
        assert_eq!(CleanPolicy::TrashAll.removal_for(&t(Group::Build)), Removal::Trash);
        assert_eq!(CleanPolicy::DeleteAll.removal_for(&t(Group::Redownload)), Removal::Delete);
        assert_eq!(CleanPolicy::from_key("trash-unless-cache"), Some(CleanPolicy::TrashUnlessCache));
    }

    /// The real NSFileManager path, on a scratch file only. Run by hand:
    /// `cargo test ns_trash_scratch -- --ignored` (it lands in your Trash, then is put back and removed).
    #[test]
    #[ignore]
    fn ns_trash_scratch() {
        let dir = std::env::temp_dir().join(format!("dusty-ns-trash-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("dusty-scratch.txt");
        std::fs::write(&f, b"scratch").unwrap();
        let to = ns_trash(&f).unwrap();
        assert!(to.exists() && !f.exists(), "landed at {}", to.display());
        std::fs::rename(&to, &f).unwrap();
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
