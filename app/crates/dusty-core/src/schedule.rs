//! Scheduled safe clean: a user LaunchAgent that runs Dusty headless once a week.
//!
//! `install_schedule` writes `~/Library/LaunchAgents/io.github.deuspheara.dusty.scheduled-clean.plist`,
//! which runs `<Dusty binary> --scheduled-clean --policy=<policy>` at the chosen time (launchd runs a
//! missed slot when the Mac wakes up). `main.rs` must call [`run_scheduled_clean_main`] when it sees
//! [`SCHEDULED_CLEAN_FLAG`], before starting the UI.
//!
//! A scheduled clean only takes safe picks: the default selection minus anything re-downloadable or
//! data-wiping, and leaves project build folders touched in the last week alone. Every run is
//! recorded in the history (source `scheduled`); output goes to
//! `~/Library/Application Support/Dusty/scheduled.log`.

use super::history::{support_dir, HistorySource};
use super::trash::CleanPolicy;
use super::{default_roots, execute, exists, format_bytes, home, run_s, scan_all, tildify, write_atomic, CleanPlan, CleanReport, Group, ScanOptions, Target};
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

pub const SCHEDULE_LABEL: &str = "io.github.deuspheara.dusty.scheduled-clean";
/// Command-line flag that runs one scheduled clean headless and exits.
pub const SCHEDULED_CLEAN_FLAG: &str = "--scheduled-clean";
/// Project build folders changed this recently are left alone by a scheduled clean.
pub const SCHEDULED_KEEP_RECENT_DAYS: u64 = 7;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Schedule {
    /// 0 = Sunday … 6 = Saturday (launchd's numbering).
    pub weekday: u8,
    pub hour: u8,
    pub minute: u8,
    pub policy: CleanPolicy,
}

impl Default for Schedule {
    /// Weekly, Sunday 11:00, caches deleted and everything else to the Trash.
    fn default() -> Self {
        Schedule { weekday: 0, hour: 11, minute: 0, policy: CleanPolicy::TrashUnlessCache }
    }
}

pub fn launch_agent_path() -> PathBuf {
    home().join("Library/LaunchAgents").join(format!("{SCHEDULE_LABEL}.plist"))
}

pub fn scheduled_log() -> PathBuf {
    support_dir().join("scheduled.log")
}

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

/// The LaunchAgent plist for running `exe` on schedule `s`. Pure.
pub fn launch_agent_plist(exe: &Path, s: &Schedule) -> String {
    let args = [exe.to_string_lossy().into_owned(), SCHEDULED_CLEAN_FLAG.to_string(), format!("--policy={}", s.policy.key())];
    let args: String = args.iter().map(|a| format!("\n        <string>{}</string>", xml_escape(a))).collect();
    let log = xml_escape(&scheduled_log().to_string_lossy());
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>{SCHEDULE_LABEL}</string>
    <key>ProgramArguments</key>
    <array>{args}
    </array>
    <key>StartCalendarInterval</key>
    <dict>
        <key>Weekday</key>
        <integer>{}</integer>
        <key>Hour</key>
        <integer>{}</integer>
        <key>Minute</key>
        <integer>{}</integer>
    </dict>
    <key>ProcessType</key>
    <string>Background</string>
    <key>LowPriorityIO</key>
    <true/>
    <key>Nice</key>
    <integer>10</integer>
    <key>StandardOutPath</key>
    <string>{log}</string>
    <key>StandardErrorPath</key>
    <string>{log}</string>
</dict>
</plist>
"#,
        s.weekday.min(6),
        s.hour.min(23),
        s.minute.min(59)
    )
}

fn launchctl(args: &[&str]) -> super::RunResult {
    #[cfg(test)]
    {
        let _ = args;
        super::RunResult { ok: true, stdout: String::new(), stderr: String::new() }
    }
    #[cfg(not(test))]
    run_s("/bin/launchctl", args, 30)
}

fn domain() -> String {
    format!("gui/{}", unsafe { libc::getuid() })
}

/// Installs (or replaces) the weekly clean for the running Dusty binary. The app should live in
/// /Applications or ~/Applications: if it is moved later, the schedule must be installed again.
pub fn install_schedule(s: &Schedule) -> Result<PathBuf, String> {
    let exe = std::env::current_exe().map_err(|e| format!("couldn't find Dusty's own binary ({e})"))?;
    let exe = std::fs::canonicalize(&exe).unwrap_or(exe);
    install_schedule_for(&exe, s)
}

/// [`install_schedule`] for a given binary.
pub fn install_schedule_for(exe: &Path, s: &Schedule) -> Result<PathBuf, String> {
    if exe.to_string_lossy().contains("/AppTranslocation/") {
        return Err("Move Dusty to your Applications folder first: macOS runs it from a temporary copy right now.".into());
    }
    if !exe.is_file() {
        return Err(format!("{} isn't there", tildify(exe)));
    }
    let f = launch_agent_path();
    let _ = std::fs::create_dir_all(support_dir());
    write_atomic(&f, launch_agent_plist(exe, s).as_bytes())?;
    // replace a loaded copy (bootout fails harmlessly when none is loaded)
    let target = format!("{}/{SCHEDULE_LABEL}", domain());
    launchctl(&["bootout", &target]);
    let r = launchctl(&["bootstrap", &domain(), &f.to_string_lossy()]);
    if !r.ok {
        let legacy = launchctl(&["load", "-w", &f.to_string_lossy()]);
        if !legacy.ok {
            return Err(format!("saved {}, but launchd didn't take it ({})", tildify(&f), r.stderr.trim()));
        }
    }
    Ok(f)
}

/// Turns the schedule off: unloads it and removes Dusty's own LaunchAgent file (only that file).
/// Returns `false` when there was no schedule.
pub fn remove_schedule() -> Result<bool, String> {
    let f = launch_agent_path();
    let existed = exists(&f);
    launchctl(&["bootout", &format!("{}/{SCHEDULE_LABEL}", domain())]);
    if existed {
        std::fs::remove_file(&f).map_err(|e| format!("couldn't remove {} ({e})", tildify(&f)))?;
    }
    Ok(existed)
}

/// True when the weekly clean is installed.
pub fn is_scheduled() -> bool {
    launch_agent_path().is_file()
}

/// The installed schedule, read back from the LaunchAgent file.
#[cfg(test)]
pub fn current_schedule() -> Option<Schedule> {
    let v = super::read_plist_json(&launch_agent_path())?;
    let cal = v.get("StartCalendarInterval")?;
    let n = |k: &str| cal.get(k).and_then(|x| x.as_u64()).map(|x| x as u8);
    let policy = v
        .get("ProgramArguments")?
        .as_array()?
        .iter()
        .filter_map(|a| a.as_str()?.strip_prefix("--policy=").and_then(CleanPolicy::from_key))
        .next()
        .unwrap_or_default();
    Some(Schedule { weekday: n("Weekday").unwrap_or(0), hour: n("Hour").unwrap_or(0), minute: n("Minute").unwrap_or(0), policy })
}

/// `--policy=<delete|trash|trash-unless-cache>` from the command line (default: trash-unless-cache).
pub fn policy_from_args<I: IntoIterator<Item = String>>(args: I) -> CleanPolicy {
    args.into_iter().filter_map(|a| a.strip_prefix("--policy=").and_then(CleanPolicy::from_key)).last().unwrap_or_default()
}

/// What a scheduled clean takes from a scan: the default selection, minus anything that costs a
/// download or wipes data. Pure.
pub fn scheduled_selection(targets: &[Target]) -> Vec<Target> {
    targets.iter().filter(|t| t.default_on && t.group != Group::Redownload && !t.wipes_data).cloned().collect()
}

/// Scans with the default roots and cleans the safe picks with `policy`, recording the run in the
/// history. Blocking (minutes): it is meant for the headless `--scheduled-clean` run.
pub fn run_scheduled_clean(policy: CleanPolicy) -> CleanReport {
    let cancel = AtomicBool::new(false);
    let opts = ScanOptions { roots: default_roots(), keep_recent_days: SCHEDULED_KEEP_RECENT_DAYS, min_size_mb: 1 };
    let targets = scan_all(&opts, &|_| {}, &cancel);
    let plan = CleanPlan::new(&scheduled_selection(&targets), policy, HistorySource::Scheduled, false);
    execute(&plan, &mut |_| {}, &cancel)
}

/// Entry point for `main.rs`: runs [`run_scheduled_clean`] with the policy from `args`, prints a
/// short report (it lands in the scheduled log) and returns the process exit code.
pub fn run_scheduled_clean_main<I: IntoIterator<Item = String>>(args: I) -> i32 {
    let policy = policy_from_args(args);
    let started = super::history::iso_utc(super::now_secs());
    let r = run_scheduled_clean(policy);
    println!(
        "{started} scheduled clean ({}): {} items, {} ({} deleted, {} to the Trash){}",
        policy.key(),
        r.entry.items.len(),
        format_bytes(r.entry.bytes),
        format_bytes(r.entry.bytes_deleted()),
        format_bytes(r.entry.bytes_trashed()),
        if r.entry.practice { ", practice run" } else { "" }
    );
    for e in &r.errors {
        println!("  {e}");
    }
    if let Some(e) = &r.history_error {
        println!("  history: {e}");
    }
    0
}

#[cfg(test)]
mod tests {
    use super::super::tests::FakeHome;
    use super::super::Guard;
    use super::*;

    #[test]
    fn install_and_remove_in_a_fake_home() {
        let fake = FakeHome::new("schedule");
        assert!(!is_scheduled());
        let exe = fake.path.join("Applications/Dusty.app/Contents/MacOS/Dusty");
        super::super::tests::touch(&exe, 10);
        let s = Schedule { weekday: 3, hour: 9, minute: 30, policy: CleanPolicy::TrashAll };
        let f = install_schedule_for(&exe, &s).unwrap();
        assert_eq!(f, fake.path.join("Library/LaunchAgents/io.github.deuspheara.dusty.scheduled-clean.plist"));
        assert!(is_scheduled());
        let lint = run_s("/usr/bin/plutil", &["-lint", &f.to_string_lossy()], 10);
        assert!(lint.ok, "{}", lint.stdout);
        assert_eq!(current_schedule(), Some(s));
        assert!(install_schedule_for(&fake.path.join("nope"), &s).is_err());
        assert!(install_schedule_for(Path::new("/private/var/folders/x/AppTranslocation/Dusty"), &s).unwrap_err().contains("Applications"));
        assert!(remove_schedule().unwrap());
        assert!(!is_scheduled());
        assert!(!remove_schedule().unwrap());
    }

    #[test]
    fn args_and_selection() {
        assert_eq!(policy_from_args(["x".to_string(), "--policy=delete".into()]), CleanPolicy::DeleteAll);
        assert_eq!(policy_from_args(Vec::<String>::new()), CleanPolicy::TrashUnlessCache);
        assert_eq!(policy_from_args(["--policy=bogus".to_string()]), CleanPolicy::TrashUnlessCache);
        let t = |id: &str, g, on| Target { id: id.into(), label: id.into(), group: g, default_on: on, bytes: 1, paths: vec![], commands: vec![], detail: None, guard: Guard::None, wipes_data: id.starts_with("docker-volumes") };
        let picks = scheduled_selection(&[
            t("build", Group::Build, true),
            t("opt-in", Group::Cache, false),
            t("re", Group::Redownload, true),
            t("docker-volumes ⚠ data", Group::Docker, true),
            t("logs", Group::Junk, true),
        ]);
        assert_eq!(picks.iter().map(|t| t.id.as_str()).collect::<Vec<_>>(), ["build", "logs"]);
    }
}
