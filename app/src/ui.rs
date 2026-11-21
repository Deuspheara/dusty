//! The app: checkup → results → (choose items) → confirm → cleaning → done.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use gpui::prelude::FluentBuilder;
use gpui::*;

use crate::dusty::{Dusty, Mood};
use crate::engine::{self, format_bytes, Disk, Group, ScanOptions, ScanProgress, Target};
use crate::platform::{self, haptic, Haptic};

// ───────────────────────── theme ─────────────────────────

pub const FONT: &str = ".SystemUIFont";
const TOP: f32 = 54.; // the draggable band under the transparent titlebar
const ROW_H: f32 = 36.; // a row in the checkup's "latest finds" card

/// Spacing scale (px). Every padding, gap and margin comes from here.
#[allow(dead_code)]
mod sp {
    pub const XXS: f32 = 2.;
    pub const XS: f32 = 4.;
    pub const S: f32 = 8.;
    pub const M: f32 = 12.;
    pub const L: f32 = 16.;
    pub const XL: f32 = 24.;
    pub const XXL: f32 = 32.;
}
/// Page side padding: content, sticky footers and the sheet all line up on it.
const GUTTER: f32 = sp::XL;
/// Inner padding of every card.
const CARD_PAD: f32 = sp::L;
/// Minimum size of anything clickable.
const HIT: f32 = 28.;

/// Corner radii.
mod radius {
    pub const XS: f32 = 6.;
    pub const S: f32 = 8.;
    pub const M: f32 = 12.;
    pub const L: f32 = 16.;
    pub const XL: f32 = 20.;
    pub const SHEET: f32 = 28.;
}

/// Type scale (px). Display text is BLACK, body is BOLD/SEMIBOLD.
mod ts {
    pub const MICRO: f32 = 11.;
    pub const CAPTION: f32 = 12.;
    pub const SMALL: f32 = 13.;
    pub const BODY: f32 = 14.;
    pub const LEAD: f32 = 15.;
    pub const BUTTON: f32 = 16.;
    pub const TITLE: f32 = 17.;
    pub const STAT: f32 = 22.;
    pub const H2: f32 = 24.;
    pub const H1: f32 = 26.;
    pub const DISPLAY: f32 = 30.;
    pub const HERO: f32 = 46.;
    pub const HERO_XL: f32 = 52.;
}

/// Motion (ms). Springs (`back_out`) for things that arrive, quint ease-out for things that fill or slide.
mod motion {
    /// checkmarks, chevrons, small pops
    pub const QUICK: u64 = 180;
    /// toasts, backdrop fades
    pub const BASE: u64 = 260;
    /// screens, cards and bubbles arriving
    pub const ENTER: u64 = 380;
    /// the confirm sheet sliding up
    pub const SHEET: u64 = 420;
    /// the sheet / toast leaving: exits are faster than entrances
    pub const EXIT: u64 = 200;
    /// bars filling and numbers counting up
    pub const FILL: u64 = 800;
    /// delay between siblings in a staggered entrance
    pub const STAGGER: u64 = 60;
    /// how far things rise while they fade in (px)
    pub const RISE: f32 = 14.;
}

/// True when macOS "Reduce motion" is on (or DUSTY_REDUCE_MOTION=1): animations collapse to their end state,
/// the splash is skipped and confetti stays off. Fades stay, since they don't move anything.
fn reduced() -> bool {
    REDUCED.load(std::sync::atomic::Ordering::Relaxed)
}
static REDUCED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
fn refresh_reduced() {
    let v = match std::env::var("DUSTY_REDUCE_MOTION").as_deref() {
        Ok("1") => true,
        Ok("0") => false,
        _ => platform::reduce_motion(),
    };
    REDUCED.store(v, std::sync::atomic::Ordering::Relaxed);
}

/// A one-shot animation of `ms`, or (almost) instant under Reduce motion.
fn anim(ms: u64) -> Animation {
    Animation::new(Duration::from_millis(if reduced() { 1 } else { ms }))
}
/// The same, for durations measured by hand (seconds).
fn secs(ms: u64) -> f32 {
    if reduced() {
        0.001
    } else {
        ms as f32 / 1000.
    }
}
/// Progress 0..1 of something that started at `t0` and lasts `ms` (instant under Reduce motion).
fn since(t0: Instant, ms: u64) -> f32 {
    (t0.elapsed().as_secs_f32() / secs(ms)).clamp(0., 1.)
}

fn ease_out_cubic(t: f32) -> f32 {
    1. - (1. - t).powi(3)
}

/// System font with tabular digits, so counting numbers don't jiggle.
fn tabular(weight: FontWeight) -> Font {
    Font { family: FONT.into(), features: FontFeatures(Arc::new(vec![("tnum".into(), 1)])), fallbacks: None, weight, style: FontStyle::Normal }
}

#[derive(Clone, Copy)]
struct Tone {
    base: u32,
    /// the solid bottom edge of a chunky button
    edge: u32,
    /// tinted background behind a glyph or a selected row
    soft: u32,
    hover: u32,
    /// the tone as readable text on a card (sizes, group names)
    text: u32,
}

/// Every color the UI paints. Two of these exist: warm paper by day, plum night (same as the website).
struct Palette {
    bg: u32,
    ink: u32,
    muted: u32,
    line: u32,
    card: u32,
    card_hover: u32,
    /// "other used space" on the disk bar
    used: u32,
    disabled_edge: u32,
    /// dims the window behind the confirm sheet, and drop shadows
    scrim: u32,
    scrim_a: f32,
    /// text on the busy-items warning (on `sun.soft`)
    warn: u32,
    grape: Tone,
    mint: Tone,
    coral: Tone,
    sun: Tone,
    sky: Tone,
    blue: Tone,
    teal: Tone,
}

const LIGHT: Palette = Palette {
    bg: 0xFFFBF5,
    ink: 0x2B2340,
    muted: 0x8A84A3,
    line: 0xE8E4F3,
    card: 0xFFFFFF,
    card_hover: 0xFCFBFF,
    used: 0xBAB3CF,
    disabled_edge: 0xD4CFE4,
    scrim: 0x2B2340,
    scrim_a: 0.45,
    warn: 0x8A6000,
    grape: Tone { base: 0x7C5CFF, edge: 0x5B3FD9, soft: 0xEFEAFF, hover: 0x8B6EFF, text: 0x5B3FD9 },
    mint: Tone { base: 0x22C993, edge: 0x169E72, soft: 0xDFF8EE, hover: 0x2ED69F, text: 0x169E72 },
    coral: Tone { base: 0xFF5A6E, edge: 0xD63D52, soft: 0xFFE5E9, hover: 0xFF6B7D, text: 0xD63D52 },
    sun: Tone { base: 0xFFC83D, edge: 0xE0A100, soft: 0xFFF4D6, hover: 0xFFD25E, text: 0xE0A100 },
    sky: Tone { base: 0x3DB8FF, edge: 0x1E93D6, soft: 0xE3F5FF, hover: 0x55C1FF, text: 0x1E93D6 },
    blue: Tone { base: 0x5B8DEF, edge: 0x3F6FD1, soft: 0xE6EEFF, hover: 0x6E9BF2, text: 0x3F6FD1 },
    teal: Tone { base: 0x2EC4B6, edge: 0x1E9C90, soft: 0xDDF7F4, hover: 0x45D0C3, text: 0x1E9C90 },
};

/// Night: plum surfaces, tones kept vivid, accent text lifted so it reads on dark cards.
const DARK: Palette = Palette {
    bg: 0x17112B,
    ink: 0xF7F2FF,
    muted: 0xB6ACD9,
    line: 0x3A2F63,
    card: 0x251C44,
    card_hover: 0x2C2250,
    used: 0x5A4E8A,
    disabled_edge: 0x2B2350,
    scrim: 0x07040F,
    scrim_a: 0.6,
    warn: 0xFFD773,
    grape: Tone { base: 0x8B6EFF, edge: 0x5B3FD9, soft: 0x3B2E6D, hover: 0x9B82FF, text: 0xAB97FF },
    mint: Tone { base: 0x22C993, edge: 0x13805D, soft: 0x244255, hover: 0x2ED69F, text: 0x60D8B1 },
    coral: Tone { base: 0xFF5A6E, edge: 0xB8324A, soft: 0x552A4D, hover: 0xFF6B7D, text: 0xFF8897 },
    sun: Tone { base: 0xFFC83D, edge: 0xB98500, soft: 0x554242, hover: 0xFFD25E, text: 0xFFD773 },
    sky: Tone { base: 0x3DB8FF, edge: 0x1877B0, soft: 0x2A3E6D, hover: 0x55C1FF, text: 0x73CCFF },
    blue: Tone { base: 0x5B8DEF, edge: 0x3459AE, soft: 0x31356A, hover: 0x6E9BF2, text: 0x89ADF3 },
    teal: Tone { base: 0x2EC4B6, edge: 0x177E75, soft: 0x27415D, hover: 0x45D0C3, text: 0x69D5CA },
};

/// Follows the macOS appearance; flipped by `set_appearance` whenever System Settings changes it.
static DARK_MODE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

fn pal() -> &'static Palette {
    if DARK_MODE.load(std::sync::atomic::Ordering::Relaxed) {
        &DARK
    } else {
        &LIGHT
    }
}

/// Returns true when the palette actually changed.
fn set_appearance(a: WindowAppearance) -> bool {
    let dark = matches!(a, WindowAppearance::Dark | WindowAppearance::VibrantDark);
    DARK_MODE.swap(dark, std::sync::atomic::Ordering::Relaxed) != dark
}

fn tone(g: Group) -> Tone {
    let p = pal();
    match g {
        Group::Build => p.coral,
        Group::Apps => p.sky,
        Group::Cache => p.sun,
        Group::Junk => p.teal,
        Group::Docker => p.blue,
        Group::Redownload => p.grape,
    }
}
fn glyph(g: Group) -> &'static str {
    match g {
        Group::Build => "🧱",
        Group::Apps => "🌐",
        Group::Cache => "🗄️",
        Group::Junk => "🧾",
        Group::Docker => "🐳",
        Group::Redownload => "📦",
    }
}
/// Overshoot then settle (GPUI's `bounce` goes there and back, which is not what we want).
fn back_out(t: f32) -> f32 {
    let (c1, c3) = (1.70158, 2.70158);
    let u = t - 1.;
    1. + c3 * u * u * u + c1 * u * u
}

fn alpha(hex: u32, a: f32) -> Hsla {
    let mut c: Hsla = rgb(hex).into();
    c.a = a;
    c
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

#[derive(Default)]
struct CleanShared {
    done: Vec<Vec<String>>, // errors per finished target, in order
    finished: bool,
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
    shared: Arc<Mutex<CleanShared>>,
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
const SPLASH_HOLD: f32 = 0.32;
const SPLASH_IRIS: f32 = 0.42;
const SPLASH_END: f32 = SPLASH_HOLD + SPLASH_IRIS;
/// How long a toast stays up (ms).
const TOAST_MS: u64 = 2600;

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
    dusty: Entity<Dusty>,
    disk: Disk,
    targets: Vec<Target>,
    selected: HashSet<String>,
    expanded: HashSet<Group>,
    scan: Option<ScanJob>,
    clean: Option<CleanJob>,
    confirm: bool,
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
    particles: Vec<Particle>,
    last: Instant,
    rng: u64,
    done_freed: u64,
    done_estimated: bool,
    done_disk: Disk,
    hints_done: HashSet<usize>,
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
        let appearance = cx.observe_window_appearance(window, move |_, window, cx| {
            if pinned.is_none() && set_appearance(window.appearance()) {
                cx.notify();
            }
        });
        let disk = engine::disk();
        let dusty = cx.new(|_| Dusty::new(250.));
        let mut app = DustyApp {
            // Reduce motion skips the splash and the home entrance
            intro: if reduced() { None } else { Some(Instant::now()) },
            intro_kicked: false,
            screen: Screen::Home,
            nav: 0,
            nav_t: Instant::now(),
            dusty,
            disk,
            targets: vec![],
            selected: HashSet::new(),
            expanded: HashSet::new(),
            scan: None,
            clean: None,
            confirm: false,
            confirm_closing: None,
            group_anim: HashMap::new(),
            anim_gen: 0,
            pick_shown: 0.,
            dry_run: false,
            bubble: "".into(),
            toast: None,
            particles: vec![],
            last: Instant::now(),
            rng: 0x2545F4914F6CDD1D,
            done_freed: 0,
            done_estimated: false,
            done_disk: disk,
            hints_done: HashSet::new(),
            _appearance: appearance,
        };
        app.enter_home(cx);
        app
    }

    /// Seconds since launch, while the intro plays.
    fn intro_t(&self) -> Option<f32> {
        // DUSTY_SPLASH_SLOW=10 plays the intro 10× slower (for checking it frame by frame)
        let slow: f32 = std::env::var("DUSTY_SPLASH_SLOW").ok().and_then(|v| v.parse().ok()).unwrap_or(1.);
        self.intro.map(|t| t.elapsed().as_secs_f32() / slow.max(0.1))
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
        self.screen = s;
        self.nav += 1;
        self.nav_t = Instant::now();
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
        self.disk = engine::disk();
        let used = self.used();
        let (mood, line) = if used > 0.92 {
            (Mood::Sad, "I'm soooo stuffed. Help?")
        } else if used > 0.75 {
            (Mood::Think, "Feeling a bit… heavy today.")
        } else {
            (Mood::Idle, "Light as a feather. But I smell crumbs…")
        };
        self.bubble = line.into();
        self.dusty(cx, |d| {
            d.set_size(250.);
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
        let opts = ScanOptions { roots: engine::default_roots(), keep_recent_days: 0, min_size_mb: 1 };
        let task = cx.background_spawn(async move { engine::scan_all(&opts, progress, cancel) });
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
        self.expanded = [Group::Build].into();
        self.targets = targets;
        self.disk = engine::disk();
        let safe = self.safe_bytes();
        self.bubble = if safe > 0 { format!("{} is safe to toss right now. No downloads needed!", format_bytes(safe)).into() } else { "Spotless already. I'm proud of you.".into() };
        self.dusty(cx, |d| {
            d.set_size(118.);
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
        if let Some(job) = &mut self.scan {
            let p = job.progress.lock().unwrap();
            for t in p.found.iter().skip(job.seen) {
                job.total += t.bytes;
                if t.bytes > 200 * 1024 * 1024 {
                    job.chips.push((t.id.clone(), t.label.clone(), t.bytes, t.group));
                }
                if t.bytes > 4 * 1024 * 1024 * 1024 {
                    big_find = Some(t.label.clone());
                }
            }
            job.seen = p.found.len();
            let msg = p.message.clone();
            drop(p);
            if job.chips.len() > 6 {
                let n = job.chips.len() - 6;
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
        if let Some(l) = line_change {
            self.bubble = l.into();
        }
        if let Some(label) = big_find {
            self.bubble = format!("Whoa, {label}! That's a big one.").into();
            self.dusty(cx, |d| d.st.set(Mood::Wow, Some(0.7)));
        }
    }

    // ── selection ──

    fn safe_bytes(&self) -> u64 {
        self.targets.iter().filter(|t| t.default_on).map(|t| t.bytes).sum()
    }
    fn selected_targets(&self) -> Vec<Target> {
        self.targets.iter().filter(|t| self.selected.contains(&t.id)).cloned().collect()
    }
    fn selected_bytes(&self) -> u64 {
        self.targets.iter().filter(|t| self.selected.contains(&t.id)).map(|t| t.bytes).sum()
    }

    fn sync_pick_dusty(&mut self, cx: &mut Context<Self>) {
        let total: u64 = self.targets.iter().map(|t| t.bytes).sum::<u64>().max(1);
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
            if t.wipes_data() {
                self.bubble = "Careful: that one wipes real data, not just cache.".into();
                self.toast("⚠️ This wipes real data, not just a cache.");
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
    }

    // ── cleaning ──

    /// True right after a screen or sheet appeared: a double-click must not land on the next screen's button.
    fn just_arrived(&self) -> bool {
        self.nav_t.elapsed() < Duration::from_millis(400)
    }

    fn open_confirm(&mut self, cx: &mut Context<Self>) {
        if self.selected.is_empty() || self.just_arrived() || self.confirm {
            return;
        }
        self.confirm = true;
        self.confirm_closing = None;
        self.nav_t = Instant::now();
        cx.notify();
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
        let shared = Arc::new(Mutex::new(CleanShared::default()));
        let dry = self.dry_run;
        {
            let shared = shared.clone();
            let items = items.clone();
            std::thread::spawn(move || {
                for t in &items {
                    let errs = engine::clean_target(t, dry);
                    shared.lock().unwrap().done.push(errs);
                }
                shared.lock().unwrap().finished = true;
            });
        }
        let before = engine::disk();
        self.clean = Some(CleanJob { items, shared, dry, before, next: 0, flight: None, eaten: 0, shown: 0., shown_progress: 0., failed: vec![], ok_count: 0, finished_at: None, popups: vec![], shake_until: None });
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
        let (done_n, finished) = {
            let s = job.shared.lock().unwrap();
            (s.done.len(), s.finished)
        };
        let n = job.items.len();
        let mut chomp = false;
        let mut sad = false;
        let mut crumbs_at = None;
        let mut look = None;

        if job.flight.is_none() && job.next < n && job.next < done_n {
            let errs = job.shared.lock().unwrap().done[job.next].clone();
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
        let after = engine::disk();
        // what the disk really gained; fall back to the estimate (other apps write too)
        let real = after.free.saturating_sub(job.before.free);
        // what the disk really gained; other apps write too, so fall back to the estimate (and say so)
        self.done_estimated = !job.dry && real <= job.eaten / 4;
        self.done_freed = if job.dry || self.done_estimated { job.eaten } else { real };
        self.done_disk = after;
        self.disk = after;
        self.bubble = if job.dry { "Practice done! Nothing was deleted.".into() } else { "I feel brand new. Thank you!".into() };
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

#[derive(Clone, Copy)]
enum Btn {
    Primary(Tone),
    Ghost,
}

const BTN_H: f32 = 54.;
const BTN_EDGE: f32 = 4.;
/// How far a chunky button travels when pressed: it lands on its edge.
const BTN_TRAVEL: f32 = 3.;

/// Chunky 3D button: a thick bottom edge; pressing pushes the face down onto it. Brightens on hover.
fn button(id: impl Into<ElementId>, label: impl Into<SharedString>, kind: Btn, enabled: bool) -> Stateful<Div> {
    let (bg, edge, fg, hover) = match kind {
        Btn::Primary(t) => (t.base, t.edge, 0xFFFFFF, t.hover),
        Btn::Ghost => (pal().card, pal().line, pal().muted, pal().card_hover),
    };
    let (bg, edge, fg) = if enabled { (bg, edge, fg) } else { (pal().line, pal().disabled_edge, pal().muted) };
    let ghost = matches!(kind, Btn::Ghost);
    div()
        .id(id)
        .group("btn")
        .h(px(BTN_H + BTN_EDGE))
        .w_full()
        .flex_none()
        .child(
            div()
                .id("face")
                .h(px(BTN_H))
                .w_full()
                .rounded(px(radius::L))
                .bg(rgb(bg))
                .when(ghost, |d| d.border_2())
                .border_b(px(BTN_EDGE))
                .border_color(rgb(edge))
                .flex()
                .items_center()
                .justify_center()
                .text_color(rgb(fg))
                .text_size(px(ts::BUTTON))
                .font_weight(FontWeight::BLACK)
                .child(label.into().to_uppercase())
                .when(enabled, |d| {
                    d.cursor_pointer()
                        .group_hover("btn", move |s| {
                            let s = s.bg(rgb(hover));
                            if ghost {
                                s.text_color(rgb(pal().ink))
                            } else {
                                s
                            }
                        })
                        // the face drops onto its edge; its own height stays the same
                        .group_active("btn", |s| s.mt(px(BTN_TRAVEL)).h(px(BTN_H - BTN_TRAVEL)).border_b(px(BTN_EDGE - BTN_TRAVEL)))
                }),
        )
        .when(enabled, |d| {
            d.cursor_pointer().on_mouse_down(MouseButton::Left, move |_, _, _| haptic(match kind {
                Btn::Primary(_) => Haptic::Thump,
                Btn::Ghost => Haptic::Click,
            }))
        })
}

/// Small rounded pill in the top band ("Rescan", "Done"). Stops the band from starting a window drag.
fn pill(id: &'static str, label: &'static str) -> Stateful<Div> {
    div()
        .id(id)
        .absolute()
        .right(px(sp::L))
        .top(px((TOP - (HIT + 4.)) / 2.))
        .h(px(HIT + 4.))
        .px(px(sp::M))
        .flex()
        .items_center()
        .rounded_full()
        .border_2()
        .border_b_4()
        .border_color(rgb(pal().line))
        .bg(rgb(pal().card))
        .text_size(px(ts::SMALL))
        .font_weight(FontWeight::BLACK)
        .text_color(rgb(pal().muted))
        .cursor_pointer()
        .hover(|s| s.text_color(rgb(pal().grape.text)).border_color(rgb(pal().grape.base)))
        .active(|s| s.border_b_2().mt(px(2.)))
        .on_mouse_down(MouseButton::Left, |_, _, cx| {
            haptic(Haptic::Click);
            cx.stop_propagation()
        })
        .child(label)
}

/// Dusty's speech bubble; `key` changes whenever the line does, so each new line pops in.
fn bubble(text: SharedString, key: u64) -> impl IntoElement {
    div()
        .relative()
        .max_w(px(330.))
        .px(px(sp::L + sp::XXS))
        .py(px(sp::M))
        .rounded(px(sp::L + sp::XXS))
        .bg(rgb(pal().card))
        .border_2()
        .border_color(rgb(pal().line))
        .border_b_4()
        .text_size(px(ts::LEAD))
        .line_height(px(ts::LEAD * 1.35))
        .font_weight(FontWeight::BOLD)
        .text_color(rgb(pal().ink))
        .text_center()
        .child(text)
        .with_animation(ElementId::NamedInteger("bubble".into(), key), anim(motion::ENTER), |el, t| el.opacity((t * 2.).min(1.)).mt(px(sp::S * (1. - back_out(t)))))
}

/// Stacked disk bar, painted directly: used space, then each reclaimable group growing in (`t` 0→1).
/// Reclaimable segments are split by a hairline in the card color so neighbours stay readable.
fn disk_bar(disk: Disk, segments: &[(u64, u32)], t: f32) -> impl IntoElement {
    let total = disk.total.max(1) as f32;
    let used_other = disk.total.saturating_sub(disk.free).saturating_sub(segments.iter().map(|s| s.0).sum()) as f32 / total;
    let segs: Vec<(f32, u32)> = segments.iter().filter(|s| s.0 > 0).map(|(b, c)| (*b as f32 / total, *c)).collect();
    canvas(
        |_, _, _| {},
        move |b, _, window, _| {
            let (x0, w, h) = (b.origin.x, b.size.width, b.size.height);
            let r = h / 2.;
            window.paint_quad(fill(b, rgb(pal().line)).corner_radii(r));
            let grow = if segs.is_empty() { ease_out_cubic(t) } else { 1. };
            let mut x = used_other;
            let mut quads = vec![(0., used_other * grow, pal().used)];
            let n = segs.len().max(1) as f32;
            for (i, (fw, c)) in segs.iter().enumerate() {
                // each segment eases in a beat after the previous one
                let k = ease_out_cubic(((t - i as f32 * 0.35 / n) / 0.65).clamp(0., 1.));
                quads.push((x, fw * k, *c));
                x += fw * k;
            }
            for (i, (left, fw, c)) in quads.into_iter().enumerate() {
                if fw <= 0. {
                    continue;
                }
                let qb = Bounds::new(point(x0 + w * left, b.origin.y), size(w * fw, h));
                let first = left == 0.;
                let last = (left + fw) >= 0.999;
                let rr = |on: bool| if on { r } else { px(0.) };
                let corners = Corners { top_left: rr(first), bottom_left: rr(first), top_right: rr(last), bottom_right: rr(last) };
                window.paint_quad(fill(qb, rgb(c)).corner_radii(corners));
                if i > 0 && w * fw > px(4.) {
                    window.paint_quad(fill(Bounds::new(qb.origin, size(px(1.5), h)), rgb(pal().card)));
                }
            }
        },
    )
    .h(px(20.))
}

/// Rounded progress bar painted directly (track, fill, soft shine), so the fill always sits inside the track.
fn progress_bar(t: f32, tone: Tone) -> impl IntoElement {
    let t = t.clamp(0., 1.);
    canvas(
        |_, _, _| {},
        move |b, _, window, _| {
            let r = b.size.height / 2.;
            window.paint_quad(fill(b, rgb(pal().line)).corner_radii(r));
            let w = b.size.width * t;
            if w > px(0.5) {
                // never narrower than the bar is tall, so the rounded ends stay round
                let w = w.max(b.size.height);
                let fb = Bounds::new(b.origin, size(w, b.size.height));
                window.paint_quad(fill(fb, rgb(tone.base)).corner_radii(r));
                if w > b.size.height * 2. {
                    let pad = b.size.height * 0.3;
                    let shine = Bounds::new(point(b.origin.x + pad * 1.6, b.origin.y + pad * 0.85), size(w - pad * 3.2, b.size.height * 0.22));
                    window.paint_quad(fill(shine, alpha(0xFFFFFF, 0.45)).corner_radii(b.size.height * 0.11));
                }
            }
        },
    )
    .h(px(16.))
    .w_full()
}

/// Indeterminate bar for the checkup: a grape slug sliding across (a still, half-filled bar under Reduce motion).
fn busy_bar() -> impl IntoElement {
    let track = div().w_full().h(px(10.)).rounded_full().bg(rgb(pal().line)).overflow_hidden().relative();
    if reduced() {
        return track.child(div().h_full().w(relative(0.5)).rounded_full().bg(rgb(pal().grape.base)).opacity(0.7));
    }
    track.child(div().absolute().top_0().h_full().w(relative(0.3)).rounded_full().bg(rgb(pal().grape.base)).with_animation(
        "scanbar",
        Animation::new(Duration::from_millis(1300)).repeat().with_easing(ease_in_out),
        |el, t| el.left(relative(-0.3 + 1.3 * t)),
    ))
}

/// One line, cut with an ellipsis. GPUI's `truncate()` caches the first (unconstrained) measurement of a
/// `nowrap` text, so long labels got clipped mid-letter; a one-line clamp re-measures once the width is known.
trait OneLine: Styled + Sized {
    fn one_line(self) -> Self {
        self.overflow_hidden().text_ellipsis().line_clamp(1)
    }
}
impl<T: Styled> OneLine for T {}

/// A one-line label that takes the rest of a flex row. The text sits in a block wrapper so it is measured
/// against the wrapper's resolved width (a flex item measured directly can end up cut to a few letters).
fn fill_line(el: Div) -> Div {
    div().flex_1().min_w(px(0.)).overflow_hidden().child(el.one_line())
}

fn card() -> Div {
    div().rounded(px(radius::XL)).bg(rgb(pal().card)).border_2().border_color(rgb(pal().line)).border_b_4()
}

fn label_small(s: impl Into<SharedString>) -> Div {
    div().text_size(px(ts::CAPTION)).line_height(px(ts::CAPTION * 1.4)).font_weight(FontWeight::BOLD).text_color(rgb(pal().muted)).child(s.into())
}

/// Secondary sentence under a title.
fn subtitle(s: impl Into<SharedString>) -> Div {
    div().text_size(px(ts::BODY)).line_height(px(ts::BODY * 1.4)).font_weight(FontWeight::SEMIBOLD).text_color(rgb(pal().muted)).child(s.into())
}

/// A size or count: black weight with tabular digits, so it doesn't jiggle while it counts.
fn number(text: impl Into<SharedString>, size: f32, color: u32) -> Div {
    // big numbers get a tight line so they sit close to their caption
    let lh = if size >= ts::DISPLAY { 1.1 } else { 1.3 };
    div().flex_none().text_size(px(size)).line_height(px((size * lh).round())).font(tabular(FontWeight::BLACK)).text_color(rgb(color)).child(text.into())
}

/// Group glyph on its soft tint. Three sizes keep icons consistent across screens.
fn glyph_tile(g: Group, side: f32) -> Div {
    let (r, t) = if side >= 40. { (radius::M + 2., 22.) } else if side >= 32. { (radius::M - 2., 17.) } else { (radius::S, 13.) };
    div().size(px(side)).flex_none().rounded(px(r)).bg(rgb(tone(g).soft)).flex().items_center().justify_center().text_size(px(t)).child(glyph(g))
}

/// A sticky footer under a scrolling list: hairline on top, gutter on the sides.
fn footer(bg: u32) -> Div {
    div().flex_none().px(px(GUTTER)).pt(px(sp::M)).pb(px(sp::L)).flex().flex_col().gap(px(sp::XS)).border_t_2().border_color(rgb(pal().line)).bg(rgb(bg))
}

/// Checkbox that pops its tick in. `round` for the hint list.
fn checkbox(key: SharedString, on: bool, t: Tone, side: f32, round: bool) -> Div {
    div()
        .size(px(side))
        .flex_none()
        .when(round, |d| d.rounded_full())
        .when(!round, |d| d.rounded(px(radius::S)))
        .border_2()
        .border_color(rgb(if on { t.base } else { pal().line }))
        .bg(rgb(if on { t.base } else { pal().card }))
        .flex()
        .items_center()
        .justify_center()
        .text_color(rgb(0xFFFFFF))
        .font_weight(FontWeight::BLACK)
        .when(on, |d| {
            let full = side * 0.58;
            d.child(div().child("✓").with_animation(ElementId::Name(key), anim(motion::QUICK + 80), move |el, k| el.opacity(k).text_size(px(full * (0.4 + 0.6 * back_out(k))))))
        })
}

/// Pop an element in after `delay_ms`: fade and rise with a little overshoot.
fn pop_in<E: IntoElement + 'static>(el: E, id: ElementId, delay_ms: u64) -> impl IntoElement {
    let total = motion::ENTER + delay_ms;
    let start = delay_ms as f32 / total as f32;
    div().child(el).with_animation(id, anim(total), move |d, t| {
        let k = ((t - start) / (1. - start)).clamp(0., 1.);
        d.opacity(k).mt(px(motion::RISE * (1. - back_out(k))))
    })
}

/// `v` counting up from zero over `ms`, starting at `t0`.
fn count_up(v: u64, t0: Instant, ms: u64) -> u64 {
    (v as f64 * ease_out_cubic(since(t0, ms)) as f64).round() as u64
}

// ───────────────────────── render ─────────────────────────

impl Render for DustyApp {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let now = Instant::now();
        if std::env::var_os("DUSTY_TRACE").is_some() && self.nav == 1 && self.particles.is_empty() {
            static ONCE: std::sync::Once = std::sync::Once::new();
            ONCE.call_once(|| eprintln!("first frame after {} ms", crate::START.get().map(|t| t.elapsed().as_millis()).unwrap_or(0)));
        }
        let dt = now.duration_since(self.last).as_secs_f32().min(0.1);
        self.last = now;
        let vw = f32::from(window.viewport_size().width);
        match self.screen {
            Screen::Scanning => self.tick_scan(dt, cx),
            Screen::Cleaning => self.tick_clean(dt, window, cx),
            _ => {}
        }
        self.tick_particles(dt, vw);
        if self.toast.as_ref().is_some_and(|(_, t)| t.elapsed() > Duration::from_millis(TOAST_MS)) {
            self.toast = None;
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
        let animating = self.intro.is_some()
            || matches!(self.screen, Screen::Scanning | Screen::Cleaning)
            || !self.particles.is_empty()
            || self.toast.is_some()
            || self.confirm_closing.is_some()
            || pick_moving
            || groups_moving
            || self.nav_t.elapsed() < Duration::from_millis(1400);
        if animating {
            window.request_animation_frame();
        }

        let content = match self.screen {
            Screen::Home => self.home(cx).into_any_element(),
            Screen::Scanning => self.scanning(cx).into_any_element(),
            Screen::Results => self.results(cx).into_any_element(),
            Screen::Pick => self.pick(cx).into_any_element(),
            Screen::Cleaning => self.cleaning(window, cx).into_any_element(),
            Screen::Done => self.done(cx).into_any_element(),
        };

        div()
            .size_full()
            .relative()
            .bg(rgb(pal().bg))
            .font_family(FONT)
            .text_color(rgb(pal().ink))
            .flex()
            .flex_col()
            .child(self.top_band(cx))
            .child(
                div()
                    .flex_1()
                    .min_h(px(0.))
                    .flex()
                    .flex_col()
                    .child(content)
                    .with_animation(ElementId::NamedInteger("screen".into(), self.nav), anim(motion::ENTER).with_easing(ease_out_quint()), |el, t| el.opacity(t).mt(px(motion::RISE * (1. - t)))),
            )
            .when(self.confirm, |d| d.child(self.confirm_sheet(cx)))
            .when_some(self.toast.clone(), |d, (msg, t0)| d.child(toast(msg, t0)))
            .child(self.particles_layer())
            .when_some(self.intro_t().filter(|t| *t < SPLASH_END), |d, t| d.child(self.splash(t, window, cx)))
    }
}

/// A dark pill under the top band: springs down, then lifts away as it fades.
fn toast(msg: SharedString, t0: Instant) -> impl IntoElement {
    let age = t0.elapsed().as_millis() as u64;
    let k_in = since(t0, motion::BASE);
    let k_out = (age + motion::EXIT).saturating_sub(TOAST_MS) as f32 / motion::EXIT as f32;
    let k_out = if reduced() { 0. } else { k_out.min(1.) };
    let a = k_in * (1. - k_out);
    div().absolute().top(px(TOP + sp::S)).left_0().right_0().px(px(GUTTER)).flex().justify_center().child(
        div()
            .max_w(px(360.))
            .px(px(sp::L))
            .py(px(sp::M - sp::XXS))
            .rounded(px(radius::L))
            .bg(rgb(pal().ink))
            .text_color(rgb(pal().bg))
            .text_size(px(ts::BODY))
            .font_weight(FontWeight::BOLD)
            .shadow(vec![BoxShadow { color: alpha(pal().scrim, 0.25), offset: point(px(0.), px(6.)), blur_radius: px(18.), spread_radius: px(0.) }])
            .opacity(a)
            .mt(px(-sp::M * (1. - back_out(k_in)) - sp::S * k_out))
            .child(msg),
    )
}

/// Row height of an item inside an open group on Results.
const ITEM_H: f32 = 40.;

impl DustyApp {
    /// The window's top: content runs edge to edge under the transparent titlebar; this band drags the window.
    fn top_band(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let title = match self.screen {
            Screen::Home => "",
            Screen::Scanning => "Checkup",
            Screen::Results => "Results",
            Screen::Pick => "Choose items",
            Screen::Cleaning => "Cleaning",
            Screen::Done => "All done",
        };
        div()
            .id("top-band")
            .h(px(TOP))
            .w_full()
            .flex_none()
            .flex()
            .items_center()
            .justify_center()
            .relative()
            .on_mouse_down(MouseButton::Left, |e, window, cx| {
                if e.click_count >= 2 {
                    window.titlebar_double_click();
                } else {
                    platform::begin_window_drag(window, cx);
                }
            })
            .child(div().text_size(px(ts::BODY)).font_weight(FontWeight::BLACK).text_color(rgb(pal().muted)).child(title))
            .when(self.screen == Screen::Pick, |d| {
                d.child(pill("back", "Done").on_click(cx.listener(|this, _, _, cx| {
                    this.dusty(cx, |d| d.set_size(118.));
                    this.go(Screen::Results, cx);
                })))
            })
            .when(self.screen == Screen::Results, |d| d.child(pill("rescan", "Rescan").on_click(cx.listener(|this, _, _, cx| this.start_scan(cx)))))
    }

    fn page(&self) -> Div {
        div().flex_1().min_h(px(0.)).flex().flex_col().px(px(GUTTER))
    }

    fn home(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let d = self.disk;
        let bar_t = self.enter_k(0.3).min(since(self.nav_t, motion::FILL));
        self.page()
            .items_center()
            .child(div().h(px(sp::L)))
            .child(self.enter(bubble(self.bubble.clone(), self.nav), 0.12))
            .child(div().mt(px(sp::S)).child(self.dusty.clone()))
            .child(self.enter(div().text_size(px(ts::DISPLAY)).font_weight(FontWeight::BLACK).child("Disk checkup"), 0.16))
            .child(self.enter(subtitle("Dusty sniffs out build leftovers and caches you can safely clear.").text_size(px(ts::LEAD)).line_height(px(ts::LEAD * 1.4)).mt(px(sp::XS)).text_center(), 0.2))
            .child(self.enter(
                card().w_full().mt(px(sp::XL)).p(px(CARD_PAD)).flex().flex_col().gap(px(sp::M))
                    .child(
                        div().flex().justify_between().items_end()
                            .child(div().flex().items_end().gap(px(sp::S - 2.)).child(number(format_bytes(d.free), ts::H2, pal().ink)).child(label_small("free").mb(px(sp::XS))))
                            .child(label_small(format!("of {}", format_bytes(d.total))).mb(px(sp::XS))),
                    )
                    .child(disk_bar(d, &[], bar_t)),
                0.24,
            ))
            .child(div().flex_1())
            .child(self.enter(button("start", "Start checkup", Btn::Primary(pal().grape), true).on_click(cx.listener(|this, _, _, cx| this.start_scan(cx))), 0.3))
            .child(self.enter(label_small("Nothing is deleted until you say so."), 0.36).mt(px(sp::XS)).mb(px(sp::L)))
    }

    fn scanning(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let (shown, chips) = self.scan.as_ref().map(|s| (s.shown, s.chips.clone())).unwrap_or_default();
        let newest = chips.last().map(|c| c.0.clone()).unwrap_or_default();
        // five rows in a four-row window: the list slides down one slot as a find arrives, the oldest slides out
        let rows: Vec<_> = chips.into_iter().rev().take(5).collect();
        let finds: AnyElement = if rows.is_empty() {
            div().size_full().flex().items_center().justify_center().child(label_small("Nothing big yet. Still sniffing…")).into_any_element()
        } else {
            div()
                .flex()
                .flex_col()
                .children(rows.into_iter().enumerate().map(|(i, (id, label, bytes, g))| {
                    let row = div()
                        .h(px(ROW_H))
                        .flex_none()
                        .flex()
                        .items_center()
                        .gap(px(sp::M))
                        .when(i > 0, |d| d.border_t_1().border_color(rgb(pal().line)))
                        .child(glyph_tile(g, 24.))
                        .child(fill_line(div().text_size(px(ts::BODY)).font_weight(FontWeight::BOLD).child(label)))
                        .child(number(format_bytes(bytes), ts::BODY, tone(g).text));
                    if i == 0 {
                        row.with_animation(ElementId::Name(format!("find-{id}").into()), anim(motion::ENTER), |el, k| el.opacity(k)).into_any_element()
                    } else {
                        row.into_any_element()
                    }
                }))
                .with_animation(ElementId::Name(format!("finds-{newest}").into()), anim(motion::ENTER).with_easing(ease_out_quint()), |el, k| el.mt(px(-ROW_H * (1. - k))))
                .into_any_element()
        };
        self.page()
            .items_center()
            .child(div().h(px(sp::S)))
            // fixed-height slot: a one- or two-line bubble never nudges Dusty or the counter
            .child(div().h(px(70.)).flex().items_end().child(bubble(self.bubble.clone(), self.scan.as_ref().map(|s| s.line_i as u64).unwrap_or(0) + self.nav * 1000)))
            .child(div().mt(px(sp::XS)).child(self.dusty.clone()))
            .child(number(format_bytes(shown as u64), ts::HERO, pal().grape.base))
            .child(label_small("of leftovers found so far"))
            .child(div().w_full().mt(px(sp::L)).child(busy_bar()))
            // latest finds: fixed slots in a fixed-height window, so nothing below ever moves
            .child(
                card().w_full().mt(px(sp::L)).px(px(CARD_PAD)).py(px(sp::M)).flex().flex_col()
                    .child(label_small("Latest finds").mb(px(sp::XS)))
                    .child(div().h(px(4. * ROW_H)).overflow_hidden().child(finds)),
            )
            .child(div().flex_1())
            .child(button("cancel", "Stop", Btn::Ghost, true).on_click(cx.listener(|this, _, _, cx| this.cancel_scan(cx))))
            .child(div().h(px(sp::L)))
    }

    /// Open-ness (0..1) of row `i` in a group on Results: rows unfold in a quick stagger and fold together.
    fn group_k(&self, g: Group, i: usize) -> f32 {
        let open = self.expanded.contains(&g);
        let Some((t0, _)) = self.group_anim.get(&g) else { return if open { 1. } else { 0. } };
        if reduced() {
            return if open { 1. } else { 0. };
        }
        let ms = t0.elapsed().as_secs_f32() * 1000.;
        if open {
            let delay = i.min(8) as f32 * motion::STAGGER as f32 / 2.;
            ease_out_cubic(((ms - delay) / motion::ENTER as f32).clamp(0., 1.))
        } else {
            let k = (ms / motion::EXIT as f32).clamp(0., 1.);
            1. - k * k
        }
    }

    fn results(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let total: u64 = self.targets.iter().map(|t| t.bytes).sum();
        let safe = self.safe_bytes();
        let max = self.targets.iter().map(|t| t.bytes).max().unwrap_or(1).max(1);
        let segs: Vec<(u64, u32)> = Group::ORDER.iter().map(|g| (self.targets.iter().filter(|t| t.group == *g).map(|t| t.bytes).sum(), tone(*g).base)).collect();
        let groups: Vec<Group> = Group::ORDER.iter().copied().filter(|g| self.targets.iter().any(|t| t.group == *g)).collect();
        let nav = self.nav;
        let empty = self.targets.is_empty();

        let list = div()
            .id("results-list")
            .flex_1()
            .min_h(px(0.))
            .overflow_y_scroll()
            .px(px(GUTTER))
            .child(
                div().flex().items_center().gap(px(sp::M)).child(div().flex_none().child(self.dusty.clone())).child(
                    div().flex().flex_col().gap(px(sp::XS)).flex_1().min_w(px(0.))
                        .child(div().text_size(px(ts::H1)).line_height(px(ts::H1 * 1.15)).font(tabular(FontWeight::BLACK)).child(format!("Found {}", format_bytes(count_up(total, self.nav_t, motion::FILL)))))
                        .child(subtitle(self.bubble.clone())),
                ),
            )
            .child(
                card().w_full().p(px(CARD_PAD)).mt(px(sp::M)).flex().flex_col().gap(px(sp::S + sp::XXS))
                    .child(div().flex().justify_between().child(label_small(format!("{} free", format_bytes(self.disk.free)))).child(label_small(format!("of {}", format_bytes(self.disk.total)))))
                    .child(disk_bar(self.disk, &segs, since(self.nav_t, motion::FILL + 400)))
                    .when(!groups.is_empty(), |d| {
                        d.child(div().flex().flex_wrap().gap_x(px(sp::M)).gap_y(px(sp::XS)).children(groups.iter().map(|g| {
                            div().flex().items_center().gap(px(sp::XS + 1.)).child(div().size(px(10.)).rounded(px(3.)).bg(rgb(tone(*g).base))).child(label_small(g.name()))
                        })))
                    }),
            )
            .when(empty, |d| {
                d.child(pop_in(
                    card().mt(px(sp::M)).px(px(sp::XL)).py(px(sp::XL)).flex().flex_col().items_center().gap(px(sp::XS))
                        .child(div().text_size(px(ts::HERO - 10.)).child("✨"))
                        .child(div().text_size(px(ts::TITLE)).font_weight(FontWeight::BLACK).child("Nothing to clean"))
                        .child(subtitle("No build leftovers or caches worth clearing right now. Check again after a few builds.").text_center()),
                    ElementId::NamedInteger("empty".into(), nav),
                    motion::STAGGER,
                ))
            })
            .children(groups.iter().enumerate().map(|(gi, g)| {
                let g = *g;
                let t = tone(g);
                let items: Vec<Target> = self.targets.iter().filter(|x| x.group == g).cloned().collect();
                let sum: u64 = items.iter().map(|x| x.bytes).sum();
                let open = self.expanded.contains(&g);
                let gen = self.group_anim.get(&g).map(|(_, n)| *n).unwrap_or(0);
                let shown = open || self.group_k(g, 0) > 0.;
                let rows: Vec<(Target, f32)> = if shown { items.iter().enumerate().map(|(i, x)| (x.clone(), self.group_k(g, i))).collect() } else { vec![] };
                let tail = rows.first().map(|r| r.1).unwrap_or(0.);
                pop_in(
                    card().mt(px(sp::M)).overflow_hidden().child(
                        div()
                            .id(ElementId::Name(format!("group-{gi}").into()))
                            .flex()
                            .items_center()
                            .gap(px(sp::M))
                            .p(px(CARD_PAD))
                            // hover tint follows the card's inner corners (overflow doesn't clip to the radius)
                            .rounded_t(px(radius::XL - 2.))
                            .when(!shown, |d| d.rounded_b(px(radius::XL - 4.)))
                            .cursor_pointer()
                            .hover(|s| s.bg(rgb(pal().card_hover)))
                            .on_mouse_down(MouseButton::Left, |_, _, _| haptic(Haptic::Tick))
                            .on_click(cx.listener(move |this, _, _, cx| this.toggle_group(g, cx)))
                            .child(glyph_tile(g, 44.))
                            .child(
                                // name and size share the first line; the caption gets the full width under them
                                div().flex_1().min_w(px(0.)).flex().flex_col().gap(px(sp::XXS))
                                    .child(
                                        div().flex().items_center().gap(px(sp::S))
                                            .child(div().flex_1().min_w(px(0.)).text_size(px(ts::TITLE)).line_height(px(ts::TITLE * 1.25)).font_weight(FontWeight::BLACK).child(g.name()))
                                            .child(number(format_bytes(sum), ts::TITLE + 1., t.text)),
                                    )
                                    .child(label_small(format!("{} · {} items", g.blurb(), items.len()))),
                            )
                            .child(
                                div().w(px(sp::L)).flex_none().flex().justify_center().text_color(rgb(pal().muted)).font_weight(FontWeight::BLACK).child(if open { "▾" } else { "▸" })
                                    .with_animation(ElementId::NamedInteger(format!("chev-{gi}").into(), gen * 2 + open as u64), anim(motion::QUICK + 60), |el, k| el.opacity(0.3 + 0.7 * k).mt(px(-3. * (1. - back_out(k))))),
                            ),
                    ).when(shown, |d| {
                        d.child(div().px(px(CARD_PAD)).pb(px(sp::M * tail)).flex().flex_col().children(rows.into_iter().enumerate().map(|(i, (x, k))| {
                            let w = x.bytes as f32 / max as f32;
                            div().h(px(ITEM_H * k)).overflow_hidden().opacity(k).child(
                                div().h(px(ITEM_H)).pt(px(sp::XS + sp::XXS)).flex().flex_col().gap(px(sp::XS))
                                    .child(
                                        div().flex().items_center().gap(px(sp::S)).text_size(px(ts::BODY)).font_weight(FontWeight::BOLD)
                                            .child(div().size(px(10.)).flex_none().rounded_full().border_2().border_color(rgb(t.base)).when(x.default_on, |d| d.bg(rgb(t.base))))
                                            .child(fill_line(div().child(x.label.clone())))
                                            .when_some(x.detail.clone(), |d, det| d.child(div().flex_none().max_w(px(120.)).one_line().text_color(rgb(pal().muted)).text_size(px(ts::CAPTION)).child(det)))
                                            .child(number(format_bytes(x.bytes), ts::BODY, pal().ink)),
                                    )
                                    .child(
                                        div().h(px(6.)).ml(px(18.)).rounded_full().bg(rgb(pal().line)).overflow_hidden().child(
                                            div().h_full().rounded_full().bg(rgb(t.base)).w(relative(w)).with_animation(
                                                ElementId::NamedInteger(format!("bar-{}", x.id).into(), gen * 1000 + i as u64),
                                                anim(motion::FILL).with_easing(ease_out_quint()),
                                                move |el, k| el.w(relative(w * k)),
                                            ),
                                        ),
                                    ),
                            )
                        })))
                    }),
                    ElementId::NamedInteger("gcard".into(), gi as u64 + nav * 10),
                    motion::STAGGER * (gi as u64 + 1),
                )
            }))
            .child(div().h(px(sp::XL)));

        div()
            .flex_1()
            .min_h(px(0.))
            .flex()
            .flex_col()
            .child(list)
            .child(
                footer(pal().bg)
                    .child(button("clean-safe", if safe > 0 { format!("Clean {}", format_bytes(safe)) } else { "Nothing safe to clean".into() }, Btn::Primary(pal().mint), safe > 0).on_click(cx.listener(|this, _, _, cx| {
                        this.selected = this.targets.iter().filter(|t| t.default_on).map(|t| t.id.clone()).collect();
                        this.open_confirm(cx);
                    })))
                    .child(button("choose", "Choose items", Btn::Ghost, !self.targets.is_empty()).on_click(cx.listener(|this, _, _, cx| {
                        this.bubble = "Tap things. I'll get lighter, promise.".into();
                        this.dusty(cx, |d| d.set_size(84.));
                        this.sync_pick_dusty(cx);
                        this.go(Screen::Pick, cx);
                    }))),
            )
    }

    fn pick(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let groups: Vec<Group> = Group::ORDER.iter().copied().filter(|g| self.targets.iter().any(|t| t.group == *g)).collect();
        let sel_n = self.selected.len();
        let dry = self.dry_run;
        let chip = |id: &'static str, label: &'static str, on: bool| {
            div()
                .id(id)
                .h(px(HIT + 4.))
                .px(px(sp::M))
                .flex()
                .items_center()
                .rounded_full()
                .border_2()
                .border_b_4()
                .text_size(px(ts::SMALL))
                .font_weight(FontWeight::BLACK)
                .cursor_pointer()
                .border_color(rgb(if on { pal().grape.base } else { pal().line }))
                .bg(rgb(if on { pal().grape.soft } else { pal().card }))
                .text_color(rgb(if on { pal().grape.text } else { pal().ink }))
                .hover(|s| s.border_color(rgb(pal().grape.base)))
                .active(|s| s.border_b_2().mt(px(2.)))
                .on_mouse_down(MouseButton::Left, |_, _, _| haptic(Haptic::Tick))
                .child(label)
        };
        let all_safe = self.selected == self.targets.iter().filter(|t| t.default_on).map(|t| t.id.clone()).collect::<HashSet<_>>();
        let all = self.selected.len() == self.targets.len();

        let list = div()
            .id("pick-list")
            .flex_1()
            .min_h(px(0.))
            .overflow_y_scroll()
            .px(px(GUTTER))
            .child(div().text_size(px(ts::H1)).line_height(px(ts::H1 * 1.15)).font_weight(FontWeight::BLACK).child("What should Dusty eat?"))
            .child(subtitle("Pre-ticked items rebuild themselves. Purple ones cost a download later.").mt(px(sp::XS)))
            .child(
                div().flex().flex_wrap().items_center().gap(px(sp::S)).mt(px(sp::M))
                    .child(chip("q-safe", "✨ Safe picks", all_safe).on_click(cx.listener(|this, _, _, cx| {
                        this.selected = this.targets.iter().filter(|t| t.default_on).map(|t| t.id.clone()).collect();
                        this.bubble = "The safe picks. Nothing to re-download.".into();
                        this.sync_pick_dusty(cx);
                        cx.notify();
                    })))
                    .child(chip("q-all", "Everything", all).on_click(cx.listener(|this, _, _, cx| {
                        this.selected = this.targets.iter().map(|t| t.id.clone()).collect();
                        this.bubble = "EVERYTHING? Bold. I like it.".into();
                        this.dusty(cx, |d| d.st.set(Mood::Wow, Some(1.)));
                        this.sync_pick_dusty(cx);
                        cx.notify();
                    })))
                    .child(chip("q-none", "Nothing", self.selected.is_empty()).on_click(cx.listener(|this, _, _, cx| {
                        this.selected.clear();
                        this.bubble = "Nothing? I'll stay… chonky.".into();
                        this.dusty(cx, |d| d.st.set(Mood::Sad, Some(1.)));
                        this.sync_pick_dusty(cx);
                        cx.notify();
                    })))
                    .child(div().flex_1())
                    .child(
                        div()
                            .id("dry")
                            .h(px(HIT + 4.))
                            .on_mouse_down(MouseButton::Left, |_, _, _| haptic(Haptic::Tick))
                            .flex()
                            .items_center()
                            .gap(px(sp::S))
                            .cursor_pointer()
                            .text_size(px(ts::SMALL))
                            .font_weight(FontWeight::BLACK)
                            .text_color(rgb(if dry { pal().sky.text } else { pal().muted }))
                            .hover(|s| s.text_color(rgb(pal().ink)))
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.dry_run = !this.dry_run;
                                this.bubble = if this.dry_run { "Practice run: I'll only pretend.".into() } else { "For real this time!".into() };
                                cx.notify();
                            }))
                            .child("Practice run")
                            .child(
                                div().w(px(44.)).h(px(26.)).flex_none().rounded_full().bg(rgb(if dry { pal().sky.base } else { pal().line })).relative().child(
                                    div().absolute().top(px(3.)).size(px(20.)).rounded_full().bg(rgb(0xFFFFFF)).shadow(vec![BoxShadow { color: alpha(0x000000, 0.12), offset: point(px(0.), px(2.)), blur_radius: px(0.), spread_radius: px(0.) }])
                                        .with_animation(ElementId::NamedInteger("knob".into(), dry as u64), anim(motion::BASE + 40), move |el, t| {
                                            let t = back_out(t);
                                            let (a, b) = if dry { (3., 21.) } else { (21., 3.) };
                                            el.left(px(a + (b - a) * t))
                                        }),
                                ),
                            ),
                    ),
            )
            .children(groups.iter().map(|g| {
                let g = *g;
                let t = tone(g);
                let items: Vec<Target> = self.targets.iter().filter(|x| x.group == g).cloned().collect();
                let ids: Vec<String> = items.iter().map(|x| x.id.clone()).collect();
                let all_on = ids.iter().all(|i| self.selected.contains(i));
                div().mt(px(sp::XL)).flex().flex_col().gap(px(sp::S))
                    .child(
                        div().flex().items_center().gap(px(sp::S))
                            .child(div().flex_none().text_size(px(ts::TITLE)).font_weight(FontWeight::BLACK).text_color(rgb(t.text)).child(format!("{} {}", glyph(g), g.name())))
                            .child(fill_line(label_small(g.blurb())))
                            .child(
                                div()
                                    .id(ElementId::Name(format!("all-{:?}", g).into()))
                                    .flex_none()
                                    .h(px(HIT))
                                    .px(px(sp::S + sp::XXS))
                                    .flex()
                                    .items_center()
                                    .rounded_full()
                                    .text_size(px(ts::CAPTION))
                                    .font_weight(FontWeight::BLACK)
                                    .text_color(rgb(t.text))
                                    .cursor_pointer()
                                    .hover(|s| s.bg(rgb(t.soft)))
                                    .active(|s| s.opacity(0.7))
                                    .on_mouse_down(MouseButton::Left, |_, _, _| haptic(Haptic::Tick))
                                    .child(if all_on { "NONE" } else { "ALL" })
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        let all_on = ids.iter().all(|i| this.selected.contains(i));
                                        for i in &ids {
                                            if all_on {
                                                this.selected.remove(i);
                                            } else {
                                                this.selected.insert(i.clone());
                                            }
                                        }
                                        this.sync_pick_dusty(cx);
                                        cx.notify();
                                    })),
                            ),
                    )
                    .children(items.into_iter().map(|x| {
                        let on = self.selected.contains(&x.id);
                        let id = x.id.clone();
                        let risky = x.wipes_data();
                        let detail = x.detail.clone().unwrap_or_else(|| if !x.paths.is_empty() { format!("{} paths", x.paths.len()) } else if let Some(c) = x.commands.first() { format!("runs {}", c.cmd) } else { String::new() });
                        div()
                            .id(ElementId::Name(format!("item-{}", x.id).into()))
                            .flex()
                            .items_center()
                            .gap(px(sp::M))
                            .px(px(sp::M + sp::XXS))
                            .py(px(sp::M))
                            .rounded(px(radius::L))
                            .border_2()
                            .border_b_4()
                            .border_color(rgb(if on { t.base } else { pal().line }))
                            .bg(rgb(if on { t.soft } else { pal().card }))
                            .cursor_pointer()
                            .hover(move |s| {
                                let s = s.border_color(rgb(t.base));
                                if on {
                                    s
                                } else {
                                    s.bg(rgb(pal().card_hover))
                                }
                            })
                            .active(|s| s.border_b_2().mt(px(2.)))
                            .on_mouse_down(MouseButton::Left, |_, _, _| haptic(Haptic::Tick))
                            .on_click(cx.listener(move |this, _, _, cx| this.toggle(&id, cx)))
                            .child(checkbox(format!("ck-{}", x.id).into(), on, t, 26., false))
                            .child(
                                // labels are direct children of the column so their one-line cut measures at its width
                                div().flex_1().min_w(px(0.)).flex().flex_col().gap(px(sp::XXS))
                                    .child(div().text_size(px(ts::LEAD)).font_weight(FontWeight::BLACK).one_line().child(x.label.clone()))
                                    .when(!detail.is_empty(), |d| d.child(label_small(detail).one_line())),
                            )
                            .child(
                                div().flex_none().flex().flex_col().items_end().gap(px(sp::XXS))
                                    .child(number(format_bytes(x.bytes), ts::LEAD, if on { t.text } else { pal().ink }))
                                    .when(risky, |d| d.child(div().px(px(radius::XS)).py(px(1.)).rounded(px(radius::XS)).bg(rgb(pal().coral.soft)).text_color(rgb(pal().coral.text)).text_size(px(ts::MICRO)).font_weight(FontWeight::BLACK).child("wipes data"))),
                            )
                    }))
            }))
            .child(div().h(px(sp::XL)));

        div().flex_1().min_h(px(0.)).flex().flex_col().child(list).child(
            footer(pal().card)
                .gap(px(sp::S - 2.))
                .child(
                    div().flex().items_center().gap(px(sp::M)).child(div().flex_none().child(self.dusty.clone())).child(
                        div().flex().flex_col().flex_1().min_w(px(0.))
                            .child(
                                div().flex().items_end().gap(px(sp::S - 2.))
                                    .child(div().flex_none().text_size(px(ts::LEAD)).font_weight(FontWeight::BLACK).mb(px(sp::XXS)).child("Free up"))
                                    .child(number(format_bytes(self.pick_shown.round() as u64), ts::H2, pal().mint.text))
                                    .child(label_small(format!("· {sel_n} items")).mb(px(sp::XS)).font(tabular(FontWeight::BOLD))),
                            )
                            .child(div().text_size(px(ts::SMALL)).font_weight(FontWeight::SEMIBOLD).text_color(rgb(pal().muted)).one_line().child(self.bubble.clone())),
                    ),
                )
                .child(button("pick-go", if dry { format!("Practice with {sel_n} items") } else { format!("Clean {sel_n} items") }, Btn::Primary(pal().mint), sel_n > 0).on_click(cx.listener(|this, _, _, cx| this.open_confirm(cx)))),
        )
    }

    fn confirm_sheet(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let items = self.selected_targets();
        let n = items.len();
        let bytes: u64 = items.iter().map(|t| t.bytes).sum();
        let risky = items.iter().filter(|t| t.wipes_data()).count();
        let redl = items.iter().filter(|t| t.group == Group::Redownload).count();
        let dry = self.dry_run;
        // in: backdrop fades, sheet springs up. out: both leave faster than they came.
        let k_in = since(self.nav_t, motion::SHEET);
        let k_out = self.confirm_closing.map(|t| since(t, motion::EXIT)).unwrap_or(0.);
        let backdrop = since(self.nav_t, motion::BASE) * (1. - k_out);
        let drop = 380. * (1. - back_out(k_in)) + 420. * k_out * k_out;
        div()
            .id("sheet-wrap")
            .absolute()
            .inset_0()
            .occlude()
            .flex()
            .flex_col()
            .justify_end()
            .child(div().id("sheet-bg").absolute().inset_0().bg(alpha(pal().scrim, pal().scrim_a)).opacity(backdrop).on_click(cx.listener(|this, _, _, cx| this.close_confirm(cx))))
            .child(
                div()
                    .id("sheet")
                    .relative()
                    .bg(rgb(pal().card))
                    .rounded_t(px(radius::SHEET))
                    .border_t_2()
                    .border_color(rgb(pal().line))
                    .px(px(GUTTER))
                    .pt(px(sp::M))
                    .pb(px(sp::XL))
                    .flex()
                    .flex_col()
                    .items_center()
                    .gap(px(sp::S))
                    .mb(px(-drop))
                    .child(div().w(px(36.)).h(px(4.)).rounded_full().bg(rgb(pal().line)).mb(px(sp::S)))
                    .child(div().text_size(px(40.)).h(px(48.)).flex().items_center().child(if risky > 0 { "😬" } else if dry { "🧪" } else { "🧹" }).with_animation(
                        ElementId::NamedInteger("sheet-emoji".into(), self.nav),
                        anim(motion::SHEET + 120),
                        |el, k| {
                            let k = ((k - 0.25) / 0.75).clamp(0., 1.);
                            el.text_size(px(40. * (0.4 + 0.6 * back_out(k)))).opacity(k)
                        },
                    ))
                    .child(div().text_size(px(ts::STAT)).line_height(px(ts::STAT * 1.25)).font(tabular(FontWeight::BLACK)).text_center().child(if dry { format!("Practice with {n} items?") } else { format!("Delete {n} items, about {}?", format_bytes(bytes)) }))
                    .child(subtitle(if dry {
                        "Nothing will be deleted. Dusty just rehearses.".to_string()
                    } else if risky > 0 {
                        format!("{risky} of them wipe real data (not just caches). This can't be undone.")
                    } else if redl > 0 {
                        format!("{redl} will need a re-download next time you use them. The rest rebuilds by itself.")
                    } else {
                        "All of it rebuilds by itself when needed. Nothing personal is touched.".to_string()
                    }).text_center())
                    .child(div().h(px(sp::S)))
                    .child(button("confirm-go", if dry { "Start practice" } else { "Yes, clean it" }, Btn::Primary(if risky > 0 { pal().coral } else { pal().mint }), true).on_mouse_up(MouseButton::Left, move |_, _, _| if !dry { haptic(Haptic::Double) }).on_click(cx.listener(|this, _, _, cx| this.start_clean(cx))))
                    .child(button("confirm-no", "Not yet", Btn::Ghost, true).on_click(cx.listener(|this, _, _, cx| this.close_confirm(cx)))),
            )
    }

    fn cleaning(&mut self, window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let Some(job) = &self.clean else { return div().into_any_element() };
        let n = job.items.len();
        let vw = f32::from(window.viewport_size().width);
        let shaking = job.shake_until.is_some_and(|t| Instant::now() < t);
        let current = job.items.get(job.next).cloned();
        let waiting = job.flight.is_none() && job.next < n;

        // the plate: the item Dusty is working on (or the next one up), and how many follow
        let flying = job.flight.is_some();
        let plate_item = if flying { job.items.get(job.next + 1).cloned() } else { current.clone() };
        let remaining = n.saturating_sub(job.next + 1 + usize::from(flying));
        let item_card = |t: &Target| {
            let tn = tone(t.group);
            div().flex().items_center().gap(px(sp::M)).px(px(sp::M + sp::XXS)).py(px(sp::S + sp::XXS)).rounded(px(radius::L)).bg(rgb(pal().card)).border_2().border_b_4().border_color(rgb(tn.base))
                .child(glyph_tile(t.group, 34.))
                .child(div().flex().flex_col().min_w(px(0.)).child(div().text_size(px(ts::LEAD)).font_weight(FontWeight::BLACK).one_line().max_w(px(200.)).child(t.label.clone())).child(number(format_bytes(t.bytes), ts::CAPTION, tn.text)))
        };

        let mut col = self.page().items_center().relative();
        col = col
            .child(
                div().h(px(104.)).flex().flex_col().items_center().gap(px(sp::S - 2.)).pt(px(sp::XS))
                    .when_some(plate_item, |d, t| {
                        let card = item_card(&t);
                        let card = if shaking {
                            card.with_animation(ElementId::NamedInteger("shake".into(), job.next as u64), anim(420), |el, k| el.ml(px((k * 40.).sin() * 10. * (1. - k)))).into_any_element()
                        } else if flying {
                            // next one slides up into place while the current one is in the air
                            card.opacity(0.55).with_animation(ElementId::NamedInteger("next".into(), job.next as u64), anim(motion::ENTER).with_easing(ease_out_quint()), |el, k| el.mt(px(26. * (1. - k)))).into_any_element()
                        } else if reduced() {
                            card.into_any_element()
                        } else {
                            card.with_animation(ElementId::NamedInteger("wobble".into(), job.next as u64), Animation::new(Duration::from_millis(900)).repeat().with_easing(ease_in_out), |el, k| el.mt(px((k * std::f32::consts::TAU).sin() * 2.))).into_any_element()
                        };
                        d.child(card)
                    })
                    .child(label_small(if waiting && !shaking { "Working on it…".to_string() } else if remaining > 0 { format!("+{remaining} more after this") } else { String::new() })),
            )
            .child(div().mt(px(sp::S - 2.)).child(self.dusty.clone()))
            .child(bubble(self.bubble.clone(), job.next as u64 + 7777))
            .child(number(format_bytes(job.shown as u64), ts::HERO, pal().mint.text).mt(px(sp::S)))
            .child(label_small(if job.dry { "would be freed (practice run)" } else { "eaten so far" }))
            .child(div().w_full().mt(px(sp::M)).child(progress_bar(job.shown_progress, pal().mint)))
            .child(label_small(format!("{} of {}", job.next.min(n), n)).font(tabular(FontWeight::BOLD)).mt(px(sp::S - 2.)));

        // the flying item: an arc from the plate into Dusty's mouth, shrinking as it goes
        let flight = job.flight.as_ref().and_then(|f| {
            let m = self.dusty.read(_cx).mouth_point()?;
            let p = (f.t0.elapsed().as_secs_f32() / flight_secs()).min(1.);
            let e = p * p * (3. - 2. * p);
            let (x0, y0) = (f32::from(f.from.x), f32::from(f.from.y));
            let (x1, y1) = (f32::from(m.x), f32::from(m.y));
            let cxp = (x0 + x1) / 2. + vw * 0.16;
            let cyp = y0 - 50.;
            let u = 1. - e;
            let x = u * u * x0 + 2. * u * e * cxp + e * e * x1;
            let y = u * u * y0 + 2. * u * e * cyp + e * e * y1;
            let scale = 1. - 0.8 * e;
            Some((job.items[f.idx].clone(), x, y, scale, p))
        });
        let popups: Vec<(SharedString, f32, f32, f32)> = job.popups.iter().map(|p| (p.text.clone(), f32::from(p.at.x), f32::from(p.at.y), p.t0.elapsed().as_secs_f32())).collect();
        let failed = job.failed.len();

        div()
            .flex_1()
            .min_h(px(0.))
            .relative()
            .flex()
            .flex_col()
            .child(col)
            .when(failed > 0, |d| d.child(div().px(px(GUTTER)).pb(px(sp::M)).flex().justify_center().child(label_small(format!("{failed} item(s) were busy and got skipped")))))
            .child(div().h(px(sp::L)))
            // overlay in window coordinates (this column starts right under the top band)
            .when_some(flight, |d, (t, x, y, s, p)| {
                let tn = tone(t.group);
                let w = 210. * s;
                d.child(
                    div().absolute().left(px(x - w / 2.)).top(px(y - TOP - 26. * s)).w(px(w)).opacity((1.4 - p).min(1.)).child(
                        div().flex().items_center().gap(px(sp::S * s)).px(px(sp::M * s)).py(px(10. * s)).rounded(px(radius::L * s)).bg(rgb(pal().card)).border_2().border_b_4().border_color(rgb(tn.base))
                            .shadow(vec![BoxShadow { color: alpha(pal().scrim, pal().scrim_a / 3.), offset: point(px(0.), px(8. * s)), blur_radius: px(16. * s), spread_radius: px(0.) }])
                            .child(div().text_size(px(18. * s)).child(glyph(t.group)))
                            .child(div().text_size(px((ts::LEAD * s).max(5.))).font_weight(FontWeight::BLACK).one_line().child(t.label.clone())),
                    ),
                )
            })
            .children(popups.into_iter().map(|(text, x, y, age)| {
                // springs up out of the mouth, then drifts and fades
                let k = (age / 1.1).min(1.);
                let pop = back_out((age / 0.25).min(1.));
                div().absolute().left(px(x - 60.)).w(px(120.)).top(px(y - TOP - 150. - 50. * k)).flex().justify_center().opacity(1. - k * k)
                    .child(number(text, ts::STAT * (0.6 + 0.4 * pop), pal().mint.text))
            }))
            .into_any_element()
    }

    fn done(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let job_dry = self.clean.as_ref().map(|j| j.dry).unwrap_or(false);
        let ok = self.clean.as_ref().map(|j| j.ok_count).unwrap_or(0);
        let failed = self.clean.as_ref().map(|j| j.failed.clone()).unwrap_or_default();
        let freed = count_up(self.done_freed, self.nav_t, motion::FILL + 300);
        let estimated = self.done_estimated;
        let disk = self.done_disk;
        let nav = self.nav;
        let stat = |label: &'static str, value: String, t: Tone, delay: u64, i: u64| {
            let total = motion::ENTER + delay;
            let start = delay as f32 / total as f32;
            div().flex_1().flex_basis(px(0.)).rounded(px(radius::L + 2.)).bg(rgb(t.base)).border_2().border_b_4().border_color(rgb(t.base)).flex().flex_col()
                .child(div().py(px(sp::XS + 1.)).flex().justify_center().text_color(rgb(0xFFFFFF)).text_size(px(ts::CAPTION)).font_weight(FontWeight::BLACK).child(label.to_uppercase()))
                .child(div().rounded(px(radius::M + 2.)).bg(rgb(pal().card)).py(px(sp::S + sp::XXS)).flex().justify_center().child(number(value, ts::STAT, t.text)))
                .with_animation(ElementId::NamedInteger("stat".into(), nav * 10 + i), anim(total), move |el, k| {
                    let k = ((k - start) / (1. - start)).clamp(0., 1.);
                    el.opacity(k).mt(px(motion::RISE * (1. - back_out(k))))
                })
        };
        // note: no w_full() inside scroll containers (percentages don't resolve there); stretch instead
        let list = div()
            .id("done-list")
            .flex_1()
            .min_h(px(0.))
            .overflow_y_scroll()
            .px(px(GUTTER))
            .flex()
            .flex_col()
            .child(
                div().flex().flex_col().items_center()
                    .child(bubble(self.bubble.clone(), nav))
                    .child(div().mt(px(sp::XS)).child(self.dusty.clone()))
                    .child(div().text_size(px(ts::LEAD)).font_weight(FontWeight::BLACK).text_color(rgb(pal().muted)).child(if job_dry { "Would free" } else if estimated { "Freed (estimated)" } else { "Freed" }))
                    .child(number(format_bytes(freed), ts::HERO_XL, pal().mint.text)),
            )
            .child(div().flex().gap(px(sp::M)).mt(px(sp::M)).child(stat("Free space now", format_bytes(disk.free), pal().mint, 2 * motion::STAGGER, 0)).child(stat("Items cleaned", ok.to_string(), pal().sky, 4 * motion::STAGGER, 1)))
            .when(!failed.is_empty(), |d| {
                d.child(pop_in(
                    div().mt(px(sp::M)).p(px(CARD_PAD)).rounded(px(radius::L)).bg(rgb(pal().sun.soft)).text_color(rgb(pal().warn)).text_size(px(ts::BODY)).font_weight(FontWeight::BOLD).flex().flex_col().gap(px(sp::XS))
                        .child(div().font_weight(FontWeight::BLACK).child(format!("{} item(s) were busy", failed.len())))
                        .children(failed.iter().take(4).map(|(l, e)| div().one_line().child(format!("{l}: {e}"))))
                        .child(div().child("Something was using them. Try again later, no hard feelings.")),
                    ElementId::NamedInteger("busy".into(), nav),
                    5 * motion::STAGGER,
                ))
            })
            .child(pop_in(
                card().mt(px(sp::M)).p(px(CARD_PAD)).flex().flex_col()
                    .child(div().text_size(px(ts::TITLE)).font_weight(FontWeight::BLACK).mb(px(sp::S - 2.)).child("Worth checking by hand"))
                    .children(HINTS.iter().enumerate().map(|(i, (t, d))| {
                        let on = self.hints_done.contains(&i);
                        div()
                            .id(("hint", i))
                            .group("hint")
                            .on_mouse_down(MouseButton::Left, |_, _, _| haptic(Haptic::Tick))
                            .flex()
                            .items_center()
                            .gap(px(sp::M))
                            .py(px(sp::S + sp::XXS))
                            .when(i > 0, |d| d.border_t_2().border_color(rgb(pal().line)))
                            .cursor_pointer()
                            .on_click(cx.listener(move |this, _, _, cx| {
                                if !this.hints_done.remove(&i) {
                                    this.hints_done.insert(i);
                                }
                                cx.notify();
                            }))
                            .child(checkbox(format!("hint-ck-{i}").into(), on, pal().mint, 22., true).group_hover("hint", |s| s.border_color(rgb(pal().mint.base))))
                            .child(
                                div().flex().flex_col().min_w(px(0.)).gap(px(1.))
                                    .child(div().text_size(px(ts::LEAD)).font_weight(FontWeight::BLACK).text_color(rgb(if on { pal().muted } else { pal().ink })).when(on, |d| d.line_through()).child(*t))
                                    .child(label_small(*d)),
                            )
                    })),
                ElementId::NamedInteger("hints".into(), nav),
                6 * motion::STAGGER,
            ))
            .child(div().h(px(sp::L)));
        div().flex_1().min_h(px(0.)).flex().flex_col().child(list).child(footer(pal().bg).child(button("done-home", "Done", Btn::Primary(pal().mint), true).on_click(cx.listener(|this, _, _, cx| {
            this.clean = None;
            this.targets.clear();
            this.enter_home(cx);
        }))))
    }

    /// Purple splash: a white badge pops in while dust specks sweep into it, then the badge bursts
    /// into an iris that opens on Dusty and reveals the home screen. Click to skip.
    fn splash(&self, t: f32, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let vs = window.viewport_size();
        let (vw, vh) = (f32::from(vs.width), f32::from(vs.height));
        // the iris opens where Dusty is (fallback: where Dusty will be)
        let c = self.dusty.read(cx).center().map(|p| (f32::from(p.x), f32::from(p.y))).unwrap_or((vw / 2., 270.));
        let badge_in = (t / 0.22).clamp(0., 1.);
        let badge_out = ((t - SPLASH_HOLD + 0.04) / 0.12).clamp(0., 1.);
        let badge = 96. * back_out(badge_in) * (1. - badge_out * badge_out);
        let word = ((t - 0.08) / 0.2).clamp(0., 1.) * (1. - badge_out);
        let iris = ((t - SPLASH_HOLD) / SPLASH_IRIS).clamp(0., 1.);
        let iris = iris * iris * (3. - 2. * iris);
        let far = [(0., 0.), (vw, 0.), (0., vh), (vw, vh)].iter().map(|(x, y)| ((x - c.0).powi(2) + (y - c.1).powi(2)).sqrt()).fold(0., f32::max) + 12.;
        let radius = far * iris;
        let sweep = (t / SPLASH_HOLD).clamp(0., 1.);
        div()
            .id("splash")
            .absolute()
            .inset_0()
            .occlude()
            .on_mouse_down(MouseButton::Left, cx.listener(|this, _, _, cx| {
                // skip: jump straight to the reveal
                if let Some(t0) = this.intro {
                    let t = t0.elapsed().as_secs_f32();
                    if t < SPLASH_HOLD {
                        this.intro = Some(t0 - Duration::from_secs_f32(SPLASH_HOLD - t));
                    }
                }
                cx.notify();
            }))
            .child(
                canvas(
                    |_, _, _| {},
                    move |_, _, window, _| {
                        let purple = rgb(pal().grape.base);
                        if radius < 1. {
                            window.paint_quad(fill(Bounds::new(point(px(0.), px(0.)), size(px(vw), px(vh))), purple));
                        } else {
                            // purple everywhere except the iris: per 2 px row, the parts left and right of the circle
                            let step = 2.;
                            let mut y = 0.;
                            while y < vh {
                                let dy = (y + step / 2. - c.1).abs();
                                let row = |x0: f32, x1: f32| Bounds::new(point(px(x0), px(y)), size(px((x1 - x0).max(0.)), px(step + 0.5)));
                                if dy >= radius {
                                    window.paint_quad(fill(row(0., vw), purple));
                                } else {
                                    let dx = (radius * radius - dy * dy).sqrt();
                                    window.paint_quad(fill(row(0., c.0 - dx), purple));
                                    window.paint_quad(fill(row(c.0 + dx, vw), purple));
                                }
                                y += step;
                            }
                            // a soft white rim on the opening edge
                            let rim = Bounds::new(point(px(c.0 - radius - 3.), px(c.1 - radius - 3.)), size(px(radius * 2. + 6.), px(radius * 2. + 6.)));
                            window.paint_quad(quad(rim, px(radius + 3.), transparent_black(), px(3.), alpha(0xFFFFFF, 0.35 * (1. - iris)), BorderStyle::Solid));
                        }
                        // dust specks spiralling into the badge
                        if sweep < 1. {
                            for i in 0..14 {
                                let a0 = i as f32 * 2.399;
                                let k = (sweep * 1.25 - (i % 5) as f32 * 0.05).clamp(0., 1.);
                                let e = k * k;
                                let r = (150. + (i % 4) as f32 * 30.) * (1. - e);
                                let a = a0 + e * 2.2;
                                let (x, y) = (c.0 + a.cos() * r, c.1 + a.sin() * r);
                                let s = 3. + (i % 3) as f32 * 2.;
                                let col = alpha(0xFFFFFF, 0.75 * (1. - e));
                                window.paint_quad(fill(Bounds::new(point(px(x - s / 2.), px(y - s / 2.)), size(px(s), px(s))), col).corner_radii(px(s / 2.)));
                            }
                        }
                    },
                )
                .absolute()
                .inset_0(),
            )
            .when(badge > 0.5, |d| {
                d.child(
                    div()
                        .absolute()
                        .left(px(c.0 - badge / 2.))
                        .top(px(c.1 - badge / 2.))
                        .size(px(badge))
                        .rounded_full()
                        .bg(rgb(0xFFFFFF))
                        .shadow(vec![BoxShadow { color: alpha(pal().grape.edge, 0.6), offset: point(px(0.), px(badge * 0.06)), blur_radius: px(0.), spread_radius: px(0.) }])
                        .flex()
                        .items_center()
                        .justify_center()
                        .text_color(rgb(pal().grape.base))
                        .text_size(px(badge * 0.5))
                        .font_weight(FontWeight::BLACK)
                        .child("✦"),
                )
            })
            .child(
                div()
                    .absolute()
                    .left_0()
                    .right_0()
                    .top(px(c.1 + 70. - 12. * word))
                    .flex()
                    .flex_col()
                    .items_center()
                    .opacity(word)
                    .child(div().text_color(rgb(0xFFFFFF)).text_size(px(34.)).font_weight(FontWeight::BLACK).child("Dusty"))
                    .child(div().text_color(alpha(0xFFFFFF, 0.7)).text_size(px(14.)).font_weight(FontWeight::BOLD).child("tidy disk, happy Mac")),
            )
    }

    fn particles_layer(&self) -> impl IntoElement {
        let parts: Vec<(f32, f32, f32, u32, f32, bool)> = self.particles.iter().map(|p| (p.x, p.y, p.size, p.color, (1. - p.life / p.max).clamp(0., 1.), p.round)).collect();
        canvas(
            |_, _, _| {},
            move |_, _, window, _| {
                for (x, y, s, c, a, round) in parts {
                    let mut color: Hsla = rgb(c).into();
                    color.a = a.min(1.) * if a < 0.3 { a / 0.3 } else { 1. };
                    let b = Bounds::new(point(px(x - s / 2.), px(y - s / 2.)), size(px(s), px(s * if round { 1. } else { 0.6 })));
                    window.paint_quad(fill(b, color).corner_radii(px(if round { s / 2. } else { 1.5 })));
                }
            },
        )
        .absolute()
        .inset_0()
    }
}
