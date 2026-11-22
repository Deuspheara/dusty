//! The app: checkup → results → (choose items) → confirm → cleaning → done, plus the tools reached from home
//! (uninstall apps, big & old files, what's using space, history, settings) in `ui/tools.rs`.

mod components;
pub mod icons;
mod screens;
mod theme;
mod tools;
mod update_sheet;

use std::collections::{HashMap, HashSet};
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use gpui::prelude::FluentBuilder;
use futures::StreamExt;
use gpui::*;

use crate::dusty::{Dusty, Mood};
use icons::{icon, icon_raster, Ico};
use dusty_core::history::HistorySource;
use dusty_core::summary::LastScan;
use dusty_core::{format_bytes, CleanPlan, CleanPolicy, CleanProgress, CleanReport, Disk, Group, ScanOptions, ScanProgress, Target};
use crate::platform::{self, haptic, Haptic};
use crate::updater::{self, Action as UpdAction, Phase as UpdPhase};
use components::*;
use theme::*;
use update_sheet::*;

// ───────────────────────── keyboard ─────────────────────────

actions!(dusty_keys, [Primary, Back, FocusNext, FocusPrev, ToggleFocused, Rescan, UndoIgnore]);

/// Return = the screen's primary button, Esc = close the sheet / go back, ↑ ↓ + Space on Choose items, ⌘R = rescan,
/// ⌘Z = undo "Never clean this" while its toast is up.
pub fn bind_keys(cx: &mut App) {
    let ctx = Some("Dusty");
    cx.bind_keys([
        KeyBinding::new("enter", Primary, ctx),
        KeyBinding::new("escape", Back, ctx),
        KeyBinding::new("down", FocusNext, ctx),
        KeyBinding::new("up", FocusPrev, ctx),
        KeyBinding::new("space", ToggleFocused, ctx),
        KeyBinding::new("cmd-r", Rescan, ctx),
        KeyBinding::new("cmd-z", UndoIgnore, ctx),
    ]);
}

// ───────────────────────── menu bar extra ─────────────────────────

/// How often the menu bar's free space is measured again.
const STATUS_REFRESH: Duration = Duration::from_secs(180);

/// "🐾 124 GB": the free space, in the menu bar extra.
pub fn refresh_status_title() {
    platform::status_item_title(&format!("🐾 {}", format_bytes(dusty_core::disk().free)));
}

// ───────────────────────── state ─────────────────────────

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Screen {
    Home,
    Scanning,
    Results,
    Pick,
    Cleaning,
    Done,
    /// tools reached from home (see `ui/tools.rs`)
    Apps,
    AppDetail,
    Large,
    Usage,
    History,
    Settings,
}

impl Screen {
    fn is_tool(self) -> bool {
        matches!(self, Screen::Apps | Screen::AppDetail | Screen::Large | Screen::Usage | Screen::History | Screen::Settings)
    }
}

/// What the confirm sheet is asking about.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum ConfirmKind {
    Clean,
    Uninstall,
    TrashFiles,
}

struct ScanJob {
    progress: Arc<Mutex<ScanProgress>>,
    cancel: Arc<AtomicBool>,
    seen: usize,
    total: u64,
    shown: f64,
    chips: Vec<(String, String, u64, Group)>,
    line_i: usize,
    line_t: Instant,
}


struct Flight {
    idx: usize,
    t0: Instant,
    from: Point<Pixels>,
}

struct Popup {
    text: SharedString,
    at: Point<Pixels>,
    t0: Instant,
}

struct CleanJob {
    items: Vec<Target>,
    /// `dusty_core::execute` reports each finished item's errors here, in order
    progress: Arc<Mutex<CleanProgress>>,
    /// set when the clean (and its history line) is done
    report: Arc<Mutex<Option<CleanReport>>>,
    dry: bool,
    before: Disk,
    next: usize,
    flight: Option<Flight>,
    eaten: u64,
    shown: f64,
    /// eased 0..1 so the bar glides instead of jumping per item
    shown_progress: f32,
    failed: Vec<(String, String)>,
    ok_count: usize,
    finished_at: Option<Instant>,
    popups: Vec<Popup>,
    shake_until: Option<Instant>,
}

/// "Put back" for things a clean, uninstall or big-files run moved to the Trash.
#[derive(Clone, PartialEq, Debug)]
enum PutBack {
    No,
    Busy,
    Done,
}

struct Particle {
    x: f32,
    y: f32,
    vx: f32,
    vy: f32,
    g: f32,
    size: f32,
    color: u32,
    life: f32,
    max: f32,
    round: bool,
}

/// Splash timeline (seconds): badge pops and dust sweeps in, then an iris opens on Dusty.
/// Dusty beside the totals on Results and Choose items.
const RESULTS_DUSTY: f32 = 100.;
/// How long the old screen takes to fade out before the next one comes in (ms).
const LEAVE_MS: u64 = 110;
const SPLASH_HOLD: f32 = 0.32;
const SPLASH_IRIS: f32 = 0.42;
const SPLASH_END: f32 = SPLASH_HOLD + SPLASH_IRIS;
/// How long a toast stays up (ms). One with an Undo stays longer, so there's time to reach it.
const TOAST_MS: u64 = 2600;
const UNDO_TOAST_MS: u64 = 5000;

/// "Never clean this": the row fades and slides out (`motion::EXIT`), then its slot closes (`motion::BASE`).
const REMOVE_MS: u64 = motion::EXIT + motion::BASE;

/// What "Undo" on the "Dusty will leave it alone" toast puts back.
struct IgnoreUndo {
    target: Target,
    /// where it sat in the list, and whether it was ticked
    index: usize,
    selected: bool,
    /// the exact entry added to the ignore file (`id:<item>`), or None when it was already there
    line: Option<String>,
}

/// How long an item flies from the plate into Dusty's mouth (s). Shorter under Reduce motion.
fn flight_secs() -> f32 {
    if reduced() {
        0.3
    } else {
        0.8
    }
}

pub struct DustyApp {
    /// launch time while the splash / home entrance is playing
    intro: Option<Instant>,
    intro_kicked: bool,
    screen: Screen,
    nav: u64,
    nav_t: Instant,
    /// when the current screen was entered (drives its count-ups and bar fills); unlike `nav_t` a sheet opening doesn't touch it
    screen_t: Instant,
    /// bumped once per scan: Results' entrance (cards, count-up) plays then, not on every return to it
    results_gen: u64,
    /// the screen being faded out just before the new one fades in, and since when
    leaving: Option<(Screen, Instant)>,
    /// the old screen's entry time (its count-ups stay finished while it fades out) and Dusty's size before / after the switch
    leave_screen_t: Instant,
    leave_sizes: Option<(f32, f32)>,
    last_side: f32,
    /// last seen layout (wide or not) and when it changed: the content crossfades over the switch
    last_wide: bool,
    layout_t: Instant,
    /// when the confirm sheet opened
    sheet_t: Instant,
    dusty: Entity<Dusty>,
    disk: Disk,
    targets: Vec<Target>,
    selected: HashSet<String>,
    expanded: HashSet<Group>,
    scan: Option<ScanJob>,
    clean: Option<CleanJob>,
    confirm: bool,
    confirm_kind: ConfirmKind,
    /// when the confirm sheet started closing: it plays its exit before it is removed
    confirm_closing: Option<Instant>,
    /// when each group last opened or closed on Results (drives the expand / collapse animation)
    group_anim: HashMap<Group, (Instant, u64)>,
    anim_gen: u64,
    /// "Free up …" on Choose items, eased toward the selection so it counts instead of jumping
    pick_shown: f64,
    dry_run: bool,
    bubble: SharedString,
    toast: Option<(SharedString, Instant)>,
    /// the icon at the toast's left, if any
    toast_icon: Option<Ico>,
    /// the toast on screen offers "Undo" for the last "Never clean this"
    toast_undo: bool,
    undo: Option<IgnoreUndo>,
    /// rows on their way out after "Never clean this", by target id
    removing: HashMap<String, Instant>,
    /// the item row under the mouse (shows its Finder / never-clean actions)
    hovered_row: Option<String>,
    /// keyboard focus on Choose items (target id), and whether its ring shows (keyboard, not mouse)
    pick_focus: Option<String>,
    pick_ring: bool,
    pick_scroll: ScrollHandle,
    focus: FocusHandle,
    last_scan: Option<LastScan>,
    particles: Vec<Particle>,
    last: Instant,
    rng: u64,
    done_freed: u64,
    done_estimated: bool,
    done_disk: Disk,
    /// the disk right before cleaning, for the before → after bar
    done_before: Disk,
    /// bytes the clean moved to the Trash (logs, re-downloadables): not free until the Trash is emptied
    done_trashed: u64,
    /// the clean's history entry, when something can be put back from the Trash
    done_entry: Option<String>,
    done_put_back: PutBack,
    /// the new tools' state (`ui/tools.rs`)
    tools: tools::Tools,
    /// window size seen by the last render (lays out the treemap, sizes Dusty on short windows)
    view: (f32, f32),
    hints_done: HashSet<usize>,
    /// update sheet: last `updater::generation()` seen, whether it's up, when it opened / started closing,
    /// the phase it shows (kept while it slides away) and its eased progress bar
    upd_gen: u64,
    upd_visible: bool,
    upd_t: Instant,
    upd_closing: Option<Instant>,
    upd_phase: UpdPhase,
    upd_bar: f32,
    upd_version: String,
    _appearance: Subscription,
}

const HINTS: &[(&str, &str)] = &[
    ("Old simulator runtimes", "Xcode → Settings → Components, delete the ones you don't need"),
    ("Docker.raw keeps its size", "Docker Desktop → Settings → Resources, lower the disk limit"),
    ("Android SDK leftovers", "Android Studio → SDK Manager, unused images and NDKs"),
    ("Time Machine local snapshots", "Run: tmutil listlocalsnapshots /"),
    ("Old backups", "Things like ~/.codex.backup-* can go if you don't need them"),
];

impl DustyApp {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        // light or dark follows System Settings live; DUSTY_APPEARANCE=dark|light pins it (screenshots)
        let pinned = match std::env::var("DUSTY_APPEARANCE").as_deref() {
            Ok("dark") => Some(WindowAppearance::Dark),
            Ok("light") => Some(WindowAppearance::Light),
            _ => None,
        };
        set_appearance(pinned.unwrap_or(window.appearance()));
        refresh_reduced();
        cx.set_reduce_motion(reduced()); // gpui's own animations follow it too
        let appearance = cx.observe_window_appearance(window, move |_, window, cx| {
            if pinned.is_none() && set_appearance(window.appearance()) {
                cx.notify();
            }
        });
        let disk = dusty_core::disk();
        let dusty = cx.new(|_| Dusty::new(250.));
        let mut app = DustyApp {
            // Reduce motion skips the splash and the home entrance
            intro: if reduced() { None } else { Some(Instant::now()) },
            intro_kicked: false,
            screen: Screen::Home,
            nav: 0,
            nav_t: Instant::now(),
            screen_t: Instant::now(),
            results_gen: 0,
            leaving: None,
            leave_screen_t: Instant::now(),
            leave_sizes: None,
            last_side: 250.,
            last_wide: false,
            layout_t: Instant::now() - Duration::from_secs(5),
            sheet_t: Instant::now(),
            dusty,
            disk,
            targets: vec![],
            selected: HashSet::new(),
            expanded: HashSet::new(),
            scan: None,
            clean: None,
            confirm: false,
            confirm_kind: ConfirmKind::Clean,
            confirm_closing: None,
            group_anim: HashMap::new(),
            anim_gen: 0,
            pick_shown: 0.,
            dry_run: false,
            bubble: "".into(),
            toast: None,
            toast_icon: None,
            toast_undo: false,
            undo: None,
            removing: HashMap::new(),
            hovered_row: None,
            pick_focus: None,
            pick_ring: false,
            pick_scroll: ScrollHandle::new(),
            focus: cx.focus_handle(),
            last_scan: None, // read by enter_home below
            particles: vec![],
            last: Instant::now(),
            rng: 0x2545F4914F6CDD1D,
            done_freed: 0,
            done_estimated: false,
            done_disk: disk,
            done_before: disk,
            done_trashed: 0,
            done_entry: None,
            done_put_back: PutBack::No,
            tools: tools::Tools::default(),
            view: (440., 800.),
            hints_done: HashSet::new(),
            upd_gen: 0,
            upd_visible: false,
            upd_t: Instant::now(),
            upd_closing: None,
            upd_phase: UpdPhase::Idle,
            upd_bar: 0.,
            upd_version: String::new(),
            _appearance: appearance,
        };
        app.focus.focus(window, cx);
        app.enter_home(cx);
        app.open_from_env(cx);
        if let Some(v) = updater::just_updated() {
            app.toast_with(Ico::Party, &format!("Dusty is now {}", updater::pretty(&v)));
        }
        // Sparkle talks to `updater` and the menu bar extra talks to `platform` from AppKit callbacks;
        // pick up their news here (and re-measure the menu bar's free space now and then)
        let mut status_t = Instant::now();
        let handle = window.window_handle();
        let mut wake = crate::wake::listen();
        cx.spawn(async move |this, cx| loop {
            // sleep until a callback pings (or the menu bar title is due), instead of polling
            let timer = cx.background_executor().timer(STATUS_REFRESH.saturating_sub(status_t.elapsed()));
            let _ = futures::future::select(wake.next(), timer).await;
            while wake.try_recv().is_ok() {} // one pass covers a burst of pings
            let g = updater::generation();
            if status_t.elapsed() > STATUS_REFRESH {
                status_t = Instant::now();
                refresh_status_title();
            }
            let req = platform::take_status_request();
            if matches!(req, platform::StatusRequest::Open | platform::StatusRequest::CheckUp) {
                cx.update(|cx| cx.activate(true));
                let _ = handle.update(cx, |_, window, _| window.activate_window());
            }
            let alive = this.update(cx, |app, cx| {
                if app.upd_gen != g {
                    app.upd_gen = g;
                    cx.notify();
                }
                match req {
                    platform::StatusRequest::CheckUp => app.menu_checkup(cx),
                    platform::StatusRequest::Quit => cx.quit(),
                    _ => {}
                }
            });
            if alive.is_err() {
                break;
            }
        })
        .detach();
        app
    }

    /// Widest the content may grow: the split screens use the room, the rest stay a readable column.
    fn col_max(&self, screen: Screen) -> f32 {
        match (self.is_wide(), screen) {
            (true, Screen::Scanning | Screen::Cleaning) => 720.,
            (true, Screen::Home | Screen::Results | Screen::Pick | Screen::Large) => 1100.,
            (true, _) => 900.,
            _ => MAX_W,
        }
    }

    /// When the screen on view was entered (the old screen's count-ups stay finished while it fades out).
    pub(super) fn st(&self) -> Instant {
        if self.leaving.is_some() {
            self.leave_screen_t
        } else {
            self.screen_t
        }
    }

    /// Seconds since launch, while the intro plays.
    fn intro_t(&self) -> Option<f32> {
        // DUSTY_SPLASH_SLOW=10 plays the intro 10× slower (for checking it frame by frame)
        static SLOW: std::sync::OnceLock<f32> = std::sync::OnceLock::new();
        let slow = *SLOW.get_or_init(|| std::env::var("DUSTY_SPLASH_SLOW").ok().and_then(|v| v.parse::<f32>().ok()).unwrap_or(1.).max(0.1));
        self.intro.map(|t| t.elapsed().as_secs_f32() / slow)
    }

    /// Entrance progress (0..1) of a home element that starts `delay` s after the iris opens.
    fn enter_k(&self, delay: f32) -> f32 {
        match self.intro_t() {
            None => 1.,
            Some(t) => ((t - SPLASH_HOLD - delay) / (motion::ENTER as f32 / 1000.)).clamp(0., 1.),
        }
    }

    /// Wrap an element in its entrance: fade in and rise with a little overshoot.
    fn enter(&self, el: impl IntoElement, delay: f32) -> Div {
        let k = self.enter_k(delay);
        div().w_full().flex().flex_col().items_center().opacity((k * 1.6).min(1.)).mt(px(motion::RISE * 1.5 * (1. - back_out(k)))).child(el)
    }

    fn used(&self) -> f32 {
        1. - self.disk.free as f32 / self.disk.total.max(1) as f32
    }

    fn go(&mut self, s: Screen, cx: &mut Context<Self>) {
        // the old screen fades out first (only the checkup flow's screens are safe to draw after their state moved on)
        let calm = |x: Screen| x.is_tool() || matches!(x, Screen::Home | Screen::Scanning | Screen::Results | Screen::Pick | Screen::Done);
        self.leaving = (self.screen != s && calm(self.screen) && !reduced() && self.intro.is_none()).then(|| (self.screen, Instant::now()));
        if self.leaving.is_some() {
            self.leave_screen_t = self.screen_t;
            let new = self.dusty.read(cx).size();
            self.leave_sizes = Some((self.last_side, new));
        }
        self.screen = s;
        self.nav += 1;
        self.nav_t = Instant::now();
        self.screen_t = self.nav_t;
        cx.notify();
    }

    fn dusty(&self, cx: &mut Context<Self>, f: impl FnOnce(&mut Dusty)) {
        self.dusty.update(cx, |d, cx| {
            f(d);
            cx.notify();
        });
    }

    fn rand(&mut self) -> f32 {
        self.rng ^= self.rng << 13;
        self.rng ^= self.rng >> 7;
        self.rng ^= self.rng << 17;
        (self.rng % 10_000) as f32 / 10_000.
    }

    fn enter_home(&mut self, cx: &mut Context<Self>) {
        refresh_reduced();
        cx.set_reduce_motion(reduced()); // gpui's own animations follow it too
        self.disk = dusty_core::disk();
        self.last_scan = dusty_core::summary::load_last_scan();
        refresh_status_title();
        let used = self.used();
        let (mood, line) = if used > 0.92 {
            (Mood::Sad, "I'm soooo stuffed. Help?")
        } else if used > 0.75 {
            (Mood::Think, "Feeling a bit… heavy today.")
        } else {
            (Mood::Idle, "Light as a feather. But I smell crumbs…")
        };
        self.bubble = line.into();
        let side = home_dusty_side(self.view.0, self.view.1);
        self.dusty(cx, |d| {
            d.set_size(side);
            d.st.set(mood, None);
            d.st.dirt = used;
            d.st.chub_target = used;
            d.st.look_at(None);
        });
        self.go(Screen::Home, cx);
    }

    // ── scanning ──

    fn start_scan(&mut self, cx: &mut Context<Self>) {
        if self.just_arrived() && self.screen != Screen::Home {
            return;
        }
        let progress = Arc::new(Mutex::new(ScanProgress::default()));
        let cancel = Arc::new(AtomicBool::new(false));
        self.scan = Some(ScanJob { progress: progress.clone(), cancel: cancel.clone(), seen: 0, total: 0, shown: 0., chips: vec![], line_i: 0, line_t: Instant::now() });
        self.bubble = "*sniff sniff* Let me have a look around…".into();
        self.dusty(cx, |d| {
            d.set_size(210.);
            d.st.set(Mood::Think, None);
        });
        self.go(Screen::Scanning, cx);
        let opts = ScanOptions { roots: dusty_core::default_roots(), keep_recent_days: 0, min_size_mb: 1 };
        let task = cx.background_spawn(async move { dusty_core::scan_all(&opts, &|e| progress.lock().unwrap().apply(e), &cancel) });
        cx.spawn(async move |this, cx| {
            let targets = task.await;
            let _ = this.update(cx, |app, cx| app.scan_done(targets, cx));
        })
        .detach();
    }

    fn scan_done(&mut self, targets: Vec<Target>, cx: &mut Context<Self>) {
        let cancelled = self.scan.as_ref().map(|s| s.cancel.load(std::sync::atomic::Ordering::Relaxed)).unwrap_or(true);
        self.scan = None;
        if cancelled {
            return;
        }
        self.selected = targets.iter().filter(|t| t.default_on).map(|t| t.id.clone()).collect();
        self.expanded = HashSet::new();
        self.results_gen += 1;
        self.targets = targets;
        if let Some(problem) = dusty_core::user_catalog_problem() {
            self.toast(problem);
        }
        self.disk = dusty_core::disk();
        self.removing.clear();
        self.undo = None;
        self.pick_focus = None;
        self.pick_ring = false;
        let safe = self.safe_bytes();
        // `scan_all` already saved this checkup's summary (last-scan.json); the home screen reads it back
        self.last_scan = dusty_core::summary::load_last_scan();
        self.bubble = if safe > 0 { format!("{} is safe to toss right now. No downloads needed!", format_bytes(safe)).into() } else { "Spotless already. I'm proud of you.".into() };
        self.dusty(cx, |d| {
            d.set_size(RESULTS_DUSTY);
            d.st.set(Mood::Happy, Some(1.6));
        });
        self.burst_center(70, cx);
        self.go(Screen::Results, cx);
    }

    fn cancel_scan(&mut self, cx: &mut Context<Self>) {
        if let Some(s) = &self.scan {
            s.cancel.store(true, std::sync::atomic::Ordering::Relaxed);
        }
        self.scan = None;
        self.enter_home(cx);
    }

    fn tick_scan(&mut self, dt: f32, cx: &mut Context<Self>) {
        let mut big_find = None;
        let mut line_change = None;
        let mut fed: Vec<u32> = vec![];
        if let Some(job) = &mut self.scan {
            let p = job.progress.lock().unwrap();
            for t in p.found.iter().skip(job.seen) {
                job.total += t.bytes;
                if t.bytes > 200 * 1024 * 1024 {
                    job.chips.push((t.id.clone(), t.label.clone(), t.bytes, t.group));
                    fed.push(tone(t.group).base);
                }
                if t.bytes > 4 * 1024 * 1024 * 1024 {
                    big_find = Some(t.label.clone());
                }
            }
            job.seen = p.found.len();
            let msg = p.message.clone();
            drop(p);
            if job.chips.len() > 12 {
                let n = job.chips.len() - 12;
                job.chips.drain(..n);
            }
            job.shown += (job.total as f64 - job.shown) * (1. - (-dt as f64 * 6.).exp());
            if job.line_t.elapsed() > Duration::from_millis(2600) {
                job.line_i += 1;
                job.line_t = Instant::now();
                const LINES: &[&str] = &["*sniff sniff* …build folders, I can tell.", "Checking with git so I never touch your real code.", "Ooh, Xcode left crumbs everywhere.", "Peeking at caches. Politely."];
                line_change = Some(if job.line_i % 2 == 0 && !msg.is_empty() { msg } else { LINES[job.line_i % LINES.len()].to_string() });
            }
        }
        if !fed.is_empty() {
            self.feed_cat(fed);
        }
        if let Some(l) = line_change {
            self.bubble = l.into();
        }
        if let Some(label) = big_find {
            self.bubble = format!("Whoa, {label}! That's a big one.").into();
            self.dusty(cx, |d| d.st.set(Mood::Wow, Some(0.7)));
        }
    }

    // ── selection ──

    /// Targets still in the list (not on their way out after "Never clean this").
    fn live(&self) -> impl Iterator<Item = &Target> {
        self.targets.iter().filter(|t| !self.removing.contains_key(&t.id))
    }
    fn safe_ids(&self) -> HashSet<String> {
        self.live().filter(|t| t.default_on).map(|t| t.id.clone()).collect()
    }
    fn safe_bytes(&self) -> u64 {
        self.live().filter(|t| t.default_on).map(|t| t.bytes).sum()
    }
    fn selected_targets(&self) -> Vec<Target> {
        self.targets.iter().filter(|t| self.selected.contains(&t.id)).cloned().collect()
    }
    fn selected_bytes(&self) -> u64 {
        self.targets.iter().filter(|t| self.selected.contains(&t.id)).map(|t| t.bytes).sum()
    }

    fn sync_pick_dusty(&mut self, cx: &mut Context<Self>) {
        let total: u64 = self.live().map(|t| t.bytes).sum::<u64>().max(1);
        let k = self.selected_bytes() as f32 / total as f32;
        self.dusty(cx, |d| {
            d.st.dirt = 0.85 - k * 0.85;
            d.st.chub_target = (1. - k).max(0.) * 0.9;
        });
    }

    fn toggle(&mut self, id: &str, cx: &mut Context<Self>) {
        let Some(t) = self.targets.iter().find(|t| t.id == id).cloned() else { return };
        if self.selected.remove(id) {
            self.bubble = "Okay, I'll keep that one.".into();
        } else {
            self.selected.insert(id.to_string());
            if t.wipes_data {
                self.bubble = "Careful: that one wipes real data, not just cache.".into();
                self.toast_with(Ico::Warning, "This wipes real data, not just a cache.");
                self.dusty(cx, |d| d.st.set(Mood::Wow, Some(1.2)));
            } else if t.group == Group::Redownload {
                self.bubble = format!("That one costs a {} download later.", format_bytes(t.bytes)).into();
                self.dusty(cx, |d| d.st.set(Mood::Think, Some(1.)));
            } else {
                self.bubble = "Crumbs, begone!".into();
                self.dusty(cx, |d| d.st.poke(0.));
            }
        }
        self.sync_pick_dusty(cx);
        cx.notify();
    }

    fn toggle_group(&mut self, g: Group, cx: &mut Context<Self>) {
        if !self.expanded.remove(&g) {
            self.expanded.insert(g);
        }
        self.anim_gen += 1;
        self.group_anim.insert(g, (Instant::now(), self.anim_gen));
        cx.notify();
    }

    fn close_confirm(&mut self, cx: &mut Context<Self>) {
        if self.confirm && self.confirm_closing.is_none() {
            self.confirm_closing = Some(Instant::now());
            cx.notify();
        }
    }

    fn toast(&mut self, msg: &str) {
        self.toast = Some((msg.to_string().into(), Instant::now()));
        self.toast_icon = None;
        self.toast_undo = false;
    }

    fn toast_with(&mut self, i: Ico, msg: &str) {
        self.toast(msg);
        self.toast_icon = Some(i);
    }

    fn toast_ms(&self) -> u64 {
        if self.toast_undo {
            UNDO_TOAST_MS
        } else {
            TOAST_MS
        }
    }

    // ── row actions ──

    fn reveal(&mut self, id: &str, cx: &mut Context<Self>) {
        let Some(t) = self.targets.iter().find(|t| t.id == id) else { return };
        if !platform::reveal_in_finder(&t.paths, cx) {
            self.toast("It's already gone from the disk.");
        }
    }

    /// "Never clean this": add the item to `~/.config/dusty/ignore`, let its row fade out, offer Undo.
    fn never_clean(&mut self, id: &str, cx: &mut Context<Self>) {
        if self.removing.contains_key(id) {
            return;
        }
        let Some(index) = self.targets.iter().position(|t| t.id == id) else { return };
        let t = self.targets[index].clone();
        let line = dusty_core::IgnoreEntry::line_for_item(&t.id);
        let added = match dusty_core::add_to_ignore(&line) {
            Ok(added) => added,
            Err(e) => {
                self.toast(&e);
                return;
            }
        };
        // already on the list (added elsewhere meanwhile): Undo must not take someone else's line out
        let line = added.then_some(line);
        haptic(Haptic::Click);
        if self.pick_focus.as_deref() == Some(id) {
            // the ring hops to a neighbour instead of vanishing
            self.pick_focus = self.pick_neighbour(id);
        }
        let selected = self.selected.remove(id);
        self.removing.insert(t.id.clone(), Instant::now());
        if self.hovered_row.as_deref() == Some(id) {
            self.hovered_row = None;
        }
        self.undo = Some(IgnoreUndo { target: t, index, selected, line });
        self.toast = Some(("Dusty will leave it alone.".into(), Instant::now()));
        self.toast_icon = Some(Ico::Never);
        self.toast_undo = true;
        self.bubble = "Got it. That one's off the menu for good.".into();
        if self.screen == Screen::Pick {
            self.sync_pick_dusty(cx);
        }
        cx.notify();
    }

    fn undo_ignore(&mut self, cx: &mut Context<Self>) {
        let Some(u) = self.undo.take() else { return };
        if let Some(line) = &u.line {
            if let Err(e) = dusty_core::remove_from_ignore(line) {
                self.toast(&e);
                return;
            }
        }
        haptic(Haptic::Tick);
        let id = u.target.id.clone();
        if self.removing.remove(&id).is_none() && !self.targets.iter().any(|t| t.id == id) {
            let at = u.index.min(self.targets.len());
            self.targets.insert(at, u.target);
        }
        if u.selected {
            self.selected.insert(id);
        }
        self.toast("Okay, it's back on the menu.");
        self.bubble = "Back on the menu!".into();
        if self.screen == Screen::Pick {
            self.sync_pick_dusty(cx);
        }
        cx.notify();
    }

    /// Rows that finished leaving are dropped from the list for real. True while some are still moving.
    fn tick_removals(&mut self) -> bool {
        let gone: Vec<String> = self.removing.iter().filter(|(_, t)| since(**t, REMOVE_MS) >= 1.).map(|(id, _)| id.clone()).collect();
        for id in &gone {
            self.removing.remove(id);
            if let Some(i) = self.targets.iter().position(|t| &t.id == id) {
                self.targets.remove(i);
                if let Some(u) = self.undo.as_mut().filter(|u| &u.target.id == id) {
                    u.index = i;
                }
            }
        }
        !self.removing.is_empty()
    }

    /// How far a leaving row has gone: (fade 0→1, height left 1→0).
    fn removal(&self, id: &str) -> (f32, f32) {
        let Some(t0) = self.removing.get(id) else { return (0., 1.) };
        let ms = t0.elapsed().as_secs_f32() * 1000.;
        if reduced() {
            return (1., 0.);
        }
        let fade = (ms / motion::EXIT as f32).clamp(0., 1.);
        let close = ((ms - motion::EXIT as f32) / motion::BASE as f32).clamp(0., 1.);
        (ease_out_cubic(fade), 1. - ease_out_cubic(close))
    }

    // ── keyboard ──

    /// Choose items, flattened: each group's header, then its items (removed ones included while they leave).
    /// The scroll list's children are 3 intro rows followed by exactly these.
    fn pick_rows(&self) -> Vec<(Group, Option<Target>)> {
        let mut out = vec![];
        for g in Group::ORDER.iter().copied() {
            let items: Vec<&Target> = self.targets.iter().filter(|t| t.group == g).collect();
            if items.is_empty() {
                continue;
            }
            out.push((g, None));
            out.extend(items.into_iter().map(|t| (g, Some(t.clone()))));
        }
        out
    }

    /// Ids in Choose items order that the focus ring can land on.
    fn pick_order(&self) -> Vec<String> {
        self.pick_rows().into_iter().filter_map(|(_, t)| t.map(|t| t.id)).filter(|id| !self.removing.contains_key(id)).collect()
    }

    fn pick_neighbour(&self, id: &str) -> Option<String> {
        let order = self.pick_order();
        let i = order.iter().position(|x| x == id)?;
        order.get(i + 1).or_else(|| i.checked_sub(1).and_then(|j| order.get(j))).cloned()
    }

    fn move_pick_focus(&mut self, delta: i32, cx: &mut Context<Self>) {
        let order = self.pick_order();
        if order.is_empty() {
            return;
        }
        let cur = self.pick_focus.as_ref().and_then(|id| order.iter().position(|x| x == id));
        let next = match (cur, self.pick_ring) {
            // the first press shows the ring where it is (or on the first row)
            (Some(i), false) => i,
            (None, _) => if delta > 0 { 0 } else { order.len() - 1 },
            (Some(i), true) => (i as i32 + delta).clamp(0, order.len() as i32 - 1) as usize,
        };
        let id = order[next].clone();
        if let Some(ix) = self.pick_rows().iter().position(|(_, t)| t.as_ref().is_some_and(|t| t.id == id)) {
            // the intro rows are children 0..3; the first item scrolls all the way back up (chips and title too)
            let intro = if self.is_wide() { 0 } else { 3 };
            self.pick_scroll.scroll_to_item(if next == 0 { 0 } else { intro + ix });
        }
        self.pick_focus = Some(id);
        self.pick_ring = true;
        cx.notify();
    }

    fn key_primary(&mut self, _: &Primary, _: &mut Window, cx: &mut Context<Self>) {
        if self.intro.is_some() {
            return;
        }
        if self.upd_visible {
            match &self.upd_phase {
                UpdPhase::Found(_) | UpdPhase::Ready => updater::act(UpdAction::Install),
                UpdPhase::UpToDate | UpdPhase::Failed(_) => updater::act(UpdAction::Acknowledge),
                _ => {}
            }
            cx.notify();
            return;
        }
        if self.confirm {
            if self.confirm_closing.is_none() {
                self.confirm_go(cx);
            }
            return;
        }
        if self.screen.is_tool() {
            self.tool_primary(cx);
            return;
        }
        match self.screen {
            Screen::Home => self.start_scan(cx),
            Screen::Results if self.safe_bytes() > 0 => {
                self.selected = self.safe_ids();
                self.open_confirm(cx);
            }
            Screen::Pick => self.open_confirm(cx),
            Screen::Done if !self.just_arrived() => self.leave_done(cx),
            _ => {}
        }
    }

    fn key_back(&mut self, _: &Back, _: &mut Window, cx: &mut Context<Self>) {
        if self.upd_visible {
            match &self.upd_phase {
                UpdPhase::Checking => updater::act(UpdAction::Cancel),
                UpdPhase::UpToDate | UpdPhase::Failed(_) => updater::act(UpdAction::Acknowledge),
                UpdPhase::Installing | UpdPhase::Idle => {}
                _ => updater::act(UpdAction::Hide),
            }
            cx.notify();
            return;
        }
        if self.confirm {
            self.close_confirm(cx);
            return;
        }
        if self.screen.is_tool() {
            self.tool_back(cx);
            return;
        }
        match self.screen {
            Screen::Pick if self.pick_ring => {
                // first Esc drops the focus ring, the next one goes back
                self.pick_ring = false;
                cx.notify();
            }
            Screen::Pick => self.back_to_results(cx),
            Screen::Scanning => self.cancel_scan(cx),
            Screen::Done if !self.just_arrived() => self.leave_done(cx),
            _ => {}
        }
    }

    /// A sheet (confirm or update) is up: it owns the keyboard.
    pub(super) fn modal_open(&self) -> bool {
        self.confirm || self.upd_visible
    }

    fn key_next(&mut self, _: &FocusNext, _: &mut Window, cx: &mut Context<Self>) {
        self.step_focus(1, cx);
    }

    fn key_prev(&mut self, _: &FocusPrev, _: &mut Window, cx: &mut Context<Self>) {
        self.step_focus(-1, cx);
    }

    /// ↑ / ↓: move the keyboard focus on Choose items or in a tool's list.
    fn step_focus(&mut self, delta: i32, cx: &mut Context<Self>) {
        if self.modal_open() {
            return;
        }
        if self.screen == Screen::Pick {
            self.move_pick_focus(delta, cx);
        } else if self.screen.is_tool() {
            self.tool_move_focus(delta, cx);
        }
    }

    fn key_toggle(&mut self, _: &ToggleFocused, _: &mut Window, cx: &mut Context<Self>) {
        if self.modal_open() {
            return;
        }
        if self.screen.is_tool() {
            // the app search types its spaces: let the key reach the key-down listener
            if !self.tool_toggle(cx) {
                cx.propagate();
            }
            return;
        }
        if self.screen != Screen::Pick {
            return;
        }
        match self.pick_focus.clone().filter(|_| self.pick_ring) {
            Some(id) => {
                haptic(Haptic::Tick);
                self.toggle(&id, cx);
            }
            None => self.move_pick_focus(1, cx),
        }
    }

    fn key_rescan(&mut self, _: &Rescan, _: &mut Window, cx: &mut Context<Self>) {
        if self.screen.is_tool() && !self.modal_open() {
            self.tool_rescan(cx);
            return;
        }
        if self.modal_open() || self.intro.is_some() || !matches!(self.screen, Screen::Home | Screen::Results | Screen::Pick | Screen::Done) {
            return;
        }
        self.clean = None;
        self.nav_t = Instant::now() - Duration::from_secs(1); // a key press is never a stray double-click
        self.start_scan(cx);
    }

    /// "Check up now" from the menu bar: start a checkup unless one (or a clean) is already under way.
    fn menu_checkup(&mut self, cx: &mut Context<Self>) {
        if matches!(self.screen, Screen::Scanning | Screen::Cleaning) || self.modal_open() {
            return;
        }
        self.intro = None;
        self.clean = None;
        self.tools_cancel_all();
        self.nav_t = Instant::now() - Duration::from_secs(1);
        self.start_scan(cx);
    }

    fn back_to_results(&mut self, cx: &mut Context<Self>) {
        self.dusty(cx, |d| d.set_size(RESULTS_DUSTY));
        self.go(Screen::Results, cx);
        // the numbers and bars were already counted up: they stay put when you come back
        self.screen_t = Instant::now() - Duration::from_secs(10);
    }

    fn open_pick(&mut self, cx: &mut Context<Self>) {
        self.bubble = "Tap things. I'll get lighter, promise.".into();
        self.pick_ring = false;
        self.dusty(cx, |d| d.set_size(84.));
        self.sync_pick_dusty(cx);
        self.go(Screen::Pick, cx);
    }

    fn leave_done(&mut self, cx: &mut Context<Self>) {
        self.clean = None;
        self.targets.clear();
        self.enter_home(cx);
    }

    // ── cleaning ──

    /// True right after a screen or sheet appeared: a double-click must not land on the next screen's button.
    fn just_arrived(&self) -> bool {
        self.nav_t.elapsed() < Duration::from_millis(400)
    }

    fn open_confirm(&mut self, cx: &mut Context<Self>) {
        if self.selected.is_empty() {
            return;
        }
        self.open_sheet(ConfirmKind::Clean, cx);
    }

    /// Bring up the confirm sheet for `kind` (the same sheet asks about cleaning, uninstalling and trashing files).
    fn open_sheet(&mut self, kind: ConfirmKind, cx: &mut Context<Self>) {
        if self.just_arrived() || self.confirm {
            return;
        }
        self.confirm_kind = kind;
        self.confirm = true;
        self.confirm_closing = None;
        self.nav_t = Instant::now();
        self.sheet_t = self.nav_t;
        cx.notify();
    }

    /// The sheet's "yes".
    fn confirm_go(&mut self, cx: &mut Context<Self>) {
        match self.confirm_kind {
            ConfirmKind::Clean => self.start_clean(cx),
            ConfirmKind::Uninstall => self.start_uninstall(cx),
            ConfirmKind::TrashFiles => self.start_trash_files(cx),
        }
    }

    fn start_clean(&mut self, cx: &mut Context<Self>) {
        if self.just_arrived() || self.confirm_closing.is_some() {
            return;
        }
        self.confirm = false;
        let items = self.selected_targets();
        if items.is_empty() {
            return;
        }
        let progress = Arc::new(Mutex::new(CleanProgress::default()));
        let report = Arc::new(Mutex::new(None));
        let dry = self.dry_run;
        {
            let (progress, report) = (progress.clone(), report.clone());
            let items = items.clone();
            std::thread::spawn(move || {
                // caches and build output are deleted (they rebuild, and the space really comes back); logs and
                // re-downloadables go to the Trash so they can be put back. DUSTY_FORCE_DRY still wins in the engine.
                let plan = CleanPlan::new(&items, CleanPolicy::TrashUnlessCache, HistorySource::App, dry);
                let r = dusty_core::execute(&plan, &mut |e| progress.lock().unwrap().apply(&e), &AtomicBool::new(false));
                *report.lock().unwrap() = Some(r);
            });
        }
        let before = dusty_core::disk();
        self.done_put_back = PutBack::No;
        self.clean = Some(CleanJob { items, progress, report, dry, before, next: 0, flight: None, eaten: 0, shown: 0., shown_progress: 0., failed: vec![], ok_count: 0, finished_at: None, popups: vec![], shake_until: None });
        self.bubble = if dry { "Practice run! I'll only pretend to eat.".into() } else { "Dinner time. Pass me the crumbs!".into() };
        self.dusty(cx, |d| {
            d.set_size(230.);
            d.st.set(Mood::Hungry, None);
            d.st.dirt = 1.;
            d.st.chub_target = 1.;
        });
        self.go(Screen::Cleaning, cx);
    }

    fn tick_clean(&mut self, dt: f32, window: &Window, cx: &mut Context<Self>) {
        let vw = f32::from(window.viewport_size().width);
        let mouth = self.dusty.read(cx).mouth_point();
        let Some(job) = &mut self.clean else { return };
        let done_n = job.progress.lock().unwrap().item_errors.len();
        let finished = job.report.lock().unwrap().is_some();
        let n = job.items.len();
        let mut chomp = false;
        let mut sad = false;
        let mut crumbs_at = None;
        let mut look = None;

        if job.flight.is_none() && job.next < n && job.next < done_n {
            let errs = job.progress.lock().unwrap().item_errors[job.next].clone();
            if errs.is_empty() {
                job.flight = Some(Flight { idx: job.next, t0: Instant::now(), from: point(px(vw / 2.), px(TOP + 34.)) });
            } else {
                let t = &job.items[job.next];
                job.failed.push((t.label.clone(), errs[0].clone()));
                job.shake_until = Some(Instant::now() + Duration::from_millis(420));
                job.next += 1;
                sad = true;
            }
        }
        if let Some(f) = &job.flight {
            let p = f.t0.elapsed().as_secs_f32() / flight_secs();
            if let Some(m) = mouth {
                look = Some(((f32::from(f.from.x - m.x) / 200.).clamp(-1., 1.) * (1. - p), (-1. + p * 1.4).clamp(-1., 0.4)));
            }
            if p >= 1. {
                let t = &job.items[f.idx];
                job.eaten += t.bytes;
                job.ok_count += 1;
                job.next += 1;
                if let Some(m) = mouth {
                    job.popups.push(Popup { text: format!("+{}", format_bytes(t.bytes)).into(), at: m, t0: Instant::now() });
                    crumbs_at = Some((m, tone(t.group).base));
                }
                job.flight = None;
                chomp = true;
            }
        }
        let gape = job.flight.as_ref().map(|f| f.t0.elapsed().as_secs_f32() / flight_secs() > 0.45).unwrap_or(false);
        job.shown += (job.eaten as f64 - job.shown) * (1. - (-dt as f64 * 7.).exp());
        job.popups.retain(|p| p.t0.elapsed() < Duration::from_millis(1100));
        let progress = if n == 0 { 1. } else { job.next as f32 / n as f32 };
        job.shown_progress += (progress - job.shown_progress) * (1. - (-dt * 8.).exp());
        let all_done = job.next >= n && job.flight.is_none() && finished;
        if all_done && job.finished_at.is_none() {
            job.finished_at = Some(Instant::now());
        }
        let go_done = job.finished_at.map(|t| t.elapsed() > Duration::from_millis(700)).unwrap_or(false);
        let waiting = job.flight.is_none() && job.next < n;

        self.dusty.update(cx, |d, cx| {
            d.st.dirt = 1. - progress;
            d.st.chub_target = 1. - progress;
            d.st.look_at(look.or(if waiting { Some((0., -0.8)) } else { None }));
            d.st.gape(gape);
            if chomp {
                d.st.chomp();
                haptic(Haptic::Tick);
            }
            if sad {
                d.st.set(Mood::Sad, Some(0.9));
            }
            cx.notify();
        });
        if let Some((m, color)) = crumbs_at {
            self.crumbs(m, color);
        }
        if sad {
            self.bubble = "Hm, that one's busy. I'll leave it.".into();
        } else if chomp {
            const YUM: &[&str] = &["Crunchy!", "Mmm, cached.", "Delicious build folders.", "Om nom nom.", "Lighter already!", "So tasty. So unnecessary."];
            let i = (self.rand() * YUM.len() as f32) as usize % YUM.len();
            self.bubble = YUM[i].into();
        }
        if go_done {
            self.finish_clean(cx);
        }
    }

    fn finish_clean(&mut self, cx: &mut Context<Self>) {
        let Some(job) = &self.clean else { return };
        let after = dusty_core::disk();
        self.done_before = job.before;
        let report = job.report.lock().unwrap().clone();
        // only deletes give space back right away; what went to the Trash stays on the disk until it's emptied
        let (deleted, trashed) = report.as_ref().map(|r| (r.entry.bytes_deleted(), r.entry.bytes_trashed())).unwrap_or((job.eaten, 0));
        let practice = job.dry || report.as_ref().is_some_and(|r| r.entry.practice);
        self.done_trashed = trashed;
        self.done_entry = report.as_ref().filter(|r| !r.entry.practice && r.entry.bytes_trashed() > 0).map(|r| r.entry.id.clone());
        // what the disk really gained; other apps write too, so fall back to the estimate (and say so)
        let real = after.free.saturating_sub(job.before.free);
        self.done_estimated = !practice && deleted > 0 && real <= deleted / 4;
        self.done_freed = if practice || self.done_estimated { deleted } else { real };
        // DUSTY_FORCE_DRY makes a practice run behind the UI's back: say so on the summary
        if let Some(j) = self.clean.as_mut() {
            j.dry = practice;
        }
        self.done_disk = after;
        self.disk = after;
        refresh_status_title();
        self.bubble = if practice {
            "Practice done! Nothing was deleted.".into()
        } else if trashed > 0 {
            "I feel brand new. Some of it is napping in the Trash.".into()
        } else {
            "I feel brand new. Thank you!".into()
        };
        self.dusty(cx, |d| {
            d.set_size(200.);
            d.st.set(Mood::Happy, Some(2.));
            d.st.dirt = 0.;
            d.st.chub_target = 0.;
            d.st.look_at(None);
            d.st.gape(false);
        });
        self.burst_center(140, cx);
        self.go(Screen::Done, cx);
    }

    // ── particles ──

    /// Crumbs from a new find fly up into Dusty (the scan screen's centred cat).
    fn feed_cat(&mut self, colors: Vec<u32>) {
        if reduced() || self.screen != Screen::Scanning {
            return;
        }
        let vw = self.view.0;
        for c in colors.into_iter().take(3) {
            for _ in 0..5 {
                let x = vw / 2. + (self.rand() - 0.5) * 360.;
                let y = 560. + self.rand() * 80.;
                let (tx, ty) = (vw / 2. + (self.rand() - 0.5) * 40., 270.);
                let (dx, dy) = (tx - x, ty - y);
                let steps = 24.;
                let (size, max) = (3. + self.rand() * 4., steps / 60.);
                self.particles.push(Particle { x, y, vx: dx / steps, vy: dy / steps, g: 0., size, color: c, life: 0., max, round: true });
            }
        }
    }

    fn crumbs(&mut self, at: Point<Pixels>, color: u32) {
        if reduced() {
            return;
        }
        let (x, y) = (f32::from(at.x), f32::from(at.y));
        for i in 0..16 {
            let a = -std::f32::consts::PI * (0.1 + 0.8 * self.rand());
            let v = 2. + self.rand() * 4.;
            let c = if i % 3 == 0 { 0xC9B79C } else { color };
            let (size, max) = (3. + self.rand() * 4., 0.7 + self.rand() * 0.4);
            self.particles.push(Particle { x, y, vx: a.cos() * v, vy: a.sin() * v, g: 0.25, size, color: c, life: 0., max, round: i % 2 == 0 });
        }
    }

    fn burst_center(&mut self, n: usize, cx: &mut Context<Self>) {
        let _ = cx;
        if reduced() {
            return;
        }
        const COLORS: [u32; 5] = [0x7C5CFF, 0x22C993, 0xFF5A6E, 0xFFC83D, 0x3DB8FF];
        for i in 0..n {
            let a = self.rand() * std::f32::consts::TAU;
            let v = 4. + self.rand() * 8.;
            let (size, max) = (6. + self.rand() * 6., 1.4 + self.rand() * 0.8);
            self.particles.push(Particle {
                x: 220.,
                y: 230.,
                vx: a.cos() * v,
                vy: a.sin() * v - 5.,
                g: 0.28,
                size,
                color: COLORS[i % 5],
                life: 0.,
                max,
                round: i % 4 == 0,
            });
        }
    }

    fn tick_particles(&mut self, dt: f32, vw: f32) {
        let steps = dt * 60.;
        for p in &mut self.particles {
            if p.life == 0. && p.x == 220. {
                p.x = vw / 2.; // confetti bursts from the centre of the actual window
            }
            p.vy += p.g * steps;
            p.vx *= 0.99f32.powf(steps);
            p.x += p.vx * steps;
            p.y += p.vy * steps;
            p.life += dt;
        }
        self.particles.retain(|p| p.life < p.max);
    }
}

// ───────────────────────── components ─────────────────────────
// ───────────────────────── render ─────────────────────────

impl Render for DustyApp {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let now = Instant::now();
        if self.nav == 1 && self.particles.is_empty() && crate::tracing() {
            static ONCE: std::sync::Once = std::sync::Once::new();
            ONCE.call_once(|| eprintln!("first frame after {} ms", crate::START.get().map(|t| t.elapsed().as_millis()).unwrap_or(0)));
        }
        #[cfg(feature = "profile")]
        trace_window_frames(window);
        let dt = now.duration_since(self.last).as_secs_f32().min(0.1);
        self.last = now;
        let vw = f32::from(window.viewport_size().width);
        self.view = (vw, f32::from(window.viewport_size().height));
        let tools_moving = self.tick_tools(dt, cx);
        if self.screen == Screen::Home && self.leaving.is_none() {
            let side = home_dusty_side(self.view.0, self.view.1);
            if (self.dusty.read(cx).size() - side).abs() > 0.5 {
                self.dusty(cx, |d| d.set_size(side));
            }
        }
        match self.screen {
            Screen::Scanning => self.tick_scan(dt, cx),
            Screen::Cleaning => self.tick_clean(dt, window, cx),
            _ => {}
        }
        self.tick_particles(dt, vw);
        if self.toast.as_ref().is_some_and(|(_, t)| t.elapsed() > Duration::from_millis(self.toast_ms())) {
            self.toast = None;
            if self.toast_undo {
                self.undo = None; // the chance to undo leaves with its toast
            }
            self.toast_undo = false;
        }
        let removals_moving = self.tick_removals();
        // keys go to the window's root; take focus back if nothing else holds it
        if window.focused(cx).is_none() {
            self.focus.focus(window, cx);
        }
        // the sheet plays its exit, then goes
        if self.confirm_closing.is_some_and(|t| since(t, motion::EXIT) >= 1.) {
            self.confirm = false;
            self.confirm_closing = None;
        }
        // "Free up …" glides toward the selection
        let mut pick_moving = false;
        if self.screen == Screen::Pick {
            let target = self.selected_bytes() as f64;
            let k = if reduced() { 1. } else { 1. - (-dt as f64 * 12.).exp() };
            self.pick_shown += (target - self.pick_shown) * k;
            if (target - self.pick_shown).abs() <= target * 0.0005 + 1. {
                self.pick_shown = target;
            }
            pick_moving = self.pick_shown != target;
        }
        let groups_moving = self.group_anim.values().any(|(t, _)| t.elapsed() < Duration::from_millis(motion::ENTER + motion::FILL));
        let upd_moving = self.sync_update(dt);
        if let Some(t) = self.intro_t() {
            if t >= SPLASH_HOLD && !self.intro_kicked {
                self.intro_kicked = true;
                self.dusty(cx, |d| {
                    d.st.poke(0.);
                    d.st.set(Mood::Wow, Some(0.8));
                });
            }
            if t > SPLASH_END + 0.8 {
                self.intro = None; // home entrance finished too
            }
        }
        let wide_now = self.is_wide();
        if wide_now != self.last_wide {
            self.last_wide = wide_now;
            self.layout_t = Instant::now();
        }
        let layout_k = since(self.layout_t, 220);
        let animating = self.intro.is_some()
            || matches!(self.screen, Screen::Scanning | Screen::Cleaning)
            || !self.particles.is_empty()
            || self.toast.is_some()
            || self.confirm_closing.is_some()
            || pick_moving
            || removals_moving
            || groups_moving
            || upd_moving
            || tools_moving
            || layout_k < 1.
            || self.leaving.is_some()
            // dev screenshot runs (unfocused window): keep frames coming while the first screen settles
            || (std::env::var_os("DUSTY_NO_ACTIVATE").is_some() && self.nav_t.elapsed() < Duration::from_secs(6))
            || self.nav_t.elapsed() < Duration::from_millis(1400);
        if animating {
            window.request_animation_frame();
        }

        let leaving = self.leaving.filter(|(_, t)| since(*t, LEAVE_MS) < 1.);
        if leaving.is_none() {
            self.leaving = None;
        }
        let shown = leaving.map(|(s, _)| s).unwrap_or(self.screen);
        // Dusty keeps the old screen's size until it has faded; then it takes the new one
        if leaving.is_some() {
            if let Some((old, _)) = self.leave_sizes {
                if (self.dusty.read(cx).size() - old).abs() > 0.5 {
                    self.dusty(cx, |d| d.set_size(old));
                }
            }
        } else if let Some((_, new)) = self.leave_sizes.take() {
            self.dusty(cx, |d| d.set_size(new));
        }
        if leaving.is_none() {
            self.last_side = self.dusty.read(cx).size();
        }
        // wide windows: one Dusty, one place, one size (the sidebar's)
        if self.has_sidebar(shown) && shown != Screen::Home && (self.dusty.read(cx).size() - SIDE_CAT).abs() > 0.5 {
            self.dusty(cx, |d| d.set_size(SIDE_CAT));
        }
        let content = match shown {
            Screen::Home => self.home(cx),
            Screen::Scanning => self.scanning(cx).into_any_element(),
            Screen::Results => self.results(cx),
            Screen::Pick => self.pick(cx),
            Screen::Cleaning => self.cleaning(window, cx).into_any_element(),
            Screen::Done => self.done(cx).into_any_element(),
            _ => self.tool_screen(cx),
        };

        div()
            .size_full()
            .relative()
            .track_focus(&self.focus)
            .key_context("Dusty")
            .on_action(cx.listener(Self::key_primary))
            .on_action(cx.listener(Self::key_back))
            .on_action(cx.listener(Self::key_next))
            .on_action(cx.listener(Self::key_prev))
            .on_action(cx.listener(Self::key_toggle))
            .on_action(cx.listener(Self::key_rescan))
            .on_action(cx.listener(|this, _: &UndoIgnore, _, cx| this.undo_ignore(cx)))
            // typing on Uninstall apps goes into its search field
            .on_key_down(cx.listener(|this, e: &KeyDownEvent, _, cx| this.tool_key(e, cx)))
            .bg(rgb(pal().bg))
            .font_family(FONT)
            .text_color(rgb(pal().ink))
            .flex()
            .flex_col()
            .child(self.top_band(shown, cx))
            .child(
                // wide windows: the content stays a readable column, centered, instead of stretching edge to edge
                div().flex_1().min_h(px(0.)).flex().when(self.has_sidebar(shown), |d| d.child(self.sidebar(shown, cx))).child(div().flex_1().min_w(px(0.)).h_full().flex().when(!self.has_sidebar(shown), |d| d.justify_center()).child({
                    let col = div()
                        .w_full()
                        .max_w(px(self.col_max(shown)))
                        .min_h(px(0.))
                        .flex()
                        .flex_col()
                        .child(div().size_full().flex().flex_col().opacity(0.7 + 0.3 * layout_k).child(content));
                    match leaving {
                        // out: the old screen fades away; in: the new one fades up from nothing and settles into place
                        Some((_, t0)) => col.opacity(1. - 0.6 * since(t0, LEAVE_MS)).into_any_element(),
                        None => col
                            .with_animation(ElementId::NamedInteger("screen".into(), self.nav), anim(motion::ENTER).with_easing(ease_out_quint()), |el, t| el.opacity(0.4 + 0.6 * t).mt(px(10. * (1. - t))))
                            .into_any_element(),
                    }
                })),
            )
            .when(self.confirm, |d| d.child(self.confirm_sheet(cx)))
            .when(self.upd_visible && !self.confirm, |d| d.child(self.update_sheet(cx)))
            .when_some(self.toast.clone(), |d, (msg, t0)| {
                let undo = self.toast_undo.then(|| {
                    div()
                        .id("toast-undo")
                        .h(px(HIT))
                        .ml(px(sp::S))
                        .mr(px(-sp::S))
                        .px(px(sp::S + sp::XXS))
                        .flex()
                        .items_center()
                        .rounded(px(radius::S))
                        .text_color(rgb(if is_dark() { LIGHT.grape.text } else { DARK.grape.text }))
                        .font_weight(FontWeight::BLACK)
                        .cursor_pointer()
                        .hover(|s| s.bg(alpha(pal().grape.base, 0.35)).text_color(rgb(pal().bg)))
                        .active(|s| s.opacity(0.7))
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .on_click(cx.listener(|this, _, _, cx| this.undo_ignore(cx)))
                        .child("Undo")
                        .into_any_element()
                });
                d.child(toast(msg, self.toast_icon, t0, self.toast_ms(), undo))
            })
            .child(self.particles_layer())
            .when_some(self.intro_t().filter(|t| *t < SPLASH_END), |d, t| d.child(self.splash(t, window, cx)))
    }
}

/// A dark pill under the top band: springs down, then lifts away as it fades.
/// `action` (an "Undo") sits at its right end.
fn toast(msg: SharedString, ico: Option<Ico>, t0: Instant, ms: u64, action: Option<AnyElement>) -> impl IntoElement {
    let age = t0.elapsed().as_millis() as u64;
    let k_in = since(t0, motion::BASE);
    let k_out = (age + motion::EXIT).saturating_sub(ms) as f32 / motion::EXIT as f32;
    let k_out = if reduced() { 0. } else { k_out.min(1.) };
    let a = k_in * (1. - k_out);
    div().absolute().top(px(TOP + sp::S)).left_0().right_0().px(px(GUTTER)).flex().justify_center().child(
        div()
            .max_w(px(MAX_W - 2. * GUTTER))
            .px(px(sp::L))
            .py(px(sp::M - sp::XXS))
            .rounded(px(radius::L))
            .bg(rgb(pal().ink))
            .text_color(rgb(pal().bg))
            .text_size(px(ts::BODY))
            .font_weight(FontWeight::BOLD)
            .shadow(vec![BoxShadow { color: alpha(pal().scrim, 0.25), offset: point(px(0.), px(6.)), blur_radius: px(18.), spread_radius: px(0.), inset: false }])
            .opacity(a)
            .mt(px(-sp::M * (1. - back_out(k_in)) - sp::S * k_out))
            .flex()
            .items_center()
            .gap(px(sp::S))
            .when(action.is_some(), |d| d.py(px(sp::XS)))
            .when_some(ico, |d, i| d.pl(px(sp::M)).child(icon(i, 20.)))
            .child(div().flex_1().min_w(px(0.)).child(msg))
            .children(action),
    )
}

// ───────────────────────── updates ─────────────────────────
#[cfg(test)]
mod tests {
    use super::ago;

    #[test]
    fn ago_reads_naturally() {
        assert_eq!(ago(5), "just now");
        assert_eq!(ago(60), "a minute ago");
        assert_eq!(ago(600), "10 minutes ago");
        assert_eq!(ago(3600), "an hour ago");
        assert_eq!(ago(86_400 + 5), "yesterday");
        assert_eq!(ago(3 * 86_400), "3 days ago");
        assert_eq!(ago(15 * 86_400), "2 weeks ago");
        assert_eq!(ago(90 * 86_400), "3 months ago");
        assert_eq!(ago(800 * 86_400), "2 years ago");
    }
}

/// `--features profile` + `DUSTY_TRACE=1`: every 3 s, print what gpui-pre measured for whole-window draws.
#[cfg(feature = "profile")]
fn trace_window_frames(window: &mut Window) {
    use std::sync::Mutex;
    static LAST: Mutex<Option<(Instant, u64)>> = Mutex::new(None);
    if !crate::tracing() {
        return;
    }
    let mut g = LAST.lock().unwrap();
    let Some((t, drawn_before)) = *g else {
        window.set_debug_frame_overlay_mode(gpui::DebugFrameOverlayMode::Minimal);
        *g = Some((Instant::now(), 0));
        return;
    };
    if t.elapsed() < Duration::from_secs(3) {
        return;
    }
    let s = window.frame_duration_snapshot();
    let d = &s.draw_duration_histogram;
    let drawn = d.len();
    let ms = |q: f64| d.value_at_quantile(q) as f64 / 1e6;
    eprintln!(
        "window: {:.0} draws/s, draw p50 {:.2} ms, p95 {:.2} ms, max {:.2} ms",
        (drawn - drawn_before) as f64 / t.elapsed().as_secs_f64(),
        ms(0.5),
        ms(0.95),
        d.max() as f64 / 1e6
    );
    *g = Some((Instant::now(), drawn));
}
