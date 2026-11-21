//! Editing `~/.config/dusty/ignore` from the app. Comments, blank lines and the order of everything
//! else are kept; writes are atomic (temp file + rename), so a crash never leaves a half-written list
//! (which would make every clean refuse).

use super::{ignore_file, tildify, write_atomic};
use std::path::PathBuf;

/// The entry part of a line (before `#`), trimmed.
fn entry_of(line: &str) -> &str {
    line.split_once('#').map_or(line, |(a, _)| a).trim()
}

fn read_existing() -> Result<String, String> {
    let f = ignore_file();
    match std::fs::read_to_string(&f) {
        Ok(t) => Ok(t),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(String::new()),
        Err(e) => Err(format!("couldn't read {} ({e})", tildify(&f))),
    }
}

fn save(text: &str) -> Result<(), String> {
    write_atomic(&ignore_file(), text.as_bytes())
}

fn validate(line: &str) -> Result<&str, String> {
    let l = line.trim();
    if l.is_empty() {
        return Err("Nothing to add.".into());
    }
    if l.chars().any(|c| c.is_control()) {
        return Err("An ignore entry is one line, without control characters.".into());
    }
    if l.contains('#') {
        return Err("`#` starts a comment in the ignore list, so it can't be part of an entry.".into());
    }
    Ok(l)
}

/// One ignore-list line, read for display. The line format stays in this crate.
#[derive(Clone, Debug, PartialEq)]
pub enum IgnoreEntry {
    /// A whole scan item (`id:<target id>`, globs allowed).
    Item(String),
    /// One project folder's build output (`id:proj:<path>`).
    Project(PathBuf),
    /// A path or a bare name.
    Path(String),
}

impl IgnoreEntry {
    pub fn parse(line: &str) -> Self {
        match line.strip_prefix("id:") {
            Some(id) => match id.strip_prefix("proj:") {
                Some(p) => IgnoreEntry::Project(PathBuf::from(p)),
                None => IgnoreEntry::Item(id.to_string()),
            },
            None => IgnoreEntry::Path(line.to_string()),
        }
    }

    /// The line that hides scan item `id` from the results.
    pub fn line_for_item(id: &str) -> String {
        format!("id:{id}")
    }
}

/// Entries currently on the list (comments and blank lines left out), in file order.
pub fn ignore_entries() -> Result<Vec<String>, String> {
    Ok(read_existing()?.lines().map(entry_of).filter(|l| !l.is_empty()).map(String::from).collect())
}

/// Adds one entry (a path like `~/Documents/x`, a bare name, or `id:<target id>`). Returns `false`
/// if it was already there. Creates `~/.config/dusty/` when needed.
pub fn add_to_ignore(line: &str) -> Result<bool, String> {
    let l = validate(line)?;
    let mut text = read_existing()?;
    if text.lines().any(|x| entry_of(x) == l) {
        return Ok(false);
    }
    if text.is_empty() {
        text.push_str("# Things Dusty must never touch. One per line; see the README for the format.\n");
    } else if !text.ends_with('\n') {
        text.push('\n');
    }
    text.push_str(l);
    text.push('\n');
    save(&text)?;
    Ok(true)
}

/// Removes an entry (every line whose entry part equals `line`, trailing comment included).
/// Returns `false` if it wasn't on the list. Other lines and comments stay as they are.
pub fn remove_from_ignore(line: &str) -> Result<bool, String> {
    let l = line.trim();
    let text = read_existing()?;
    let kept: Vec<&str> = text.lines().filter(|x| entry_of(x) != l || l.is_empty()).collect();
    if kept.len() == text.lines().count() {
        return Ok(false);
    }
    let mut out = kept.join("\n");
    if !out.is_empty() {
        out.push('\n');
    }
    save(&out)?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::super::tests::FakeHome;
    use super::super::IgnoreList;
    use super::*;

    #[test]
    fn add_and_remove_keep_comments() {
        let fake = FakeHome::new("ignore-edit");
        assert_eq!(ignore_entries().unwrap(), Vec::<String>::new());
        assert!(add_to_ignore("~/Documents/game/build").unwrap());
        assert!(!add_to_ignore("  ~/Documents/game/build ").unwrap(), "no duplicates");
        let f = ignore_file();
        let mut text = std::fs::read_to_string(&f).unwrap();
        text.push_str("# my own note\nSecret Stuff   # trailing\nid:docker-*"); // no final newline
        std::fs::write(&f, text).unwrap();
        assert!(add_to_ignore("id:trash").unwrap());
        assert_eq!(ignore_entries().unwrap(), ["~/Documents/game/build", "Secret Stuff", "id:docker-*", "id:trash"]);
        assert!(IgnoreList::load().blocks_path(&fake.path.join("Documents/game/build/x")));
        assert!(remove_from_ignore("Secret Stuff").unwrap());
        assert!(!remove_from_ignore("Secret Stuff").unwrap());
        let text = std::fs::read_to_string(&f).unwrap();
        assert!(text.contains("# my own note\n") && !text.contains("Secret"));
        assert!(text.ends_with("id:trash\n"));
        assert!(add_to_ignore("a#b").is_err());
        assert!(add_to_ignore("a\nb").is_err());
        assert!(add_to_ignore("  ").is_err());
        assert_eq!(IgnoreEntry::parse("id:proj:/a/b"), IgnoreEntry::Project("/a/b".into()));
        assert_eq!(IgnoreEntry::parse(&IgnoreEntry::line_for_item("trash")), IgnoreEntry::Item("trash".into()));
        assert_eq!(IgnoreEntry::parse("~/x"), IgnoreEntry::Path("~/x".into()));
    }
}
