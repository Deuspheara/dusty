//! Text output for a terminal: the grouped report, summaries, and a one-line status while work runs.
//! Status and errors go to stderr, so stdout stays clean for `--json` and pipes.

use console::{style, Term};
use dusty_core::explore::{Risk, Suggestion};
use dusty_core::history::{HistoryEntry, PutBackReport};
use dusty_core::large::LargeFile;
use dusty_core::{format_bytes, tildify, CleanPlan, CleanReport, Group, Removal, Target};
use std::time::Duration;

const BAR_WIDTH: usize = 20;
const LABEL_WIDTH: usize = 40;

/// Runs `work` on a thread while showing `status()` on one stderr line (unless `quiet` or not a
/// terminal), then returns its result.
pub fn with_status<T: Send>(quiet: bool, status: impl Fn() -> String, work: impl FnOnce() -> T + Send) -> T {
    let term = Term::stderr();
    let show = !quiet && term.is_term();
    std::thread::scope(|s| {
        let job = s.spawn(work);
        while !job.is_finished() {
            if show {
                let width = term.size().1 as usize;
                let _ = term.clear_line();
                let _ = term.write_str(&fit(&status(), width.saturating_sub(1)));
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        if show {
            let _ = term.clear_line();
        }
        job.join().expect("worker thread panicked")
    })
}

pub fn free_space() {
    println!("Free space: {}", style(format_bytes(dusty_core::disk().free)).bold());
}

pub fn report(targets: &[Target]) {
    let max = targets.iter().map(|t| t.bytes).max().unwrap_or(1).max(1);
    for g in Group::ORDER {
        let items: Vec<&Target> = targets.iter().filter(|t| t.group == g).collect();
        if items.is_empty() {
            continue;
        }
        let sum: u64 = items.iter().map(|t| t.bytes).sum();
        println!("\n{} — {}  {}", style(g.name()).bold(), style(format_bytes(sum)).bold(), style(g.blurb()).dim());
        for t in items {
            let mark = if t.default_on { style("●").green() } else { style("○").dim() };
            let detail = t.detail.as_deref().map(|d| format!("  {}", style(fit(d, LABEL_WIDTH)).dim())).unwrap_or_default();
            println!("  {mark} {} {:>8}  {}{detail}", bar(t.bytes, max), format_bytes(t.bytes), fit(&t.label, LABEL_WIDTH));
        }
    }
    println!("\n{} selected by default   {} opt-in", style("●").green(), style("○").dim());
}

/// One line in the clean picker.
pub fn target_choice(t: &Target) -> String {
    format!("{:>8}  {}  {}", format_bytes(t.bytes), t.label, style(t.group.name()).dim())
}

/// One line in the `dusty large` picker.
pub fn large_choice(f: &LargeFile) -> String {
    format!("{:>8}  {}  {}", format_bytes(f.bytes), tildify(&f.path), style(format!("{} days since last use", f.days_unused())).dim())
}

pub fn suggestion_choice(s: &Suggestion) -> String {
    format!("{:>8}  {}  {}", format_bytes(s.bytes), s.what, style(tildify(&s.path)).dim())
}

/// The agents, whether each can start here, and the account each one used last.
pub fn agents(prefs: &dusty_ai::AgentPrefs) {
    let picked = prefs.agent().id;
    for a in dusty_ai::AGENTS {
        let mark = if a.id == picked { style("●").green() } else { style("○").dim() };
        let ready = match a.availability() {
            dusty_ai::Availability::Ready => match prefs.accounts.get(a.id) {
                Some(acc) => style(format!("ready · {acc}")).green().to_string(),
                None => style(format!("ready · uses your {}", a.plan)).green().to_string(),
            },
            dusty_ai::Availability::Missing(why) => style(why).yellow().to_string(),
        };
        println!("{mark} {:<16} {:<10} {ready}", a.name, style(a.lock.chip().to_lowercase()).dim());
    }
    hint("Pick one with: dusty explore --agent <id>");
}

/// The helper's suggestions, each with its reasons.
pub fn explore(r: &dusty_ai::ExploreReport, agent: &dusty_ai::Agent, verbose: bool) {
    if let Some(e) = &r.error {
        warn(&e.message(agent));
    }
    if r.asked {
        let mut used = vec![format!("Asked {}", agent.name)];
        used.extend(r.account.clone());
        if r.tokens > 0 {
            used.push(format!("{}k tokens", r.tokens.div_ceil(1000)));
        }
        if r.own_looks > 0 {
            used.push(format!("looked at {} thing(s) itself, read-only", r.own_looks));
        }
        hint(&used.join(" · "));
    }
    if !r.asked && r.error.is_none() {
        hint(&format!("Nothing new to ask about ({} folder(s) haven't grown since last time).", r.unchanged));
    }
    for s in &r.suggestions {
        let risk = match s.risk {
            Risk::Low => style("low risk").green(),
            Risk::Medium => style("check first").yellow(),
        };
        let seen = if s.seen_before { style(" · seen before").dim().to_string() } else { String::new() };
        println!("{}  {}  {risk}{seen}", style(format_bytes(s.bytes)).bold().green(), style(&s.what).bold());
        println!("  {}", style(tildify(&s.path)).dim());
        println!("  {}", s.why);
        if !s.comes_back.is_empty() {
            println!("  {} {}", style("Once it's gone:").dim(), s.comes_back);
        }
    }
    if let Some(n) = &r.note {
        hint(n);
    }
    if verbose {
        for why in &r.refused {
            println!("  {} {why}", style("turned down:").dim());
        }
    }
    if r.suggestions.is_empty() && r.error.is_none() && r.asked {
        done(&format!("{} looked at {} folder(s) and found nothing safe to toss.", agent.name, r.looked));
    }
}

/// "Clean 4 item(s): 2.1 GB deleted, 300 MB to the Trash?"
pub fn plan_question(plan: &CleanPlan) -> String {
    let parts: Vec<String> = [(Removal::Delete, "deleted"), (Removal::Trash, "to the Trash")]
        .into_iter()
        .map(|(r, what)| (plan.bytes(Some(r)), what))
        .filter(|(b, _)| *b > 0)
        .map(|(b, what)| format!("{} {what}", format_bytes(b)))
        .collect();
    let warn = plan.items.iter().filter(|i| i.target.wipes_data).count();
    let warn = if warn > 0 { format!(" ({warn} wipe real data)") } else { String::new() };
    format!("Clean {} item(s): {}{warn}?", plan.items.len(), if parts.is_empty() { "nothing measured".into() } else { parts.join(", ") })
}

pub fn clean_summary(r: &CleanReport, estimated: u64, free_before: u64, free_after: u64, trash: bool) {
    let e = &r.entry;
    if e.practice {
        done(&format!("Dry run: would free about {}", style(format_bytes(estimated)).green().bold()));
    } else if trash {
        done(&format!("Moved about {} to the Trash (empty it to get the space back; undo: dusty put-back {})", style(format_bytes(e.bytes)).green().bold(), e.id));
    } else {
        let freed = free_after.saturating_sub(free_before);
        done(&format!("Freed {} (estimated {})", style(format_bytes(freed)).green().bold(), format_bytes(estimated)));
    }
    errors(&r.errors, "could not be removed (in use or protected)");
    if let Some(h) = &r.history_error {
        warn(&format!("Couldn't write the history ({h})."));
    }
    println!("Free space now: {}", style(format_bytes(free_after)).bold());
}

pub fn trashed_files(r: &CleanReport) {
    errors(&r.errors, "stayed put");
    let e = &r.entry;
    if e.practice {
        done("Nothing was moved (dry run).");
    } else {
        done(&format!("Moved {} to the Trash. Undo: dusty put-back {}", format_bytes(e.bytes), e.id));
    }
}

pub fn history(entries: &[HistoryEntry]) {
    if entries.is_empty() {
        done(&format!("No cleans yet ({}).", tildify(&dusty_core::history::history_file())));
        return;
    }
    for e in entries.iter().take(20) {
        let tags: Vec<&str> = [Some(e.source.key()), e.practice.then_some("practice run"), e.policy.map(|p| p.key())].into_iter().flatten().collect();
        println!("{}  {:>8}  {} item(s)  {}", style(&e.date).bold(), style(format_bytes(e.bytes)).green(), e.items.len(), style(tags.join(", ")).dim());
        let back = e.restorable().len();
        if back > 0 {
            println!("  {}", style(format!("{back} in the Trash: dusty put-back {}", e.id)).cyan());
        }
        if !e.errors.is_empty() {
            println!("  {}", style(format!("{} left alone", e.errors.len())).dim());
        }
    }
    hint(&tildify(&dusty_core::history::history_file()));
}

pub fn put_back(r: &PutBackReport) {
    for p in &r.restored {
        done(&format!("Put back {}", tildify(p)));
    }
    for e in &r.errors {
        warn(e);
    }
    if r.restored.is_empty() && r.errors.is_empty() {
        hint("Nothing from that clean is still in the Trash.");
    }
}

pub fn manual_hints() {
    println!("\n{}", style("Manual cleanups worth checking").bold());
    for (what, how) in [
        ("Old simulator runtimes not managed by simctl", "Xcode → Settings → Components"),
        ("Docker.raw keeps its size after pruning on older Docker Desktop", "Settings → Resources → lower the disk limit"),
        ("Android system images / NDKs", "Android Studio → SDK Manager"),
        ("Time Machine local snapshots", "tmutil listlocalsnapshots /"),
    ] {
        println!("  {what}: {}", style(how).dim());
    }
}

pub fn done(msg: &str) {
    println!("{} {msg}", style("✓").green());
}

pub fn hint(msg: &str) {
    println!("{}", style(msg).dim());
}

pub fn warn(msg: &str) {
    eprintln!("{} {msg}", style("!").yellow());
}

pub fn error(msg: &str) {
    eprintln!("{} {msg}", style("✗").red());
}

fn errors(errs: &[String], what: &str) {
    if errs.is_empty() {
        return;
    }
    warn(&format!("{} item(s) {what}:", errs.len()));
    for e in errs.iter().take(8) {
        eprintln!("  {}", style(e).dim());
    }
}

fn bar(bytes: u64, max: u64) -> String {
    let n = ((bytes as f64 / max as f64) * BAR_WIDTH as f64).round() as usize;
    let n = n.clamp(usize::from(bytes > 0), BAR_WIDTH);
    format!("{}{}", "█".repeat(n), style("░".repeat(BAR_WIDTH - n)).dim())
}

/// Cuts `s` to `width` characters, ending with "…" when it was longer.
fn fit(s: &str, width: usize) -> String {
    if s.chars().count() <= width {
        return s.to_string();
    }
    let mut out: String = s.chars().take(width.saturating_sub(1)).collect();
    out.push('…');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fit_cuts_long_labels() {
        assert_eq!(fit("short", 10), "short");
        assert_eq!(fit("a long label", 6), "a lon…");
    }

    #[test]
    fn bar_shows_something_for_any_size() {
        console::set_colors_enabled(false);
        assert_eq!(bar(1, 1_000_000).chars().filter(|c| *c == '█').count(), 1);
        assert_eq!(bar(0, 10).chars().filter(|c| *c == '█').count(), 0);
        assert_eq!(bar(10, 10).chars().filter(|c| *c == '█').count(), BAR_WIDTH);
    }
}
