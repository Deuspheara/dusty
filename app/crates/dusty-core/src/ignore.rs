//! Your ignore list (`~/.config/dusty/ignore`): parsing, matching, and filtering scan results.

use super::*;

/// `~/.config/dusty/ignore`: things Dusty must never delete, one per line (`#` starts a comment).
///
/// - `~/Documents/app/build` or `/abs/path`: that path, everything inside it, and any folder that
///   contains it (deleting the parent would take it along). `*` matches within one folder name,
///   `**` any number of folders. Matching ignores case, like APFS.
/// - `node_modules` (a bare name, no `/`): any file or folder with that name, and what's inside it.
/// - `id:docker-images` / `id:proj:*`: hide a whole item from the results (also commands like Docker).
///
/// Read by both the app and the CLI. If the file exists but can't be read, nothing is deleted.
#[derive(Clone, Debug, Default)]
pub struct IgnoreList {
    pub(crate) paths: Vec<Vec<String>>,
    ids: Vec<String>,
    /// Set when the file exists but couldn't be read: every delete is refused.
    pub broken: Option<String>,
}

pub fn ignore_file() -> PathBuf {
    h(".config/dusty/ignore")
}

impl IgnoreList {
    pub fn load() -> Self {
        let f = ignore_file();
        match std::fs::read_to_string(&f) {
            Ok(text) => Self::parse(&text),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Self::default(),
            Err(e) => Self { broken: Some(format!("couldn't read {} ({e})", tildify(&f))), ..Self::default() },
        }
    }

    pub fn parse(text: &str) -> Self {
        let mut out = Self::default();
        for line in text.lines() {
            let line = line.split_once('#').map_or(line, |(a, _)| a).trim();
            if line.is_empty() {
                continue;
            }
            if let Some(id) = line.strip_prefix("id:") {
                out.ids.push(id.trim().to_string());
                continue;
            }
            let full = if line == "~" {
                home()
            } else if let Some(rest) = line.strip_prefix("~/") {
                home().join(rest)
            } else if line.starts_with('/') {
                PathBuf::from(line)
            } else if !line.contains('/') {
                // bare name: any component anywhere
                out.paths.push(vec!["**".into(), line.to_string()]);
                continue;
            } else {
                home().join(line)
            };
            out.paths.push(components(&full));
        }
        out
    }

    pub fn blocks_target(&self, id: &str) -> bool {
        self.ids.iter().any(|p| glob_match(p, id))
    }

    /// True when deleting `p` would delete something on the list.
    pub fn blocks_path(&self, p: &Path) -> bool {
        if self.paths.is_empty() {
            return false;
        }
        let path = components(p);
        let path: Vec<&str> = path.iter().map(String::as_str).collect();
        self.paths.iter().any(|pat| {
            let pat: Vec<&str> = pat.iter().map(String::as_str).collect();
            overlaps(&pat, &path, false)
        })
    }
}

/// "Left … alone: it's on your ignore list (~/.config/dusty/ignore)."
pub(crate) fn ignore_refusal(what: &str) -> String {
    format!("Left {what} alone: it's on your ignore list ({}).", tildify(&ignore_file()))
}

pub(crate) fn components(p: &Path) -> Vec<String> {
    p.components().filter_map(|c| match c {
        std::path::Component::Normal(s) => Some(s.to_string_lossy().into_owned()),
        _ => None,
    }).collect()
}

/// Does deleting `path` touch something `pat` matches? Either `pat` matches `path` or one of its
/// ancestors (it's inside a protected folder), or `path` is an ancestor of a possible match (the
/// delete would take it along). After a `**`, only the first two count: "contains something named X
/// somewhere below" would block every folder.
pub(crate) fn overlaps(pat: &[&str], path: &[&str], after_globstar: bool) -> bool {
    match (pat.first(), path.first()) {
        (None, _) => true,
        (Some(&"**"), _) => overlaps(&pat[1..], path, true) || (!path.is_empty() && overlaps(pat, &path[1..], true)),
        (Some(_), None) => !after_globstar,
        (Some(seg), Some(name)) => glob_match(seg, name) && overlaps(&pat[1..], &path[1..], after_globstar),
    }
}

/// Removes ignored targets and paths from scan results. A target that lost paths is measured again.
pub(crate) fn apply_ignore(targets: Vec<Target>, ignore: &IgnoreList) -> Vec<Target> {
    targets
        .into_iter()
        .filter(|t| !ignore.blocks_target(&t.id))
        .filter_map(|mut t| {
            let before = t.paths.len();
            t.paths.retain(|p| !ignore.blocks_path(p));
            if t.paths.len() == before {
                return Some(t);
            }
            if t.paths.is_empty() && t.commands.is_empty() {
                return None;
            }
            t.bytes = total_size(&t.paths);
            Some(t)
        })
        .collect()
}
