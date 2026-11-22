//! One function per command. Each one gathers input, calls `dusty-core`, and hands the result to
//! [`render`] or [`json`]. No cleaning rule lives here.

use crate::{json, render, LargeArgs, Opts, ScheduleState};
use dialoguer::{Confirm, MultiSelect};
use dusty_core::explore::{self, Suggestion};
use dusty_core::history::{self, HistorySource};
use dusty_core::large::{self, LargeFileOptions, LargeProgress};
use dusty_core::schedule::{self, Schedule};
use dusty_core::{format_bytes, CleanPlan, CleanPolicy, CleanProgress, ScanOptions, ScanProgress, Target};
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::Mutex;

const MB: u64 = 1024 * 1024;
/// `dusty large` lists at most this many files.
const LARGE_LIMIT: usize = 200;

type Result = std::result::Result<(), String>;

pub fn scan(o: &Opts) -> Result {
    let targets = find_targets(o);
    if o.json {
        println!("{}", json::pretty(&json::targets(&targets)));
        return Ok(());
    }
    render::free_space();
    render::report(&targets);
    render::manual_hints();
    render::hint("Run `dusty clean` to choose what to remove.");
    Ok(())
}

pub fn clean(o: &Opts) -> Result {
    if o.json {
        return clean_json(o);
    }
    render::free_space();
    let targets = find_targets(o);
    if targets.is_empty() {
        render::done("Nothing to clean 🎉");
        return Ok(());
    }
    render::report(&targets);
    let preselected = |t: &Target| o.all || t.default_on;
    let picked: Vec<Target> = if o.yes {
        targets.iter().filter(|t| preselected(t)).cloned().collect()
    } else {
        let labels: Vec<String> = targets.iter().map(render::target_choice).collect();
        let defaults: Vec<bool> = targets.iter().map(preselected).collect();
        let Some(ix) = pick("What should be removed? (space = toggle, enter = confirm)", &labels, &defaults)? else {
            render::done("Cancelled.");
            return Ok(());
        };
        ix.into_iter().map(|i| targets[i].clone()).collect()
    };
    if picked.is_empty() {
        render::done("Nothing selected.");
        return Ok(());
    }
    let plan = CleanPlan::new(&picked, policy(o), HistorySource::Cli, o.dry_run);
    if !o.yes && !plan.practice && !confirm(&render::plan_question(&plan))? {
        render::done("Cancelled.");
        return Ok(());
    }
    let progress = Mutex::new(CleanProgress::default());
    let before = dusty_core::disk().free;
    let report = render::with_status(
        false,
        || {
            let p = progress.lock().unwrap();
            format!("[{}/{}] {}", p.done + 1, p.total, p.current.as_deref().unwrap_or(""))
        },
        || dusty_core::execute(&plan, &mut |e| progress.lock().unwrap().apply(&e), &AtomicBool::new(false)),
    );
    render::clean_summary(&report, plan.bytes(None), before, dusty_core::disk().free, o.trash);
    render::manual_hints();
    Ok(())
}

/// `dusty clean --json`: never asks. With `--dry-run` it prints the plan and touches nothing (not even
/// the history); with `--yes` it cleans the default selection and prints the plan and the report.
fn clean_json(o: &Opts) -> Result {
    if !o.yes && !o.dry_run {
        return Err("--json never asks: add --dry-run to see the plan, or --yes to clean".into());
    }
    let picked: Vec<Target> = find_targets(o).into_iter().filter(|t| o.all || t.default_on).collect();
    let plan = CleanPlan::new(&picked, policy(o), HistorySource::Cli, o.dry_run);
    let report = (!plan.practice).then(|| dusty_core::execute(&plan, &mut |_| {}, &AtomicBool::new(false)));
    println!("{}", json::pretty(&json::clean(&plan, report.as_ref())));
    Ok(())
}

/// Delete by default, like the CLI always did; `--trash` moves everything to the Trash instead.
fn policy(o: &Opts) -> CleanPolicy {
    if o.trash { CleanPolicy::TrashAll } else { CleanPolicy::DeleteAll }
}

pub fn history(o: &Opts) -> Result {
    let entries = history::load_history();
    if o.json {
        println!("{}", json::pretty(&json::history(&entries)));
    } else {
        render::history(&entries);
    }
    Ok(())
}

pub fn put_back(id: &str) -> Result {
    let r = history::put_back(id, None);
    render::put_back(&r);
    Ok(())
}

pub fn large(o: &Opts, a: &LargeArgs) -> Result {
    let opts = LargeFileOptions {
        roots: if o.roots.is_empty() { large::default_large_roots() } else { o.roots.clone() },
        min_bytes: a.over * MB,
        unused_days: (!a.size_only).then_some(a.unused),
        require_both: a.both,
        ..Default::default()
    };
    let progress = Mutex::new(LargeProgress::default());
    let mut files = render::with_status(o.json, || progress.lock().unwrap().message.clone(), || large::find_large_files(&opts, &progress, &AtomicBool::new(false)));
    files.truncate(LARGE_LIMIT);
    if o.json {
        println!("{}", json::pretty(&json::large_files(&files)));
        return Ok(());
    }
    if files.is_empty() {
        render::done("Nothing big or forgotten here 🎉");
        return Ok(());
    }
    // review only: nothing is preselected, and picks go to the Trash
    let labels: Vec<String> = files.iter().map(render::large_choice).collect();
    let Some(ix) = pick("Pick files to move to the Trash (enter with nothing picked to just look)", &labels, &vec![false; files.len()])? else {
        render::done("Nothing moved.");
        return Ok(());
    };
    if ix.is_empty() {
        render::done("Nothing moved.");
        return Ok(());
    }
    let paths: Vec<PathBuf> = ix.iter().map(|&i| files[i].path.clone()).collect();
    let total: u64 = ix.iter().map(|&i| files[i].bytes).sum();
    if !o.dry_run && !confirm(&format!("Move {} file(s), {}, to the Trash?", paths.len(), format_bytes(total)))? {
        render::done("Cancelled.");
        return Ok(());
    }
    render::trashed_files(&large::trash_files(&paths, o.dry_run));
    Ok(())
}

pub fn explore(o: &Opts, pick_agent: Option<&str>, list: bool, verbose: bool) -> Result {
    let mut prefs = dusty_ai::AgentPrefs::load();
    if list {
        render::agents(&prefs);
        return Ok(());
    }
    if let Some(id) = pick_agent {
        let ids: Vec<&str> = dusty_ai::AGENTS.iter().map(|a| a.id).collect();
        dusty_ai::agent(id).ok_or_else(|| format!("No agent called `{id}` (try {})", ids.join(", ")))?;
        prefs.selected = Some(id.to_string());
        let _ = prefs.save();
    }
    let agent = prefs.agent();
    let known = find_targets(o);
    let progress = Mutex::new(dusty_ai::ExploreProgress::default());
    let r = render::with_status(o.json, || progress.lock().unwrap().message.clone(), || dusty_ai::explore(agent, Some(known), &progress, &AtomicBool::new(false)));
    if o.json {
        println!("{}", json::pretty(&json::explore(&r, agent)));
        return Ok(());
    }
    render::explore(&r, agent, verbose);
    if r.suggestions.is_empty() {
        return Ok(());
    }
    // review only: nothing is preselected, and picks go to the Trash
    let labels: Vec<String> = r.suggestions.iter().map(render::suggestion_choice).collect();
    let picked = match pick("Pick what to move to the Trash (enter with nothing picked to just look)", &labels, &vec![false; labels.len()])? {
        Some(ix) if !ix.is_empty() => ix,
        _ => {
            render::done("Nothing moved.");
            return Ok(());
        }
    };
    let items: Vec<Suggestion> = picked.iter().map(|&i| r.suggestions[i].clone()).collect();
    let total: u64 = items.iter().map(|s| s.bytes).sum();
    if !o.dry_run && !confirm(&format!("Move {} item(s), {}, to the Trash?", items.len(), format_bytes(total)))? {
        render::done("Cancelled.");
        return Ok(());
    }
    let report = explore::trash(&items, o.dry_run);
    render::trashed_files(&report);
    if !o.dry_run && report.errors.is_empty() && confirm("Look for these in future checkups too (unticked)?")? {
        let n = explore::remember(&items)?;
        render::done(&format!("Added {n} to {}.", dusty_core::tildify(&dusty_core::user_catalog_file())));
    }
    Ok(())
}

pub fn schedule(state: ScheduleState) -> Result {
    match state {
        ScheduleState::On => {
            let f = schedule::install_schedule(&Schedule::default())?;
            render::done(&format!("Weekly safe clean on (Sundays 11:00): {}", dusty_core::tildify(&f)));
        }
        ScheduleState::Off => render::done(if schedule::remove_schedule()? { "Weekly clean off." } else { "There was no weekly clean." }),
        ScheduleState::Status if schedule::is_scheduled() => render::done(&format!("Weekly clean on ({}).", dusty_core::tildify(&schedule::launch_agent_path()))),
        ScheduleState::Status => render::done("Weekly clean off."),
    }
    Ok(())
}

/// Scans with the options from the command line, showing progress on stderr.
fn find_targets(o: &Opts) -> Vec<Target> {
    let roots = if o.roots.is_empty() { dusty_core::default_roots() } else { o.roots.clone() };
    let opts = ScanOptions { roots, keep_recent_days: o.keep_recent, min_size_mb: o.min_size };
    if let Some(problem) = dusty_core::user_catalog_problem() {
        render::warn(problem);
    }
    let progress = Mutex::new(ScanProgress::default());
    render::with_status(o.json, || progress.lock().unwrap().message.clone(), || dusty_core::scan_all(&opts, &|e| progress.lock().unwrap().apply(e), &AtomicBool::new(false)))
}

/// Checkbox list; `None` when cancelled (Esc / q).
fn pick(prompt: &str, labels: &[String], defaults: &[bool]) -> std::result::Result<Option<Vec<usize>>, String> {
    MultiSelect::new().with_prompt(prompt).items(labels).defaults(defaults).max_length(20).interact_opt().map_err(|e| e.to_string())
}

fn confirm(prompt: &str) -> std::result::Result<bool, String> {
    Ok(Confirm::new().with_prompt(prompt).default(true).interact_opt().map_err(|e| e.to_string())?.unwrap_or(false))
}
