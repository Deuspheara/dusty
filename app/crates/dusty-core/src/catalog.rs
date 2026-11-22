//! The catalog: what Dusty knows how to clean, as data. `catalog/projects.toml` holds the project
//! rules, `catalog/system.toml` everything else. Both are built into the binary and checked by tests,
//! so adding a kind of project or cache is an edit to a TOML file, not to code.
//!
//! People can add their own in `~/.config/dusty/catalog.toml` (same format, `[[rule]]` and
//! `[[target]]` in one file). Those come after the built-in ones, so they can add but never change
//! or loosen anything, and they can't name providers or `before` commands: a user file never runs code.

use super::*;
use crate::providers::{provider, Provider};
use serde::{Deserialize, Deserializer};
use std::sync::OnceLock;

pub(crate) const PROJECTS: &str = include_str!("../catalog/projects.toml");
pub(crate) const SYSTEM: &str = include_str!("../catalog/system.toml");

pub(crate) struct Catalog {
    pub rules: Vec<Rule>,
    pub targets: Vec<Entry>,
    /// Set when `~/.config/dusty/catalog.toml` exists but was left out (unreadable or invalid).
    pub user_error: Option<String>,
}

/// The built-in catalog plus your own additions, read once per run.
pub(crate) fn catalog() -> &'static Catalog {
    static CATALOG: OnceLock<Catalog> = OnceLock::new();
    CATALOG.get_or_init(|| {
        let mut c = Catalog::parse(PROJECTS, SYSTEM).expect("the built-in catalog is valid (checked by tests)");
        let f = user_catalog_file();
        match std::fs::read_to_string(&f) {
            Ok(text) => {
                if let Err(e) = c.add_user(&text) {
                    c.user_error = Some(format!("{} was left out: {e}", tildify(&f)));
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => c.user_error = Some(format!("couldn't read {} ({e})", tildify(&f))),
        }
        c
    })
}

/// `~/.config/dusty/catalog.toml`: your own project types and folders.
pub fn user_catalog_file() -> PathBuf {
    h(".config/dusty/catalog.toml")
}

/// Why your catalog file was left out this run, if it was. For the app and the CLI to show.
pub fn user_catalog_problem() -> Option<&'static str> {
    catalog().user_error.as_deref()
}

impl Catalog {
    pub fn parse(projects: &str, system: &str) -> Result<Catalog, String> {
        let p: ProjectsFile = toml::from_str(projects).map_err(|e| format!("projects catalog: {e}"))?;
        let s: SystemFile = toml::from_str(system).map_err(|e| format!("system catalog: {e}"))?;
        let mut c = Catalog { rules: vec![], targets: vec![], user_error: None };
        c.add(p.markers, p.rule, s.target)?;
        Ok(c)
    }

    /// Adds the rules and targets of a user catalog file, all or nothing.
    pub fn add_user(&mut self, text: &str) -> Result<(), String> {
        let u: UserFile = toml::from_str(text).map_err(|e| e.message().to_string())?;
        if let Some(t) = u.target.iter().find(|t| t.provider.is_some() || !t.before.is_empty()) {
            return Err(format!("`{}`: providers and `before` commands are for the built-in catalog only", t.id));
        }
        self.add(u.markers, u.rule, u.target)
    }

    fn add(&mut self, markers: HashMap<String, Vec<String>>, rules: Vec<RawRule>, targets: Vec<Entry>) -> Result<(), String> {
        let rules = rules.into_iter().map(|r| r.resolve(&markers)).collect::<Result<Vec<_>, _>>()?;
        for t in &targets {
            t.validate()?;
        }
        let mut ids: HashSet<&str> = self.targets.iter().map(|t| t.id.as_str()).collect();
        if let Some(dup) = targets.iter().find(|t| !ids.insert(t.id.as_str())) {
            return Err(format!("`{}` is listed twice", dup.id));
        }
        self.rules.extend(rules);
        self.targets.extend(targets);
        Ok(())
    }
}

#[derive(Deserialize)]
struct UserFile {
    #[serde(default)]
    markers: HashMap<String, Vec<String>>,
    #[serde(default)]
    rule: Vec<RawRule>,
    #[serde(default)]
    target: Vec<Entry>,
}

// ───────────────────────── project rules ─────────────────────────

#[derive(Deserialize)]
struct ProjectsFile {
    #[serde(default)]
    markers: HashMap<String, Vec<String>>,
    rule: Vec<RawRule>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawRule {
    dir: Option<String>,
    prefix: Option<String>,
    #[serde(default)]
    markers: Vec<String>,
    kind: String,
    #[serde(default)]
    redownload: bool,
}

/// A project folder Dusty may offer: matched by name, confirmed by a marker file next to it.
pub(crate) struct Rule {
    name: NameMatch,
    markers: Vec<String>,
    pub kind: String,
    pub redownload: bool,
}

enum NameMatch {
    Exact(String),
    Prefix(String),
}

impl RawRule {
    fn resolve(self, sets: &HashMap<String, Vec<String>>) -> Result<Rule, String> {
        let name = match (self.dir, self.prefix) {
            (Some(d), None) => NameMatch::Exact(d),
            (None, Some(p)) => NameMatch::Prefix(p),
            _ => return Err(format!("project rule `{}`: give exactly one of `dir` or `prefix`", self.kind)),
        };
        let mut markers = vec![];
        for m in self.markers {
            match m.strip_prefix('@') {
                Some(set) => markers.extend(sets.get(set).ok_or_else(|| format!("project rule `{}`: no marker list `{set}`", self.kind))?.iter().cloned()),
                None => markers.push(m),
            }
        }
        Ok(Rule { name, markers, kind: self.kind, redownload: self.redownload })
    }
}

impl Rule {
    /// `dir_name` is this rule's folder and its parent (whose file names are `parent`) has a marker.
    pub fn matches(&self, dir_name: &str, parent: &HashSet<String>) -> bool {
        let name_ok = match &self.name {
            NameMatch::Exact(n) => dir_name == n,
            NameMatch::Prefix(p) => dir_name.starts_with(p.as_str()),
        };
        name_ok && (self.markers.is_empty() || self.markers.iter().any(|m| has_marker(m, parent)))
    }
}

/// `*.ext` means any file with that extension, anything else an exact file name.
fn has_marker(marker: &str, parent: &HashSet<String>) -> bool {
    match marker.strip_prefix('*') {
        Some(ext) => parent.iter().any(|n| n.len() > ext.len() && n.ends_with(ext)),
        None => parent.contains(marker),
    }
}

// ───────────────────────── system targets ─────────────────────────

#[derive(Deserialize)]
struct SystemFile {
    target: Vec<Entry>,
}

/// One system target: who it is (id, label, group…) and where to look. See `catalog/system.toml`.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Entry {
    pub id: String,
    pub label: String,
    #[serde(deserialize_with = "group_key")]
    group: Group,
    #[serde(default)]
    default_on: bool,
    #[serde(default)]
    wipes_data: bool,
    running: Option<Running>,
    #[serde(default)]
    before: Vec<Vec<String>>,
    #[serde(default)]
    paths: Vec<String>,
    #[serde(default)]
    contents: Vec<String>,
    #[serde(default)]
    exclude: Vec<String>,
    #[serde(default)]
    exclude_protected: bool,
    #[serde(default)]
    files_only: bool,
    #[serde(default)]
    older_than_days: u64,
    #[serde(default)]
    extensions: Vec<String>,
    provider: Option<String>,
    #[serde(default)]
    args: Vec<String>,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum Running {
    /// `"each-app"`: guard on the app named by the path (…/Application Support/<App>/…).
    EachApp(String),
    App { app: String, process: String },
}

fn group_key<'de, D: Deserializer<'de>>(d: D) -> Result<Group, D::Error> {
    let k = String::deserialize(d)?;
    Group::from_key(&k).ok_or_else(|| serde::de::Error::custom(format!("unknown group `{k}`")))
}

/// Shared by every target of one scan.
pub(crate) struct ScanCtx<'a> {
    pub roots: &'a [PathBuf],
    docker: OnceLock<Option<HashMap<String, u64>>>,
}

impl<'a> ScanCtx<'a> {
    pub fn new(roots: &'a [PathBuf]) -> Self {
        ScanCtx { roots, docker: OnceLock::new() }
    }

    /// `docker system df`, asked once per scan whatever the number of Docker targets.
    pub fn docker_df(&self) -> Option<&HashMap<String, u64>> {
        self.docker.get_or_init(crate::providers::docker_df).as_ref()
    }
}

impl Entry {
    fn validate(&self) -> Result<(), String> {
        let err = |why: &str| Err(format!("system catalog: `{}` {why}", self.id));
        let provider = match &self.provider {
            Some(name) => match provider(name) {
                Some(p) => Some(p),
                None => return err(&format!("names an unknown provider `{name}`")),
            },
            None => None,
        };
        if provider.is_none() && self.paths.is_empty() && self.contents.is_empty() {
            return err("has nothing to look at (paths, contents or provider)");
        }
        if matches!(provider, Some(Provider::Found(_))) && !(self.paths.is_empty() && self.contents.is_empty() && self.before.is_empty()) {
            return err("mixes a command provider with paths or `before`");
        }
        let filters = !self.exclude.is_empty() || self.exclude_protected || self.files_only || self.older_than_days > 0 || !self.extensions.is_empty();
        if filters && self.contents.is_empty() {
            return err("has filters but no `contents` to apply them to");
        }
        if let Some(Running::EachApp(s)) = &self.running {
            if s != "each-app" {
                return err(&format!("has running = \"{s}\" (expected \"each-app\" or {{ app, process }})"));
            }
        }
        if self.before.iter().any(|c| c.is_empty()) {
            return err("has an empty command in `before`");
        }
        if self.paths.iter().chain(&self.contents).any(|p| p.starts_with('/') || p.split('/').any(|c| c == "..")) {
            return err("has a path that isn't inside your home folder");
        }
        Ok(())
    }

    /// What is here on this Mac right now, if anything.
    pub fn scan(&self, ctx: &ScanCtx) -> Option<Found> {
        let mut paths = self.listed_paths();
        match self.provider.as_deref().and_then(provider) {
            Some(Provider::Found(find)) => return find(ctx, &self.args),
            Some(Provider::Paths(find)) => paths.extend(find(ctx, &self.args)),
            None => {}
        }
        let sizes = size_of(&paths);
        let existing: Vec<PathBuf> = paths.into_iter().filter(|p| sizes.contains_key(p)).collect();
        if existing.is_empty() {
            return None;
        }
        let bytes = existing.iter().map(|p| sizes[p]).sum();
        let commands = self.before.iter().map(|c| Cmd { cmd: c[0].clone(), args: c[1..].to_vec() }).collect();
        Some(Found { bytes, paths: existing, commands, detail: None })
    }

    /// `contents` (narrowed by the filters), then `paths`.
    fn listed_paths(&self) -> Vec<PathBuf> {
        let keep = |p: &PathBuf| {
            let name = name_of(p);
            let lower = name.to_lowercase();
            !self.exclude.contains(&name)
                && (!self.exclude_protected || !is_protected_name(&name))
                && (!self.files_only || std::fs::symlink_metadata(p).is_ok_and(|m| m.is_file()))
                && (self.extensions.is_empty() || self.extensions.iter().any(|e| lower.ends_with(e.as_str())))
                && (self.older_than_days == 0 || older_than(p, self.older_than_days))
        };
        let mut out: Vec<PathBuf> = self.contents.iter().flat_map(|d| list_dir(&h(d))).filter(keep).collect();
        out.extend(self.paths.iter().map(|p| h(p)));
        out
    }

    pub fn target(&self, f: Found) -> Target {
        Target {
            id: self.id.clone(),
            label: self.label.clone(),
            group: self.group,
            default_on: self.default_on,
            wipes_data: self.wipes_data,
            bytes: f.bytes,
            paths: f.paths,
            commands: f.commands,
            detail: f.detail,
            guard: match &self.running {
                None => Guard::None,
                Some(Running::EachApp(_)) => Guard::PerAppFolder,
                Some(Running::App { app, process }) => Guard::App(app.clone(), process.clone()),
            },
        }
    }
}

/// What a target's scan found on this Mac.
pub(crate) struct Found {
    pub bytes: u64,
    pub paths: Vec<PathBuf>,
    pub commands: Vec<Cmd>,
    pub detail: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn built_in_catalog_is_valid() {
        let c = Catalog::parse(PROJECTS, SYSTEM).unwrap();
        assert!(c.rules.len() > 50 && c.targets.len() > 50);
    }

    /// Every project rule, generated from the catalog: found next to its marker, and (unless a
    /// marker-less rule claims the name anyway) not found without one.
    #[test]
    fn every_project_rule_needs_its_marker() {
        let rules = &catalog().rules;
        let name_of_rule = |r: &Rule| match &r.name {
            NameMatch::Exact(n) => n.clone(),
            NameMatch::Prefix(p) => format!("{p}debug"),
        };
        for r in rules {
            let dir = name_of_rule(r);
            let marker = r.markers.first().map(|m| m.replace('*', "app")).unwrap_or_default();
            let parent: HashSet<String> = [marker.clone()].into_iter().collect();
            assert!(r.matches(&dir, &parent), "`{}`: {dir} next to {marker} isn't matched", r.kind);
            let claimed_without_marker = rules.iter().any(|x| x.markers.is_empty() && x.matches(&dir, &HashSet::new()));
            if !r.markers.is_empty() && !claimed_without_marker {
                assert!(rules.iter().all(|x| !x.matches(&dir, &HashSet::new())), "{dir} matched without any marker");
            }
        }
    }

    /// Every list-based system target, generated from the catalog: what it lists is offered in a fake
    /// home, and what its filters leave out is not.
    #[test]
    fn every_listed_system_target_finds_its_files() {
        use crate::tests::{set_old, touch, FakeHome};
        let fake = FakeHome::new("catalog");
        let ctx = ScanCtx::new(&[]);
        for e in catalog().targets.iter().filter(|e| e.provider.is_none()) {
            let (mut want, mut unwanted) = (vec![], vec![]);
            for p in &e.paths {
                touch(&fake.path.join(p).join("x"), 10);
                want.push(fake.path.join(p));
            }
            for dir in &e.contents {
                let ext = e.extensions.first().cloned().unwrap_or_default();
                let kept = fake.path.join(dir).join(format!("{}-kept{ext}", e.id));
                touch(&kept, 10);
                if e.older_than_days > 0 {
                    set_old(&kept, e.older_than_days + 1);
                    let recent = fake.path.join(dir).join(format!("{}-new{ext}", e.id));
                    touch(&recent, 10);
                    unwanted.push(recent);
                }
                want.push(kept);
                for x in &e.exclude {
                    touch(&fake.path.join(dir).join(x).join("x"), 10);
                    unwanted.push(fake.path.join(dir).join(x));
                }
            }
            let found = e.scan(&ctx).map(|f| f.paths).unwrap_or_default();
            for w in &want {
                assert!(found.contains(w), "`{}` should offer {}", e.id, w.display());
            }
            for u in &unwanted {
                assert!(!found.contains(u), "`{}` shouldn't offer {}", e.id, u.display());
            }
        }
    }

    #[test]
    fn mistakes_are_reported() {
        let rules = "[[rule]]\ndir = \"x\"\nkind = \"X\"\n";
        let target = |extra: &str| format!("[[target]]\nid = \"a\"\nlabel = \"A\"\ngroup = \"junk\"\n{extra}\n");
        let bad = |projects: &str, system: &str| Catalog::parse(projects, system).err().unwrap_or_default();
        assert!(bad(rules, &target("paths = [\"x\"]\n[[target]]\nid = \"a\"\nlabel = \"B\"\ngroup = \"junk\"\npaths = [\"y\"]")).contains("twice"));
        // a user file adds, all or nothing, and never runs code
        let mut c = Catalog::parse(PROJECTS, SYSTEM).unwrap();
        let (rules_before, targets_before) = (c.rules.len(), c.targets.len());
        assert!(c.add_user(&target("provider = \"docker\"")).unwrap_err().contains("built-in catalog only"));
        assert!(c.add_user(&target("paths = [\"x\"]\nbefore = [[\"rm\", \"-rf\", \"/\"]]")).unwrap_err().contains("built-in catalog only"));
        assert!(c.add_user("[[target]]\nid = \"trash\"\nlabel = \"T\"\ngroup = \"junk\"\npaths = [\"x\"]\n").unwrap_err().contains("twice"));
        assert_eq!((c.rules.len(), c.targets.len()), (rules_before, targets_before));
        c.add_user(&format!("{rules}{}", target("contents = [\"Library/Caches/MyTool\"]"))).unwrap();
        assert_eq!((c.rules.len(), c.targets.len()), (rules_before + 1, targets_before + 1));
        assert!(bad(rules, &target("")).contains("nothing to look at"));
        assert!(bad(rules, &target("provider = \"nope\"")).contains("unknown provider"));
        assert!(bad(rules, &target("paths = [\"x\"]\nfiles_only = true")).contains("no `contents`"));
        assert!(bad(rules, &target("paths = [\"../x\"]")).contains("home folder"));
        assert!(bad(rules, &target("paths = [\"x\"]\nrunning = \"each\"")).contains("each-app"));
        assert!(bad(rules, "[[target]]\nid = \"a\"\nlabel = \"A\"\ngroup = \"nope\"\npaths = [\"x\"]\n").contains("unknown group"));
        assert!(bad(rules, &target("paths = [\"x\"]\npath = [\"y\"]")).contains("unknown field"));
        assert!(bad("[[rule]]\nkind = \"X\"\n", &target("paths = [\"x\"]")).contains("exactly one"));
        assert!(bad("[[rule]]\ndir = \"x\"\nmarkers = [\"@nope\"]\nkind = \"X\"\n", &target("paths = [\"x\"]")).contains("no marker list"));
    }
}
