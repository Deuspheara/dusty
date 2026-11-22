//! Dusty's helper ("Look further"): asks an AI to explain the big folders a checkup doesn't know, and
//! turns its answer into suggestions you pick from.
//!
//! The helper is an AI agent you pick (Claude, Codex, Gemini CLI, Copilot, Cursor, OpenCode), reached
//! through the Agent Client Protocol with your own sign-in and subscription (see [`agents`]). It is locked
//! down first (no tools, or its read-only mode), Dusty refuses every permission it asks for, and what Dusty
//! sends it is only what [`dusty_core::explore`] lists (folder names, sizes, ages and owning apps). Every
//! suggestion then goes through [`dusty_core::explore::check`] before you see it.
//!
//! One trip: a checkup says what Dusty handles already → the survey finds what's big and unexplained →
//! the helper reviews it, and may ask to look inside a few folders (twice at most) → its suggestions are
//! checked and measured → what was shown and suggested is remembered for next time.

pub mod acp;
pub mod agents;
mod brief;

pub use agents::{agent, Agent, AgentPrefs, Availability, Lock, AGENTS};
use dusty_core::explore::{self, Folder, Known, Memory, Suggestion};
use dusty_core::{ScanOptions, ScanProgress, Target};
use std::sync::atomic::AtomicBool;
use std::sync::Mutex;

/// Closer looks the helper may ask for in one trip.
const CLOSER_LOOKS: usize = 2;

/// Live progress for the UI.
#[derive(Default, Clone, Debug)]
pub struct ExploreProgress {
    pub message: String,
    /// Where the trip is, for a step list.
    pub stage: Stage,
    /// Folders shown to the helper so far, their total size, and the biggest few (`~/…`, bytes).
    pub folders: usize,
    pub folder_bytes: u64,
    pub biggest: Vec<(String, u64, u64)>,
    /// Suggestions ready so far (earlier ones first, then the helper's as they pass the checks).
    pub found: Vec<Suggestion>,
    /// How many of `found` come from earlier trips.
    pub earlier: usize,
}

/// The steps of a trip, in order.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Stage {
    /// Finding and measuring the folders the checkup doesn't know.
    #[default]
    Measure,
    /// Starting the agent.
    Wake,
    /// The agent reads the list and answers; each suggestion is checked as it comes.
    Read,
}

#[derive(Debug, PartialEq)]
pub enum ExploreError {
    /// The agent wants you to sign in (its `login` command, in Terminal).
    SignIn,
    /// It can't be started here (not installed, no Node.js): what to do.
    Unavailable(String),
    /// It has no read-only mode, so it wasn't asked.
    NoSafeMode,
    /// It reported changing a file, so the trip stopped.
    Meddled,
    Stopped,
    Failed(String),
}

impl ExploreError {
    /// What to tell the person about `agent`.
    pub fn message(&self, agent: &Agent) -> String {
        let name = agent.name;
        match self {
            ExploreError::SignIn => format!("{name} needs you to sign in first: open Terminal, run `{}`, and log in. Then look again.", agent.login),
            ExploreError::Unavailable(what) => format!("{name} isn't ready on this Mac ({what})."),
            ExploreError::NoSafeMode => format!("{name} has no read-only mode I could switch on, so I didn't ask it. Pick another helper."),
            ExploreError::Meddled => format!("{name} tried to change a file, so I stopped it. Nothing it suggested was kept."),
            ExploreError::Stopped => "Stopped.".into(),
            ExploreError::Failed(why) => format!("{name} couldn't help this time: {why}."),
        }
    }
}

#[derive(Debug, Default)]
pub struct ExploreReport {
    /// Biggest first. None of them is ticked.
    pub suggestions: Vec<Suggestion>,
    /// Folders shown to the helper.
    pub looked: usize,
    /// Folders skipped because they haven't grown since the last trip.
    pub unchanged: usize,
    /// Suggestions the checks turned down, with why (for the CLI's `--verbose`).
    pub refused: Vec<String>,
    /// The helper's word to the person, if any.
    pub note: Option<String>,
    /// Whether the helper was asked at all (not when there was nothing new to show).
    pub asked: bool,
    /// The account the agent said it used ("Claude Pro", "ChatGPT Team"), when it said.
    pub account: Option<String>,
    /// Tokens it reported spending, when it did.
    pub tokens: u64,
    /// Times it looked at something itself (agents in read-only mode may).
    pub own_looks: usize,
    pub error: Option<ExploreError>,
}

/// One trip with `agent`. `known` is the checkup you already have (`None`: Dusty runs one first). Blocking:
/// call it from a background thread.
pub fn explore(agent: &Agent, known: Option<Vec<Target>>, progress: &Mutex<ExploreProgress>, cancel: &AtomicBool) -> ExploreReport {
    let status = |m: &str| progress.lock().unwrap().message = m.to_string();
    let stopped = || cancel.load(std::sync::atomic::Ordering::Relaxed);
    let mut report = ExploreReport::default();

    let known = known.unwrap_or_else(|| {
        status("Checking what I already know…");
        let opts = ScanOptions { roots: dusty_core::default_roots(), keep_recent_days: 0, min_size_mb: 1 };
        let p = Mutex::new(ScanProgress::default());
        dusty_core::scan_all(&opts, &|e| p.lock().unwrap().apply(e), cancel)
    });
    let mut memory = Memory::load();
    let mut handled = Known::from_targets(&known);
    let earlier = memory.still_here(&handled);
    {
        let mut p = progress.lock().unwrap();
        p.found = earlier.clone();
        p.earlier = earlier.len();
    }
    for p in memory.known_paths() {
        handled.add(p);
    }

    status("Sniffing around your Library…");
    let mut survey = explore::survey(handled, &memory, cancel);
    report.unchanged = survey.unchanged;
    let mut shown: Vec<Folder> = survey.folders.clone();
    {
        let mut p = progress.lock().unwrap();
        p.folders = shown.len();
        p.folder_bytes = shown.iter().map(|f| f.bytes).sum();
        p.biggest = shown.iter().take(8).map(|f| (dusty_core::tildify(&f.path), f.bytes, f.untouched_days)).collect();
    }
    if stopped() {
        report.error = Some(ExploreError::Stopped);
        return report;
    }
    if shown.is_empty() {
        // nothing new: no need to ask anyone
        report.suggestions = earlier;
        return report;
    }

    let cmd = match agent.command(brief::SYSTEM) {
        Ok(c) => c,
        Err(what) => {
            report.error = Some(ExploreError::Unavailable(what));
            report.suggestions = earlier;
            return report;
        }
    };
    let name = agent.name;
    progress.lock().unwrap().stage = Stage::Wake;
    status(&format!("Waking up {name}…"));
    let cwd = std::env::temp_dir().join(format!("dusty-helper-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&cwd);
    let _ = std::fs::create_dir_all(&cwd);
    report.asked = true;
    let mut session_info = (None, 0, 0);
    let result = (|| -> Result<Vec<Suggestion>, ExploreError> {
        let mut session = acp::Session::start(&cmd, &cwd, cancel).map_err(into_error)?;
        let r = trip(&mut session, name, &known, &earlier, &mut shown, &mut survey, &mut report, progress, cancel);
        session_info = (session.account.clone(), session.tokens, session.own_looks);
        r
    })();
    (report.account, report.tokens, report.own_looks) = session_info;
    if let Some(a) = &report.account {
        // the picker shows it next time, before anything runs
        let mut prefs = AgentPrefs::load();
        prefs.accounts.insert(agent.id.to_string(), a.clone());
        let _ = prefs.save();
    }
    let _ = std::fs::remove_dir_all(&cwd);
    report.looked = shown.len();
    match result {
        Ok(accepted) => {
            memory.record(&shown, &accepted);
            // losing the memory only means asking again next time
            let _ = memory.save();
            report.suggestions = earlier.into_iter().chain(accepted).collect();
        }
        Err(e) => {
            // whatever passed the checks before it went wrong is still worth showing, unless it meddled
            report.suggestions = if e == ExploreError::Meddled { earlier } else { progress.lock().unwrap().found.clone() };
            report.error = Some(e);
        }
    }
    report.suggestions.sort_by_key(|s| std::cmp::Reverse(s.bytes));
    report
}

/// The conversation: the briefing, then up to [`CLOSER_LOOKS`] closer looks. Returns what passed the checks.
#[allow(clippy::too_many_arguments)]
fn trip(
    session: &mut acp::Session,
    name: &str,
    known: &[Target],
    earlier: &[Suggestion],
    shown: &mut Vec<Folder>,
    survey: &mut explore::Survey,
    report: &mut ExploreReport,
    progress: &Mutex<ExploreProgress>,
    cancel: &AtomicBool,
) -> Result<Vec<Suggestion>, ExploreError> {
    let status = |m: &str| progress.lock().unwrap().message = m.to_string();
    let mut message = brief::first(known, earlier, shown);
    let mut accepted: Vec<Suggestion> = vec![];
    progress.lock().unwrap().stage = Stage::Read;
    for round in 0..=CLOSER_LOOKS {
        status(&format!("{name} is reading about {} folder{}…", shown.len(), if shown.len() == 1 { "" } else { "s" }));
        let reply = ask(session, &message, cancel, &|| status(&format!("{name} is writing it up…")))?;
        for raw in reply.suggestions {
            // one suggestion per folder: nothing inside or around one already taken
            if accepted.iter().chain(earlier).any(|a| raw.path.starts_with(&a.path) || a.path.starts_with(&raw.path)) {
                continue;
            }
            match explore::check(&raw.path, survey) {
                Ok(bytes) => {
                    let s = Suggestion { path: raw.path, bytes, what: raw.what, why: raw.why, comes_back: raw.comes_back, risk: raw.risk, group: raw.group, evidence: raw.evidence, seen_before: false, untouched_days: None };
                    let s = Suggestion { untouched_days: explore::untouched_days(&s.path), ..s };
                    progress.lock().unwrap().found.push(s.clone());
                    accepted.push(s);
                }
                Err(why) => report.refused.push(why),
            }
        }
        if reply.note.is_some() {
            report.note = reply.note;
        }
        if reply.look_closer.is_empty() || round == CLOSER_LOOKS || cancel.load(std::sync::atomic::Ordering::Relaxed) {
            break;
        }
        status(&format!("Looking closer, like {name} asked…"));
        let more = explore::look_closer(survey, &reply.look_closer, cancel);
        shown.extend(more.iter().cloned());
        progress.lock().unwrap().folders = shown.len();
        message = brief::closer(&more);
    }
    Ok(accepted)
}

/// One question, with one more try if the answer isn't the JSON asked for.
fn ask(session: &mut acp::Session, message: &str, cancel: &AtomicBool, writing: &dyn Fn()) -> Result<brief::Reply, ExploreError> {
    let text = session.prompt(message, cancel, &mut |_| writing()).map_err(into_error)?;
    if let Some(r) = brief::parse(&text) {
        return Ok(r);
    }
    let again = session.prompt(brief::NOT_JSON, cancel, &mut |_| {}).map_err(into_error)?;
    brief::parse(&again).ok_or_else(|| ExploreError::Failed("its answer didn't make sense to me".into()))
}

fn into_error(e: acp::AcpError) -> ExploreError {
    match e {
        acp::AcpError::SignIn => ExploreError::SignIn,
        acp::AcpError::Stopped => ExploreError::Stopped,
        acp::AcpError::NoSafeMode => ExploreError::NoSafeMode,
        acp::AcpError::Meddled => ExploreError::Meddled,
        acp::AcpError::Failed(why) => ExploreError::Failed(why),
    }
}
