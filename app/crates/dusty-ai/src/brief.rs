//! What the helper is told (the briefing) and how its answer is read.

use dusty_core::explore::{expand, Folder, Owner, Risk, Suggestion};
use dusty_core::{format_bytes, tildify, Group, Target};
use serde_json::Value;
use std::path::PathBuf;

pub const SYSTEM: &str = r#"You are the helper inside Dusty, a macOS disk cleaner. Dusty shows you big folders in the user's home that its own catalog doesn't know, with their sizes, how long nothing inside changed, which app they belong to, and the biggest entries inside. You never see file contents, and you have no tools: everything you know is in the message.

Your job: point out the ones that are safe to move to the Trash, and explain each in plain words a non-expert understands.

Safe means it comes back by itself or with a download: caches, logs, temporary or partial files, crash dumps, old update downloads, leftovers of apps that are no longer installed, models or packages that can be downloaded again.

Rules:
- Only suggest paths you were shown: a listed folder, or an entry listed inside one (join the folder path and the entry name). Never invent a path.
- Never suggest documents, projects or source code, settings, databases or libraries holding the user's own content, passwords, mail, messages, notes, photos, music, game saves, virtual machine disks, or anything the user would miss after a re-download.
- If a folder is big and you can't tell from what's shown, ask to look closer instead of guessing. If you're unsure, don't suggest it.
- risk "low": rebuilt automatically, nothing is lost. risk "medium": costs a download, a sign-in or a slow first start. Never suggest anything riskier.
- Don't suggest what is listed as already handled or already suggested.
- Prefer the most specific path: the cache folder inside an app's folder, not the whole app folder, unless no installed app matches it.

Answer with one JSON object and nothing else:
{"suggestions":[{"path":"~/…","what":"short name, at most 6 words","why":"one or two sentences: what it is and why it can go","comes_back":"what happens once it's gone, in a few words","risk":"low|medium","group":"apps|cache|junk|redownload","evidence":["short facts from the listing, not its size (Dusty shows that)"]}],"look_closer":["~/… folders you want to see inside, at most 6"],"note":"optional, one short sentence about what you left alone and why (not about what you suggested)"}
Use "apps" for app caches and leftovers, "cache" for developer tool caches, "junk" for logs and temporary files, "redownload" for things that cost a download."#;

/// "~/x · 3.2 GB · untouched for 210 days · no installed app matches it · inside: models/ 3.0 GB, …"
fn folder_line(f: &Folder) -> String {
    let owner = match &f.owner {
        Owner::Unknown => String::new(),
        Owner::Installed { name, running } => format!(" · belongs to {name} (installed{})", if *running { ", open now" } else { "" }),
        Owner::NoApp => " · no installed app matches it".into(),
        Owner::Apple => " · part of macOS".into(),
    };
    let inside: Vec<String> = f.inside.iter().map(|(n, b, dir)| format!("{n}{} {}", if *dir { "/" } else { "" }, format_bytes(*b))).collect();
    let more = if f.more > 0 { format!(", +{} smaller", f.more) } else { String::new() };
    format!("- {} · {} · untouched for {} days{owner} · inside: {}{more}", tildify(&f.path), format_bytes(f.bytes), f.untouched_days, inside.join(", "))
}

/// The first message: what Dusty handles already, what was suggested before, and the folders to review.
pub fn first(known: &[Target], earlier: &[Suggestion], folders: &[Folder]) -> String {
    let mut s = String::from("ALREADY HANDLED BY DUSTY'S CHECKUP (don't suggest these or anything inside them):\n");
    let projects = known.iter().filter(|t| t.id.starts_with("proj")).count();
    if projects > 0 {
        s.push_str(&format!("- {projects} project build folders (node_modules, target, build…) in the user's code folders\n"));
    }
    for t in known.iter().filter(|t| !t.id.starts_with("proj")).take(80) {
        let paths: Vec<String> = t.paths.iter().take(3).map(|p| tildify(p)).collect();
        let more = if t.paths.len() > 3 { format!(" +{} more", t.paths.len() - 3) } else { String::new() };
        s.push_str(&format!("- {}: {}{more}\n", t.label, paths.join(", ")));
    }
    if !earlier.is_empty() {
        s.push_str("\nALREADY SUGGESTED EARLIER (the user is deciding; don't repeat):\n");
        for e in earlier {
            s.push_str(&format!("- {}: {}\n", tildify(&e.path), e.what));
        }
    }
    s.push_str("\nFOLDERS TO REVIEW:\n");
    for f in folders {
        s.push_str(&folder_line(f));
        s.push('\n');
    }
    s
}

/// A closer look at the folders the helper asked about.
pub fn closer(folders: &[Folder]) -> String {
    let mut s = String::from("INSIDE THE FOLDERS YOU ASKED ABOUT:\n");
    if folders.is_empty() {
        s.push_str("(nothing more to show there: it's small, protected or already handled)\n");
    }
    for f in folders {
        s.push_str(&folder_line(f));
        s.push('\n');
    }
    s.push_str("\nAnswer with the same JSON. List only new suggestions; leave look_closer empty unless you really need one more look.");
    s
}

pub const NOT_JSON: &str = "That wasn't the JSON object I need. Reply again with only the JSON object, nothing around it.";

/// One suggestion as the helper wrote it (the path still unchecked).
pub struct Raw {
    pub path: PathBuf,
    pub what: String,
    pub why: String,
    pub comes_back: String,
    pub risk: Risk,
    pub group: Group,
    pub evidence: Vec<String>,
}

pub struct Reply {
    pub suggestions: Vec<Raw>,
    pub look_closer: Vec<PathBuf>,
    pub note: Option<String>,
}

/// Plain text, one line, at most `max` characters.
fn clean(v: Option<&Value>, max: usize) -> String {
    let s: String = v.and_then(Value::as_str).unwrap_or("").chars().map(|c| if c.is_control() { ' ' } else { c }).collect();
    let s = s.split_whitespace().collect::<Vec<_>>().join(" ");
    if s.chars().count() > max { format!("{}…", s.chars().take(max - 1).collect::<String>().trim_end()) } else { s }
}

/// Reads the helper's answer: the JSON object in it (code fences and stray words around it are fine).
/// Suggestions with a missing path or name, or a risk other than low or medium, are dropped.
pub fn parse(text: &str) -> Option<Reply> {
    let (start, end) = (text.find('{')?, text.rfind('}')?);
    let v: Value = serde_json::from_str(text.get(start..=end)?).ok()?;
    let list = |k: &str| v.get(k).and_then(Value::as_array).cloned().unwrap_or_default();
    let suggestions = list("suggestions")
        .iter()
        .filter_map(|s| {
            let risk = match s.get("risk").and_then(Value::as_str)? {
                "low" => Risk::Low,
                "medium" => Risk::Medium,
                _ => return None,
            };
            let group = match s.get("group").and_then(Value::as_str).unwrap_or("") {
                "cache" => Group::Cache,
                "junk" => Group::Junk,
                "redownload" => Group::Redownload,
                _ => Group::Apps,
            };
            let what = clean(s.get("what"), 48);
            let why = clean(s.get("why"), 320);
            if what.is_empty() || why.is_empty() {
                return None;
            }
            let evidence = s.get("evidence").and_then(Value::as_array).map(|e| e.iter().take(4).map(|x| clean(Some(x), 90)).filter(|x| !x.is_empty()).collect()).unwrap_or_default();
            Some(Raw { path: expand(s.get("path")?.as_str()?)?, what, why, comes_back: clean(s.get("comes_back"), 120), risk, group, evidence })
        })
        .collect();
    let look_closer = list("look_closer").iter().filter_map(|p| expand(p.as_str()?)).take(6).collect();
    let note = Some(clean(v.get("note"), 400)).filter(|n| !n.is_empty());
    Some(Reply { suggestions, look_closer, note })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn answers_are_read_carefully() {
        let hm = dusty_core::home();
        let text = r#"Sure! ```json
{"suggestions":[
  {"path":"~/Library/Application Support/Gone/Cache","what":"Gone's cache","why":"A cache.\nRebuilt.","comes_back":"rebuilt","risk":"low","group":"apps","evidence":["no app"]},
  {"path":"~/Documents","what":"Docs","why":"x","risk":"high","group":"junk"},
  {"path":"relative/path","what":"x","why":"y","risk":"low"},
  {"path":"~/.tool","what":"","why":"y","risk":"low"}
],"look_closer":["~/.big"],"note":"All good."}
```"#;
        let r = parse(text).unwrap();
        assert_eq!(r.suggestions.len(), 1, "high risk, relative paths and nameless ones are dropped");
        let s = &r.suggestions[0];
        assert_eq!(s.path, hm.join("Library/Application Support/Gone/Cache"));
        assert_eq!(s.why, "A cache. Rebuilt.");
        assert_eq!((s.risk, s.group), (Risk::Low, Group::Apps));
        assert_eq!(r.look_closer, vec![hm.join(".big")]);
        assert_eq!(r.note.as_deref(), Some("All good."));
        assert!(parse("no json here").is_none());
    }
}
