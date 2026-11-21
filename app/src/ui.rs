//! The app: checkup → results → (choose items) → confirm → cleaning → done.

use std::collections::HashSet;
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
const BG: u32 = 0xFFFBF5;
const INK: u32 = 0x2B2340;
const MUTED: u32 = 0x8A84A3;
const LINE: u32 = 0xE8E4F3;
const CARD: u32 = 0xFFFFFF;
const TOP: f32 = 54.; // the draggable band under the transparent titlebar
const ROW_H: f32 = 34.; // a row in the checkup's "latest finds" card

#[derive(Clone, Copy)]
struct Tone {
    base: u32,
    edge: u32,
    soft: u32,
    hover: u32,
}
const GRAPE: Tone = Tone { base: 0x7C5CFF, edge: 0x5B3FD9, soft: 0xEFEAFF, hover: 0x8B6EFF };
const MINT: Tone = Tone { base: 0x22C993, edge: 0x169E72, soft: 0xDFF8EE, hover: 0x2ED69F };
const CORAL: Tone = Tone { base: 0xFF5A6E, edge: 0xD63D52, soft: 0xFFE5E9, hover: 0xFF6B7D };
const SUN: Tone = Tone { base: 0xFFC83D, edge: 0xE0A100, soft: 0xFFF4D6, hover: 0xFFD25E };
const SKY: Tone = Tone { base: 0x3DB8FF, edge: 0x1E93D6, soft: 0xE3F5FF, hover: 0x55C1FF };
const BLUE: Tone = Tone { base: 0x5B8DEF, edge: 0x3F6FD1, soft: 0xE6EEFF, hover: 0x6E9BF2 };
const TEAL: Tone = Tone { base: 0x2EC4B6, edge: 0x1E9C90, soft: 0xDDF7F4, hover: 0x45D0C3 };

fn tone(g: Group) -> Tone {
    match g {
        Group::Build => CORAL,
        Group::Apps => SKY,
        Group::Cache => SUN,
        Group::Junk => TEAL,
        Group::Docker => BLUE,
        Group::Redownload => GRAPE,
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
}

const HINTS: &[(&str, &str)] = &[
    ("Old simulator runtimes", "Xcode → Settings → Components, delete the ones you don't need"),
    ("Docker.raw keeps its size", "Docker Desktop → Settings → Resources, lower the disk limit"),
    ("Android SDK leftovers", "Android Studio → SDK Manager, unused images and NDKs"),
    ("Time Machine local snapshots", "Run: tmutil listlocalsnapshots /"),
    ("Old backups", "Things like ~/.codex.backup-* can go if you don't need them"),
];

impl DustyApp {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let disk = engine::disk();
        let dusty = cx.new(|_| Dusty::new(250.));
        let mut app = DustyApp {
            intro: Some(Instant::now()),
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
            Some(t) => ((t - SPLASH_HOLD - delay) / 0.38).clamp(0., 1.),
        }
    }

    /// Wrap an element in its entrance: fade in and rise with a little overshoot.
    fn enter(&self, el: impl IntoElement, delay: f32) -> Div {
        let k = self.enter_k(delay);
        div().flex().flex_col().items_center().opacity((k * 1.6).min(1.)).mt(px(22. * (1. - back_out(k)))).child(el)
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
        self.selected_targets().iter().map(|t| t.bytes).sum()
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

    fn toast(&mut self, msg: &str) {
        self.toast = Some((msg.to_string().into(), Instant::now()));
    }

    // ── cleaning ──

    /// True right after a screen or sheet appeared: a double-click must not land on the next screen's button.
    fn just_arrived(&self) -> bool {
        self.nav_t.elapsed() < Duration::from_millis(400)
    }

    fn open_confirm(&mut self, cx: &mut Context<Self>) {
        if self.selected.is_empty() || self.just_arrived() {
            return;
        }
        self.confirm = true;
        self.nav_t = Instant::now();
        cx.notify();
    }

    fn start_clean(&mut self, cx: &mut Context<Self>) {
        if self.just_arrived() {
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
            let p = f.t0.elapsed().as_secs_f32() / 0.8;
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
        let gape = job.flight.as_ref().map(|f| f.t0.elapsed().as_secs_f32() / 0.8 > 0.45).unwrap_or(false);
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

/// Chunky 3D button: a thick bottom edge that collapses when pressed; brightens on hover.
fn button(id: impl Into<ElementId>, label: impl Into<SharedString>, kind: Btn, enabled: bool) -> Stateful<Div> {
    let (bg, edge, fg, hover) = match kind {
        Btn::Primary(t) => (t.base, t.edge, 0xFFFFFF, t.hover),
        Btn::Ghost => (CARD, LINE, MUTED, 0xFBFAFF),
    };
    let (bg, edge, fg) = if enabled { (bg, edge, fg) } else { (LINE, 0xD4CFE4, MUTED) };
    div()
        .id(id)
        .h(px(58.))
        .w_full()
        .flex_none()
        .child(
            div()
                .h(px(54.))
                .w_full()
                .rounded(px(16.))
                .bg(rgb(bg))
                .border_b_4()
                .border_color(rgb(edge))
                .when(matches!(kind, Btn::Ghost), |d| d.border_2().border_b_4())
                .flex()
                .items_center()
                .justify_center()
                .text_color(rgb(fg))
                .text_size(px(16.))
                .font_weight(FontWeight::BLACK)
                .child(label.into().to_uppercase())
                .when(enabled, |d| d.cursor_pointer().hover(move |s| s.bg(rgb(hover)))),
        )
        .when(enabled, |d| {
            d.active(|s| s.pt(px(3.))).on_mouse_down(MouseButton::Left, move |_, _, _| haptic(match kind {
                Btn::Primary(_) => Haptic::Thump,
                Btn::Ghost => Haptic::Click,
            }))
        })
}

fn bubble(text: SharedString, key: u64) -> impl IntoElement {
    div()
        .relative()
        .max_w(px(330.))
        .px(px(18.))
        .py(px(12.))
        .rounded(px(18.))
        .bg(rgb(CARD))
        .border_2()
        .border_color(rgb(LINE))
        .border_b_4()
        .text_size(px(15.))
        .font_weight(FontWeight::BOLD)
        .text_color(rgb(INK))
        .text_center()
        .child(text)
        .with_animation(ElementId::NamedInteger("bubble".into(), key), Animation::new(Duration::from_millis(360)), |el, t| el.opacity((t * 2.).min(1.)).mt(px(8. * (1. - back_out(t)))))
}

/// Stacked disk bar, painted directly: used space, then each reclaimable group growing in (`t` 0→1).
fn disk_bar(disk: Disk, segments: &[(u64, u32)], t: f32) -> impl IntoElement {
    let total = disk.total.max(1) as f32;
    let used_other = disk.total.saturating_sub(disk.free).saturating_sub(segments.iter().map(|s| s.0).sum()) as f32 / total;
    let segs: Vec<(f32, u32)> = segments.iter().map(|(b, c)| (*b as f32 / total, *c)).collect();
    canvas(
        |_, _, _| {},
        move |b, _, window, _| {
            let (x0, w, h) = (b.origin.x, b.size.width, b.size.height);
            let r = h / 2.;
            window.paint_quad(fill(b, rgb(LINE)).corner_radii(r));
            let grow = if segs.is_empty() { 1. - (1. - t).powi(3) } else { 1. };
            let mut x = used_other;
            let mut quads = vec![(0., used_other * grow, 0xBAB3CFu32)];
            for (i, (fw, c)) in segs.iter().enumerate() {
                // each segment eases in a little after the previous one
                let k = ((t * 1.4 - i as f32 * 0.12).clamp(0., 1.)).powf(0.6);
                quads.push((x, fw * k, *c));
                x += fw * k;
            }
            for (left, fw, c) in quads {
                if fw <= 0. {
                    continue;
                }
                let qb = Bounds::new(point(x0 + w * left, b.origin.y), size(w * fw, h));
                let first = left == 0.;
                let last = (left + fw) >= 0.999;
                let corners = Corners { top_left: if first { r } else { px(0.) }, bottom_left: if first { r } else { px(0.) }, top_right: if last { r } else { px(0.) }, bottom_right: if last { r } else { px(0.) } };
                window.paint_quad(fill(qb, rgb(c)).corner_radii(corners));
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
            window.paint_quad(fill(b, rgb(LINE)).corner_radii(r));
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

fn card() -> Div {
    div().rounded(px(20.)).bg(rgb(CARD)).border_2().border_color(rgb(LINE)).border_b_4()
}

fn label_small(s: impl Into<SharedString>) -> Div {
    div().text_size(px(12.)).font_weight(FontWeight::BOLD).text_color(rgb(MUTED)).child(s.into())
}

fn pop_in<E: IntoElement + 'static>(el: E, id: ElementId, delay_ms: u64) -> impl IntoElement {
    let total = 380 + delay_ms;
    let start = delay_ms as f32 / total as f32;
    div().child(el).with_animation(id, Animation::new(Duration::from_millis(total)), move |d, t| {
        let k = ((t - start) / (1. - start)).clamp(0., 1.);
        let e = back_out(k);
        d.opacity(k.min(1.)).mt(px(14. * (1. - e)))
    })
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
        if self.toast.as_ref().is_some_and(|(_, t)| t.elapsed() > Duration::from_millis(2600)) {
            self.toast = None;
        }
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
        let animating = self.intro.is_some() || matches!(self.screen, Screen::Scanning | Screen::Cleaning) || !self.particles.is_empty() || self.toast.is_some() || self.nav_t.elapsed() < Duration::from_millis(1400);
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
            .bg(rgb(BG))
            .font_family(FONT)
            .text_color(rgb(INK))
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
                    .with_animation(ElementId::NamedInteger("screen".into(), self.nav), Animation::new(Duration::from_millis(340)).with_easing(ease_out_quint()), |el, t| el.opacity(t).mt(px(18. * (1. - t)))),
            )
            .when(self.confirm, |d| d.child(self.confirm_sheet(cx)))
            .when_some(self.toast.clone(), |d, (msg, t0)| {
                let age = t0.elapsed().as_secs_f32();
                let a = (age * 6.).min(1.) * ((2.6 - age) * 4.).clamp(0., 1.);
                d.child(
                    div().absolute().top(px(TOP + 6.)).left_0().right_0().flex().justify_center().child(
                        div().px(px(16.)).py(px(10.)).rounded(px(14.)).bg(rgb(INK)).text_color(rgb(0xFFFFFF)).text_size(px(14.)).font_weight(FontWeight::BOLD).opacity(a).mt(px(-10. * (1. - a))).child(msg),
                    ),
                )
            })
            .child(self.particles_layer())
            .when_some(self.intro_t().filter(|t| *t < SPLASH_END), |d, t| d.child(self.splash(t, window, cx)))
    }
}

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
            .child(div().text_size(px(14.)).font_weight(FontWeight::BLACK).text_color(rgb(MUTED)).child(title))
            .when(self.screen == Screen::Pick, |d| {
                d.child(
                    div()
                        .id("back")
                        .absolute()
                        .right(px(16.))
                        .top(px(14.))
                        .px(px(12.))
                        .py(px(5.))
                        .rounded_full()
                        .border_2()
                        .border_color(rgb(LINE))
                        .bg(rgb(CARD))
                        .text_size(px(13.))
                        .font_weight(FontWeight::BLACK)
                        .text_color(rgb(MUTED))
                        .cursor_pointer()
                        .hover(|s| s.text_color(rgb(GRAPE.base)).border_color(rgb(GRAPE.base)))
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.dusty(cx, |d| d.set_size(118.));
                            this.go(Screen::Results, cx);
                        }))
                        .child("Done"),
                )
            })
            .when(self.screen == Screen::Results, |d| {
                d.child(
                    div()
                        .id("rescan")
                        .absolute()
                        .right(px(16.))
                        .top(px(14.))
                        .px(px(12.))
                        .py(px(5.))
                        .rounded_full()
                        .border_2()
                        .border_color(rgb(LINE))
                        .bg(rgb(CARD))
                        .text_size(px(13.))
                        .font_weight(FontWeight::BLACK)
                        .text_color(rgb(MUTED))
                        .cursor_pointer()
                        .hover(|s| s.text_color(rgb(GRAPE.base)).border_color(rgb(GRAPE.base)))
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .on_click(cx.listener(|this, _, _, cx| this.start_scan(cx)))
                        .child("Rescan"),
                )
            })
    }

    fn page(&self) -> Div {
        div().flex_1().min_h(px(0.)).flex().flex_col().px(px(22.))
    }

    fn home(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let d = self.disk;
        self.page()
            .items_center()
            .child(div().h(px(18.)))
            .child(self.enter(bubble(self.bubble.clone(), self.nav), 0.12))
            .child(div().mt(px(6.)).child(self.dusty.clone()))
            .child(self.enter(div().text_size(px(30.)).font_weight(FontWeight::BLACK).mt(px(2.)).child("Disk checkup"), 0.16))
            .child(self.enter(div().text_size(px(15.)).font_weight(FontWeight::SEMIBOLD).text_color(rgb(MUTED)).mt(px(4.)).text_center().child("Dusty sniffs out build leftovers and caches you can safely clear."), 0.2))
            .child(
                card().w_full().mt(px(20.)).p(px(16.)).flex().flex_col().gap(px(10.))
                    .opacity((self.enter_k(0.24) * 1.6).min(1.)).mt(px(20. + 22. * (1. - back_out(self.enter_k(0.24)))))
                    .child(
                    div().flex().justify_between().items_end()
                        .child(div().flex().items_end().gap(px(6.)).child(div().text_size(px(24.)).font_weight(FontWeight::BLACK).child(format_bytes(d.free))).child(label_small("free").mb(px(4.))))
                        .child(label_small(format!("of {}", format_bytes(d.total))).mb(px(4.))),
                ).child(disk_bar(d, &[], self.enter_k(0.3))),
            )
            .child(div().flex_1())
            .child(
                div().w_full().opacity((self.enter_k(0.3) * 1.6).min(1.)).mt(px(30. * (1. - back_out(self.enter_k(0.3)))))
                    .child(button("start", "Start checkup", Btn::Primary(GRAPE), true).on_click(cx.listener(|this, _, _, cx| this.start_scan(cx)))),
            )
            .child(self.enter(label_small("Nothing is deleted until you say so."), 0.36).mb(px(18.)).mt(px(2.)))
    }

    fn scanning(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let (shown, chips) = self.scan.as_ref().map(|s| (s.shown, s.chips.clone())).unwrap_or_default();
        self.page()
            .items_center()
            .child(div().h(px(8.)))
            // fixed-height slot: a one- or two-line bubble never nudges Dusty or the counter
            .child(div().h(px(70.)).flex().items_end().child(bubble(self.bubble.clone(), self.scan.as_ref().map(|s| s.line_i as u64).unwrap_or(0) + self.nav * 1000)))
            .child(div().mt(px(4.)).child(self.dusty.clone()))
            .child(div().text_size(px(46.)).font_weight(FontWeight::BLACK).text_color(rgb(GRAPE.base)).child(format_bytes(shown as u64)))
            .child(label_small("of leftovers found so far"))
            .child(
                div().w_full().h(px(10.)).mt(px(16.)).rounded_full().bg(rgb(LINE)).overflow_hidden().relative().child(
                    div().absolute().top_0().h_full().w(relative(0.3)).rounded_full().bg(rgb(GRAPE.base)).with_animation(
                        "scanbar",
                        Animation::new(Duration::from_millis(1300)).repeat().with_easing(ease_in_out),
                        |el, t| el.left(relative(-0.3 + 1.3 * t)),
                    ),
                ),
            )
            // latest finds: fixed slots in a fixed-height card, so nothing below ever moves
            .child(
                card().w_full().mt(px(16.)).h(px(4. * ROW_H + 24.)).px(px(14.)).py(px(10.)).flex().flex_col()
                    .child(label_small("Latest finds").mb(px(2.)))
                    .children(chips.into_iter().rev().take(4).enumerate().map(|(i, (id, label, bytes, g))| {
                        let t = tone(g);
                        let row = div().h(px(ROW_H)).flex_none().flex().items_center().gap(px(10.)).when(i > 0, |d| d.border_t_1().border_color(rgb(LINE)))
                            .child(div().size(px(24.)).flex_none().rounded(px(8.)).bg(rgb(t.soft)).flex().items_center().justify_center().text_size(px(13.)).child(glyph(g)))
                            .child(div().flex_1().min_w(px(0.)).truncate().text_size(px(14.)).font_weight(FontWeight::BOLD).child(label))
                            .child(div().flex_none().text_size(px(14.)).font_weight(FontWeight::BLACK).text_color(rgb(t.edge)).child(format_bytes(bytes)))
                            ;
                        if i == 0 {
                            row.with_animation(ElementId::Name(format!("find-{id}").into()), Animation::new(Duration::from_millis(350)).with_easing(ease_out_quint()), |el, k| el.opacity(k)).into_any_element()
                        } else {
                            row.into_any_element()
                        }
                    })),
            )
            .child(div().flex_1())
            .child(button("cancel", "Stop", Btn::Ghost, true).on_click(cx.listener(|this, _, _, cx| this.cancel_scan(cx))))
            .child(div().h(px(16.)))
    }

    fn results(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let total: u64 = self.targets.iter().map(|t| t.bytes).sum();
        let safe = self.safe_bytes();
        let max = self.targets.iter().map(|t| t.bytes).max().unwrap_or(1).max(1);
        let segs: Vec<(u64, u32)> = Group::ORDER.iter().map(|g| (self.targets.iter().filter(|t| t.group == *g).map(|t| t.bytes).sum(), tone(*g).base)).collect();
        let groups: Vec<Group> = Group::ORDER.iter().copied().filter(|g| self.targets.iter().any(|t| t.group == *g)).collect();
        let nav = self.nav;

        let list = div()
            .id("results-list")
            .flex_1()
            .min_h(px(0.))
            .overflow_y_scroll()
            .px(px(22.))
            .child(
                div().flex().items_center().gap(px(12.)).child(div().flex_none().child(self.dusty.clone())).child(
                    div().flex().flex_col().gap(px(4.)).flex_1().min_w(px(0.))
                        .child(div().text_size(px(28.)).font_weight(FontWeight::BLACK).child(format!("Found {}", format_bytes(total))))
                        .child(div().text_size(px(14.)).font_weight(FontWeight::SEMIBOLD).text_color(rgb(MUTED)).child(self.bubble.clone())),
                ),
            )
            .child(
                card().w_full().p(px(14.)).mt(px(10.)).flex().flex_col().gap(px(8.))
                    .child(div().flex().justify_between().child(label_small(format!("{} free", format_bytes(self.disk.free)))).child(label_small(format!("of {}", format_bytes(self.disk.total)))))
                    .child(disk_bar(self.disk, &segs, (self.nav_t.elapsed().as_secs_f32() / 1.2).min(1.)))
                    .child(div().flex().flex_wrap().gap(px(10.)).children(groups.iter().map(|g| {
                        div().flex().items_center().gap(px(5.)).child(div().size(px(10.)).rounded(px(3.)).bg(rgb(tone(*g).base))).child(label_small(g.name()))
                    }))),
            )
            .children(groups.iter().enumerate().map(|(gi, g)| {
                let g = *g;
                let t = tone(g);
                let items: Vec<Target> = self.targets.iter().filter(|x| x.group == g).cloned().collect();
                let sum: u64 = items.iter().map(|x| x.bytes).sum();
                let open = self.expanded.contains(&g);
                pop_in(
                    card().mt(px(12.)).overflow_hidden().child(
                        div()
                            .id(ElementId::Name(format!("group-{gi}").into()))
                            .flex()
                            .items_center()
                            .gap(px(12.))
                            .p(px(14.))
                            .cursor_pointer()
                            .hover(|s| s.bg(rgb(0xFCFBFF)))
                            .on_mouse_down(MouseButton::Left, |_, _, _| haptic(Haptic::Tick))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                if !this.expanded.remove(&g) {
                                    this.expanded.insert(g);
                                }
                                cx.notify();
                            }))
                            .child(div().size(px(44.)).flex_none().rounded(px(14.)).bg(rgb(t.soft)).flex().items_center().justify_center().text_size(px(22.)).child(glyph(g)))
                            .child(div().flex_1().min_w(px(0.)).flex().flex_col().child(div().text_size(px(17.)).font_weight(FontWeight::BLACK).child(g.name())).child(label_small(format!("{} · {} items", g.blurb(), items.len()))))
                            .child(div().text_size(px(18.)).font_weight(FontWeight::BLACK).text_color(rgb(t.edge)).child(format_bytes(sum)))
                            .child(div().text_color(rgb(MUTED)).font_weight(FontWeight::BLACK).child(if open { "▾" } else { "▸" })),
                    ).when(open, |d| {
                        d.child(div().px(px(16.)).pb(px(14.)).flex().flex_col().gap(px(10.)).children(items.iter().enumerate().map(|(i, x)| {
                            let w = x.bytes as f32 / max as f32;
                            div().flex().flex_col().gap(px(4.)).child(
                                div().flex().items_center().gap(px(8.)).text_size(px(14.)).font_weight(FontWeight::BOLD)
                                    .child(div().size(px(10.)).flex_none().rounded_full().border_2().border_color(rgb(t.base)).when(x.default_on, |d| d.bg(rgb(t.base))))
                                    .child(div().flex_1().min_w(px(0.)).truncate().child(x.label.clone()))
                                    .when_some(x.detail.clone(), |d, det| d.child(div().text_color(rgb(MUTED)).text_size(px(12.)).truncate().max_w(px(120.)).child(det)))
                                    .child(div().font_weight(FontWeight::BLACK).child(format_bytes(x.bytes))),
                            ).child(
                                div().h(px(7.)).ml(px(18.)).rounded_full().bg(rgb(LINE)).overflow_hidden().child(
                                    div().h_full().rounded_full().bg(rgb(t.base)).w(relative(w)).with_animation(
                                        ElementId::NamedInteger(format!("bar-{}", x.id).into(), i as u64),
                                        Animation::new(Duration::from_millis(700)).with_easing(ease_out_quint()),
                                        move |el, k| el.w(relative(w * k)),
                                    ),
                                ),
                            )
                        })))
                    }),
                    ElementId::NamedInteger("gcard".into(), gi as u64 + nav * 10),
                    80 * gi as u64,
                )
            }))
            .child(div().h(px(20.)));

        div()
            .flex_1()
            .min_h(px(0.))
            .flex()
            .flex_col()
            .child(list)
            .child(
                div().px(px(22.)).pt(px(10.)).pb(px(16.)).flex().flex_col().gap(px(4.)).border_t_2().border_color(rgb(LINE)).bg(rgb(BG))
                    .child(button("clean-safe", if safe > 0 { format!("Clean {}", format_bytes(safe)) } else { "Nothing safe to clean".into() }, Btn::Primary(MINT), safe > 0).on_click(cx.listener(|this, _, _, cx| {
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
        let sel_bytes = self.selected_bytes();
        let sel_n = self.selected.len();
        let dry = self.dry_run;
        let chip = |id: &'static str, label: &'static str, on: bool| {
            div().id(id).on_mouse_down(MouseButton::Left, |_, _, _| haptic(Haptic::Tick)).px(px(12.)).py(px(7.)).rounded_full().border_2().text_size(px(13.)).font_weight(FontWeight::BLACK).cursor_pointer()
                .border_color(rgb(if on { GRAPE.base } else { LINE })).bg(rgb(if on { GRAPE.soft } else { CARD })).text_color(rgb(if on { GRAPE.base } else { INK }))
                .hover(|s| s.border_color(rgb(GRAPE.base)))
                .child(label)
        };
        let all_safe = self.selected == self.targets.iter().filter(|t| t.default_on).map(|t| t.id.clone()).collect::<HashSet<_>>();
        let all = self.selected.len() == self.targets.len();

        let list = div()
            .id("pick-list")
            .flex_1()
            .min_h(px(0.))
            .overflow_y_scroll()
            .px(px(22.))
            .child(div().text_size(px(26.)).font_weight(FontWeight::BLACK).child("What should Dusty eat?"))
            .child(div().text_size(px(14.)).font_weight(FontWeight::SEMIBOLD).text_color(rgb(MUTED)).mt(px(2.)).child("Pre-ticked items rebuild themselves. Purple ones cost a download later."))
            .child(
                div().flex().flex_wrap().items_center().gap(px(8.)).mt(px(12.))
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
                        div().id("dry").on_mouse_down(MouseButton::Left, |_, _, _| haptic(Haptic::Tick)).flex().items_center().gap(px(8.)).cursor_pointer().text_size(px(13.)).font_weight(FontWeight::BLACK).text_color(rgb(MUTED))
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.dry_run = !this.dry_run;
                                this.bubble = if this.dry_run { "Practice run: I'll only pretend.".into() } else { "For real this time!".into() };
                                cx.notify();
                            }))
                            .child("Practice run")
                            .child(
                                div().w(px(44.)).h(px(26.)).rounded_full().bg(rgb(if dry { SKY.base } else { LINE })).relative().child(
                                    div().absolute().top(px(3.)).size(px(20.)).rounded_full().bg(rgb(0xFFFFFF)).shadow(vec![BoxShadow { color: alpha(0x000000, 0.12), offset: point(px(0.), px(2.)), blur_radius: px(0.), spread_radius: px(0.) }])
                                        .with_animation(ElementId::NamedInteger("knob".into(), dry as u64), Animation::new(Duration::from_millis(300)), move |el, t| {
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
                div().mt(px(20.)).flex().flex_col().gap(px(10.))
                    .child(
                        div().flex().items_end().gap(px(8.))
                            .child(div().text_size(px(17.)).font_weight(FontWeight::BLACK).text_color(rgb(t.edge)).child(format!("{} {}", glyph(g), g.name())))
                            .child(div().flex_1().min_w(px(0.)).truncate().child(label_small(g.blurb())))
                            .child(
                                div().id(ElementId::Name(format!("all-{:?}", g).into())).text_size(px(12.)).font_weight(FontWeight::BLACK).text_color(rgb(t.edge)).cursor_pointer().on_mouse_down(MouseButton::Left, |_, _, _| haptic(Haptic::Tick)).child("ALL")
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        let all_on = ids.iter().all(|i| this.selected.contains(i));
                                        for i in &ids {
                                            if all_on { this.selected.remove(i); } else { this.selected.insert(i.clone()); }
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
                        div()
                            .id(ElementId::Name(format!("item-{}", x.id).into()))
                            .flex()
                            .items_center()
                            .gap(px(12.))
                            .px(px(14.))
                            .py(px(12.))
                            .rounded(px(16.))
                            .border_2()
                            .border_b_4()
                            .border_color(rgb(if on { t.base } else { LINE }))
                            .bg(rgb(if on { t.soft } else { CARD }))
                            .cursor_pointer()
                            .hover(|s| s.border_color(rgb(t.base)))
                            .active(|s| s.border_b_2().mt(px(2.)))
                            .on_mouse_down(MouseButton::Left, |_, _, _| haptic(Haptic::Tick))
                            .on_click(cx.listener(move |this, _, _, cx| this.toggle(&id, cx)))
                            .child(
                                div().size(px(26.)).flex_none().rounded(px(8.)).border_2().border_color(rgb(if on { t.base } else { LINE })).bg(rgb(if on { t.base } else { CARD })).flex().items_center().justify_center()
                                    .text_color(rgb(0xFFFFFF)).text_size(px(15.)).font_weight(FontWeight::BLACK)
                                    .when(on, |d| d.child(div().child("✓").with_animation(ElementId::Name(format!("ck-{}", x.id).into()), Animation::new(Duration::from_millis(260)), |el, k| el.opacity(k).text_size(px(6. + 9. * back_out(k)))))),
                            )
                            .child(
                                div().flex_1().min_w(px(0.)).flex().flex_col()
                                    .child(div().flex().items_center().gap(px(6.)).child(div().truncate().text_size(px(15.)).font_weight(FontWeight::BLACK).child(x.label.clone())).when(risky, |d| d.child(div().px(px(6.)).rounded(px(6.)).bg(rgb(CORAL.soft)).text_color(rgb(CORAL.edge)).text_size(px(11.)).font_weight(FontWeight::BLACK).child("wipes data"))))
                                    .child(label_small(x.detail.clone().unwrap_or_else(|| if !x.paths.is_empty() { format!("{} paths", x.paths.len()) } else if let Some(c) = x.commands.first() { format!("runs {}", c.cmd) } else { String::new() }))),
                            )
                            .child(div().font_weight(FontWeight::BLACK).text_color(rgb(if on { t.edge } else { INK })).child(format_bytes(x.bytes)))
                    }))
            }))
            .child(div().h(px(20.)));

        div().flex_1().min_h(px(0.)).flex().flex_col().child(list).child(
            div().px(px(22.)).pt(px(8.)).pb(px(16.)).border_t_2().border_color(rgb(LINE)).bg(rgb(CARD)).flex().flex_col().gap(px(6.))
                .child(
                    div().flex().items_center().gap(px(10.)).child(div().flex_none().child(self.dusty.clone())).child(
                        div().flex().flex_col().flex_1().min_w(px(0.))
                            .child(div().flex().items_end().gap(px(6.)).child(div().text_size(px(15.)).font_weight(FontWeight::BLACK).child("Free up")).child(div().text_size(px(24.)).font_weight(FontWeight::BLACK).text_color(rgb(MINT.edge)).child(format_bytes(sel_bytes))).child(label_small(format!("· {sel_n} items"))))
                            .child(div().text_size(px(13.)).font_weight(FontWeight::SEMIBOLD).text_color(rgb(MUTED)).truncate().child(self.bubble.clone())),
                    ),
                )
                .child(button("pick-go", if dry { format!("Practice with {sel_n} items") } else { format!("Clean {sel_n} items") }, Btn::Primary(MINT), sel_n > 0).on_click(cx.listener(|this, _, _, cx| this.open_confirm(cx)))),
        )
    }

    fn confirm_sheet(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let items = self.selected_targets();
        let n = items.len();
        let bytes: u64 = items.iter().map(|t| t.bytes).sum();
        let risky = items.iter().filter(|t| t.wipes_data()).count();
        let redl = items.iter().filter(|t| t.group == Group::Redownload).count();
        let dry = self.dry_run;
        div()
            .id("sheet-wrap")
            .absolute()
            .inset_0()
            .occlude()
            .flex()
            .flex_col()
            .justify_end()
            .child(
                div().id("sheet-bg").absolute().inset_0().bg(alpha(INK, 0.45)).on_click(cx.listener(|this, _, _, cx| {
                    this.confirm = false;
                    cx.notify();
                }))
                .with_animation("sheet-bg-in", Animation::new(Duration::from_millis(220)), |el, t| el.opacity(t)),
            )
            .child(
                div()
                    .id("sheet")
                    .relative()
                    .bg(rgb(CARD))
                    .rounded_t(px(28.))
                    .px(px(22.))
                    .pt(px(22.))
                    .pb(px(20.))
                    .flex()
                    .flex_col()
                    .items_center()
                    .gap(px(8.))
                    .child(div().text_size(px(40.)).child(if risky > 0 { "😬" } else if dry { "🧪" } else { "🧹" }))
                    .child(div().text_size(px(22.)).font_weight(FontWeight::BLACK).text_center().child(if dry { format!("Practice with {n} items?") } else { format!("Delete {n} items, about {}?", format_bytes(bytes)) }))
                    .child(div().text_size(px(14.)).font_weight(FontWeight::SEMIBOLD).text_color(rgb(MUTED)).text_center().child(if dry {
                        "Nothing will be deleted. Dusty just rehearses.".to_string()
                    } else if risky > 0 {
                        format!("{risky} of them wipe real data (not just caches). This can't be undone.")
                    } else if redl > 0 {
                        format!("{redl} will need a re-download next time you use them. The rest rebuilds by itself.")
                    } else {
                        "All of it rebuilds by itself when needed. Nothing personal is touched.".to_string()
                    }))
                    .child(div().h(px(6.)))
                    .child(button("confirm-go", if dry { "Start practice" } else { "Yes, clean it" }, Btn::Primary(if risky > 0 { CORAL } else { MINT }), true).on_mouse_up(MouseButton::Left, move |_, _, _| if !dry { haptic(Haptic::Double) }).on_click(cx.listener(|this, _, _, cx| this.start_clean(cx))))
                    .child(button("confirm-no", "Not yet", Btn::Ghost, true).on_click(cx.listener(|this, _, _, cx| {
                        this.confirm = false;
                        cx.notify();
                    })))
                    .with_animation("sheet-in", Animation::new(Duration::from_millis(440)), |el, t| el.mb(px(-360. * (1. - back_out(t))))),
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
            div().flex().items_center().gap(px(10.)).px(px(14.)).py(px(10.)).rounded(px(16.)).bg(rgb(CARD)).border_2().border_b_4().border_color(rgb(tn.base))
                .child(div().size(px(34.)).flex_none().rounded(px(10.)).bg(rgb(tn.soft)).flex().items_center().justify_center().text_size(px(17.)).child(glyph(t.group)))
                .child(div().flex().flex_col().min_w(px(0.)).child(div().text_size(px(15.)).font_weight(FontWeight::BLACK).truncate().max_w(px(200.)).child(t.label.clone())).child(div().text_size(px(12.)).font_weight(FontWeight::BLACK).text_color(rgb(tn.edge)).child(format_bytes(t.bytes))))
        };

        let mut col = self.page().items_center().relative();
        col = col
            .child(
                div().h(px(104.)).flex().flex_col().items_center().gap(px(6.)).pt(px(4.))
                    .when_some(plate_item, |d, t| {
                        let card = item_card(&t);
                        let card = if shaking {
                            card.with_animation(ElementId::NamedInteger("shake".into(), job.next as u64), Animation::new(Duration::from_millis(420)), |el, k| el.ml(px((k * 40.).sin() * 10. * (1. - k)))).into_any_element()
                        } else if flying {
                            // next one slides up into place while the current one is in the air
                            card.opacity(0.55).with_animation(ElementId::NamedInteger("next".into(), job.next as u64), Animation::new(Duration::from_millis(380)).with_easing(ease_out_quint()), |el, k| el.mt(px(26. * (1. - k)))).into_any_element()
                        } else {
                            card.with_animation(ElementId::NamedInteger("wobble".into(), job.next as u64), Animation::new(Duration::from_millis(900)).repeat().with_easing(ease_in_out), |el, k| el.mt(px((k * std::f32::consts::TAU).sin() * 2.))).into_any_element()
                        };
                        d.child(card)
                    })
                    .child(label_small(if waiting && !shaking { "Working on it…".to_string() } else if remaining > 0 { format!("+{remaining} more after this") } else { String::new() })),
            )
            .child(div().mt(px(6.)).child(self.dusty.clone()))
            .child(bubble(self.bubble.clone(), job.next as u64 + 7777))
            .child(div().text_size(px(46.)).font_weight(FontWeight::BLACK).text_color(rgb(MINT.edge)).mt(px(8.)).child(format_bytes(job.shown as u64)))
            .child(label_small(if job.dry { "would be freed (practice run)" } else { "eaten so far" }))
            .child(div().w_full().mt(px(12.)).child(progress_bar(job.shown_progress, MINT)))
            .child(label_small(format!("{} of {}", job.next.min(n), n)).mt(px(6.)));

        // the flying item: an arc from the plate into Dusty's mouth, shrinking as it goes
        let flight = job.flight.as_ref().and_then(|f| {
            let m = self.dusty.read(_cx).mouth_point()?;
            let p = (f.t0.elapsed().as_secs_f32() / 0.8).min(1.);
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
            .when(failed > 0, |d| d.child(div().px(px(22.)).pb(px(14.)).child(label_small(format!("{failed} item(s) were busy and got skipped")))))
            .child(div().h(px(14.)))
            // overlay in window coordinates (this column starts right under the top band)
            .when_some(flight, |d, (t, x, y, s, p)| {
                let tn = tone(t.group);
                let w = 210. * s;
                d.child(
                    div().absolute().left(px(x - w / 2.)).top(px(y - TOP - 26. * s)).w(px(w)).opacity((1.4 - p).min(1.)).child(
                        div().flex().items_center().gap(px(8. * s)).px(px(12. * s)).py(px(10. * s)).rounded(px(16. * s)).bg(rgb(CARD)).border_2().border_b_4().border_color(rgb(tn.base))
                            .shadow(vec![BoxShadow { color: alpha(INK, 0.15), offset: point(px(0.), px(8. * s)), blur_radius: px(16. * s), spread_radius: px(0.) }])
                            .child(div().text_size(px(18. * s)).child(glyph(t.group)))
                            .child(div().text_size(px((15. * s).max(5.))).font_weight(FontWeight::BLACK).truncate().child(t.label.clone())),
                    ),
                )
            })
            .children(popups.into_iter().map(|(text, x, y, age)| {
                let k = (age / 1.1).min(1.);
                div().absolute().left(px(x - 60.)).w(px(120.)).top(px(y - TOP - 150. - 50. * k)).flex().justify_center().opacity(1. - k * k)
                    .child(div().text_size(px(20.)).font_weight(FontWeight::BLACK).text_color(rgb(MINT.edge)).child(text))
            }))
            .into_any_element()
    }

    fn done(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let job_dry = self.clean.as_ref().map(|j| j.dry).unwrap_or(false);
        let ok = self.clean.as_ref().map(|j| j.ok_count).unwrap_or(0);
        let failed = self.clean.as_ref().map(|j| j.failed.clone()).unwrap_or_default();
        let freed = self.done_freed;
        let estimated = self.done_estimated;
        let disk = self.done_disk;
        let nav = self.nav;
        let stat = |label: &'static str, value: String, t: Tone, delay: u64, i: u64| {
            let total = 420 + delay;
            let start = delay as f32 / total as f32;
            div().flex_1().flex_basis(px(0.)).rounded(px(18.)).bg(rgb(t.base)).border_2().border_b_4().border_color(rgb(t.base)).flex().flex_col()
                .child(div().rounded_t(px(16.)).bg(rgb(t.base)).py(px(5.)).flex().justify_center().text_color(rgb(0xFFFFFF)).text_size(px(12.)).font_weight(FontWeight::BLACK).child(label.to_uppercase()))
                .child(div().rounded(px(14.)).bg(rgb(CARD)).py(px(10.)).flex().justify_center().text_size(px(22.)).font_weight(FontWeight::BLACK).text_color(rgb(t.edge)).child(value))
                .with_animation(ElementId::NamedInteger("stat".into(), nav * 10 + i), Animation::new(Duration::from_millis(total)), move |el, k| {
                    let k = ((k - start) / (1. - start)).clamp(0., 1.);
                    el.opacity(k).mt(px(14. * (1. - back_out(k))))
                })
        };
        // note: no w_full() inside scroll containers (percentages don't resolve there); stretch instead
        let list = div()
            .id("done-list")
            .flex_1()
            .min_h(px(0.))
            .overflow_y_scroll()
            .px(px(22.))
            .flex()
            .flex_col()
            .child(
                div().flex().flex_col().items_center()
                    .child(bubble(self.bubble.clone(), nav))
                    .child(div().mt(px(4.)).child(self.dusty.clone()))
                    .child(div().text_size(px(15.)).font_weight(FontWeight::BLACK).text_color(rgb(MUTED)).child(if job_dry { "Would free" } else if estimated { "Freed (estimated)" } else { "Freed" }))
                    .child(div().text_size(px(52.)).font_weight(FontWeight::BLACK).text_color(rgb(MINT.edge)).child(format_bytes(freed))),
            )
            .child(div().flex().gap(px(10.)).mt(px(12.)).child(stat("Free space now", format_bytes(disk.free), MINT, 100, 0)).child(stat("Items cleaned", ok.to_string(), SKY, 220, 1)))
            .when(!failed.is_empty(), |d| {
                d.child(
                    div().mt(px(12.)).p(px(14.)).rounded(px(16.)).bg(rgb(SUN.soft)).text_color(rgb(0x8A6000)).text_size(px(14.)).font_weight(FontWeight::BOLD).flex().flex_col().gap(px(4.))
                        .child(div().font_weight(FontWeight::BLACK).child(format!("{} item(s) were busy", failed.len())))
                        .children(failed.iter().take(4).map(|(l, e)| div().truncate().child(format!("{l}: {e}"))))
                        .child(div().child("Something was using them. Try again later, no hard feelings.")),
                )
            })
            .child(
                card().mt(px(14.)).p(px(16.)).flex().flex_col().gap(px(2.))
                    .child(div().text_size(px(17.)).font_weight(FontWeight::BLACK).mb(px(6.)).child("Worth checking by hand"))
                    .children(HINTS.iter().enumerate().map(|(i, (t, d))| {
                        let on = self.hints_done.contains(&i);
                        div().id(("hint", i)).on_mouse_down(MouseButton::Left, |_, _, _| haptic(Haptic::Tick)).flex().items_center().gap(px(10.)).py(px(8.)).border_t_2().border_color(rgb(if i == 0 { CARD } else { LINE })).cursor_pointer()
                            .on_click(cx.listener(move |this, _, _, cx| {
                                if !this.hints_done.remove(&i) {
                                    this.hints_done.insert(i);
                                }
                                cx.notify();
                            }))
                            .child(div().size(px(22.)).flex_none().rounded_full().border_2().border_color(rgb(if on { MINT.base } else { LINE })).bg(rgb(if on { MINT.base } else { CARD })).flex().items_center().justify_center().text_color(rgb(0xFFFFFF)).text_size(px(12.)).font_weight(FontWeight::BLACK).when(on, |d| d.child("✓")))
                            .child(div().flex().flex_col().min_w(px(0.)).child(div().text_size(px(15.)).font_weight(FontWeight::BLACK).text_color(rgb(if on { MUTED } else { INK })).when(on, |d| d.line_through()).child(*t)).child(label_small(*d)))
                    })),
            )
            .child(div().h(px(14.)));
        div().flex_1().min_h(px(0.)).flex().flex_col().child(list).child(
            div().px(px(22.)).pt(px(10.)).pb(px(16.)).border_t_2().border_color(rgb(LINE)).child(button("done-home", "Done", Btn::Primary(MINT), true).on_click(cx.listener(|this, _, _, cx| {
                this.clean = None;
                this.targets.clear();
                this.enter_home(cx);
            }))),
        )
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
                        let purple = rgb(GRAPE.base);
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
                        .shadow(vec![BoxShadow { color: alpha(GRAPE.edge, 0.6), offset: point(px(0.), px(badge * 0.06)), blur_radius: px(0.), spread_radius: px(0.) }])
                        .flex()
                        .items_center()
                        .justify_center()
                        .text_color(rgb(GRAPE.base))
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
