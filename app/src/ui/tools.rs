//! The tools reached from home: Uninstall apps, Big & old files, What's using space, History and Settings.
//!
//! Same pattern as the checkup: anything slow runs on a background thread with a cancel flag and a progress
//! struct the render loop polls, and the result comes back through `cx.spawn`. Removals always go through the
//! confirm sheet and end up in the Trash, with "Put back" right there and in History.

mod apps;
mod big_files;
mod past_cleans;
mod settings;
mod space;

use super::*;
use dusty_core::history::{load_history, put_back, HistoryEntry};
use dusty_core::large::{FileKind, LargeFile, LargeFileOptions, LargeProgress};
use dusty_core::schedule::{self, Schedule};
use dusty_core::uninstall::{self, AppListOptions, AppListProgress, InstalledApp, Leftover, UninstallReport};
use dusty_core::usage::{NodeKind, Rect, UsageNode, UsageOptions, UsageProgress};
use dusty_core::{large, tildify, usage};
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::atomic::Ordering;
use std::time::SystemTime;

/// Dusty's size in a tool's header.
const TOOL_DUSTY: f32 = 96.;
/// "Big find" thresholds for Dusty's wow.
const WOW_FILE: u64 = 4 * 1024 * 1024 * 1024;
const WOW_APP: u64 = 5 * 1024 * 1024 * 1024;

/// A background job the render loop polls: live progress and a cancel flag.
pub(super) struct Job<P> {
    progress: Arc<Mutex<P>>,
    cancel: Arc<AtomicBool>,
}

impl<P: Default> Job<P> {
    fn new() -> Self {
        Job { progress: Arc::new(Mutex::new(P::default())), cancel: Arc::new(AtomicBool::new(false)) }
    }
}

impl<P> Job<P> {
    fn stop(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
    /// Is `flag` this job's cancel flag, and was it left running? (A result from a stopped or replaced job is dropped.)
    fn is(&self, flag: &Arc<AtomicBool>) -> bool {
        Arc::ptr_eq(&self.cancel, flag) && !flag.load(Ordering::Relaxed)
    }
}

/// Where a "Put back" was pressed, so its result lands in the right place.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum PutBackFor {
    Clean,
    Uninstall,
    Large,
    History,
}

pub(super) struct HistRow {
    entry: HistoryEntry,
    /// paths still in the Trash (checked when the history was loaded)
    restorable: usize,
}

pub(super) struct Tools {
    // ── uninstall ──
    apps_job: Option<Job<AppListProgress>>,
    apps: Option<Vec<InstalledApp>>,
    icons: Arc<Mutex<HashMap<PathBuf, Arc<Image>>>>,
    icons_loading: Arc<AtomicBool>,
    search: String,
    app: Option<InstalledApp>,
    left_job: Option<Job<()>>,
    leftovers: Option<Vec<Leftover>>,
    left_sel: HashSet<PathBuf>,
    app_busy: bool,
    app_result: Option<UninstallReport>,
    app_put_back: PutBack,
    // ── big & old files ──
    large_job: Option<Job<LargeProgress>>,
    large_seen: usize,
    large_found: u64,
    /// Shared, so render can hold the list without copying it each frame.
    large: Option<Rc<Vec<LargeFile>>>,
    large_partial: bool,
    large_sel: HashSet<PathBuf>,
    large_busy: bool,
    large_result: Option<CleanReport>,
    large_put_back: PutBack,
    // ── what's using space ──
    usage_job: Option<Job<UsageProgress>>,
    /// breadcrumb: home first, the folder on screen last
    /// Shared, so render can hold the current level without copying the whole tree each frame.
    usage_stack: Vec<Rc<UsageNode>>,
    usage_hover: Option<usize>,
    // ── history ──
    history: Option<Vec<HistRow>>,
    history_busy: Option<String>,
    // ── settings ──
    scheduled: Option<bool>,
    schedule_busy: bool,
    schedule_note: Option<String>,
    ignore: Option<Result<Vec<String>, String>>,
    // ── shared ──
    focus: Option<usize>,
    ring: bool,
    scroll: ScrollHandle,
    hovered: Option<String>,
    /// Dusty eats this many more times, one every few frames, while things go to the Trash
    chomps: u32,
    next_chomp: Instant,
    /// the home bubble while a tool tile is hovered (put back when the mouse leaves)
    home_saved: Option<SharedString>,
    bubble_gen: u64,
}

impl Default for Tools {
    fn default() -> Self {
        Tools {
            apps_job: None,
            apps: None,
            icons: Arc::new(Mutex::new(HashMap::new())),
            icons_loading: Arc::new(AtomicBool::new(false)),
            search: String::new(),
            app: None,
            left_job: None,
            leftovers: None,
            left_sel: HashSet::new(),
            app_busy: false,
            app_result: None,
            app_put_back: PutBack::No,
            large_job: None,
            large_seen: 0,
            large_found: 0,
            large: None,
            large_partial: false,
            large_sel: HashSet::new(),
            large_busy: false,
            large_result: None,
            large_put_back: PutBack::No,
            usage_job: None,
            usage_stack: vec![],
            usage_hover: None,
            history: None,
            history_busy: None,
            scheduled: None,
            schedule_busy: false,
            schedule_note: None,
            ignore: None,
            focus: None,
            ring: false,
            scroll: ScrollHandle::new(),
            hovered: None,
            chomps: 0,
            next_chomp: Instant::now(),
            home_saved: None,
            bubble_gen: 0,
        }
    }
}

// ───────────────────────── small helpers ─────────────────────────

fn secs_since(t: SystemTime) -> u64 {
    SystemTime::now().duration_since(t).map(|d| d.as_secs()).unwrap_or(0)
}

/// "Sat 21 Nov, 10:33" in local time (the year too when it isn't this one).
fn local_date(ts: u64) -> String {
    const DAYS: [&str; 7] = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];
    const MONTHS: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
    let tm_of = |secs: u64| {
        let t = secs as libc::time_t;
        let mut tm: libc::tm = unsafe { std::mem::zeroed() };
        unsafe { libc::localtime_r(&t, &mut tm) };
        tm
    };
    let tm = tm_of(ts);
    let year = if tm.tm_year == tm_of(now_secs()).tm_year { String::new() } else { format!(" {}", tm.tm_year + 1900) };
    format!("{} {} {}{year}, {:02}:{:02}", DAYS[tm.tm_wday.clamp(0, 6) as usize], tm.tm_mday, MONTHS[tm.tm_mon.clamp(0, 11) as usize], tm.tm_hour, tm.tm_min)
}

/// A big file's icon and tint: the tint tells the kinds apart; installers get the package, the rest the file.
fn kind_look(k: FileKind) -> (Ico, Tone) {
    let p = pal();
    match k {
        FileKind::Video => (Ico::BigFiles, p.coral),
        FileKind::Audio => (Ico::BigFiles, p.grape),
        FileKind::Image => (Ico::BigFiles, p.sky),
        FileKind::DiskImage => (Ico::BigFiles, p.blue),
        FileKind::Installer => (Ico::Redownload, p.sun),
        FileKind::Archive => (Ico::BigFiles, p.teal),
        FileKind::Document => (Ico::BigFiles, p.mint),
        FileKind::Other => (Ico::BigFiles, p.grape),
    }
}

/// A small chunky button for inside cards ("Put back", "Check again").
fn mini_button(id: impl Into<ElementId>, label: impl Into<SharedString>, t: Tone, enabled: bool) -> Stateful<Div> {
    mini_button_icon(id, None, label, t, enabled)
}

/// `mini_button` with an icon before its label ("Put back").
fn mini_button_icon(id: impl Into<ElementId>, ico: Option<Ico>, label: impl Into<SharedString>, t: Tone, enabled: bool) -> Stateful<Div> {
    div()
        .id(id)
        .flex_none()
        .h(px(HIT + 4.))
        .px(px(sp::M))
        .flex()
        .items_center()
        .justify_center()
        .rounded_full()
        .border_2()
        .border_b_4()
        .border_color(rgb(if enabled { t.base } else { pal().line }))
        .bg(rgb(pal().card))
        .text_size(px(ts::SMALL))
        .font_weight(FontWeight::BLACK)
        .text_color(rgb(if enabled { t.text } else { pal().muted }))
        .when_some(ico, |d, i| d.pl(px(sp::S + 2.)).gap(px(sp::XS + 2.)).child(icon(i, 18.).when(!enabled, |d| d.opacity(0.5))))
        .child(label.into())
        .when(enabled, |d| {
            d.cursor_pointer()
                .hover(move |s| s.bg(rgb(t.soft)))
                .active(|s| s.border_b_2().mt(px(2.)))
                .on_mouse_down(MouseButton::Left, |_, _, cx| {
                    haptic(Haptic::Click);
                    cx.stop_propagation()
                })
        })
        // disabled: a shield takes the mouse so a caller's `on_click` can never fire
        .when(!enabled, |d| d.relative().child(div().absolute().inset_0().occlude()))
}

/// A soft notice card (busy apps, admin needed, partial results).
fn notice(t: Tone, text_color: u32) -> Div {
    div().p(px(CARD_PAD)).rounded(px(radius::L)).bg(rgb(t.soft)).text_color(rgb(text_color)).text_size(px(ts::BODY)).line_height(px(ts::BODY * 1.4)).font_weight(FontWeight::BOLD).flex().flex_col().gap(px(sp::XS))
}

/// An ignore-list entry as people read it: project folders as `~/…` paths (with a folder), item ids by name.
fn ignore_display(e: &str) -> (Option<Ico>, String) {
    match dusty_core::IgnoreEntry::parse(e) {
        dusty_core::IgnoreEntry::Project(p) => (Some(Ico::Reveal), tildify(&p)),
        dusty_core::IgnoreEntry::Item(id) => (Some(Ico::Never), id),
        dusty_core::IgnoreEntry::Path(p) => (None, p),
    }
}

/// Section caption inside a scroll list.
fn caption(s: impl Into<SharedString>) -> Div {
    label_small(s).mt(px(sp::L)).mb(px(sp::XS))
}

// ───────────────────────── navigation, keys, ticking ─────────────────────────

impl DustyApp {
    /// The row of tools under "Start checkup" on the home screen.
    pub(super) fn tool_row(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let p = pal();
        let tiles: [(Screen, Ico, &'static str, &'static str, Tone, &'static str); 4] = [
            (Screen::Apps, Ico::Uninstall, "Uninstall", "apps", p.sky, "Apps leave crumbs. I find those too."),
            (Screen::Large, Ico::BigFiles, "Big & old", "files", p.sun, "Forgotten giants? You pick, I chomp."),
            (Screen::Usage, Ico::Usage, "What's", "using space", p.grape, "A map of your disk, biggest first."),
            (Screen::History, Ico::History, "Clean", "history", p.teal, "What I ate, and what can go back."),
        ];
        div().w_full().flex().gap(px(sp::S)).children(tiles.into_iter().enumerate().map(|(i, (screen, e, l1, l2, t, tip))| {
            let look = (i as f32 - 1.5) / 1.5;
            div()
                .id(("tool", i))
                .group(SharedString::from(format!("tool-{i}")))
                .flex_1()
                .flex_basis(px(0.))
                .min_w(px(0.))
                .py(px(sp::S + sp::XXS))
                .rounded(px(radius::L))
                .bg(rgb(p.card))
                .border_2()
                .border_b_4()
                .border_color(rgb(p.line))
                .flex()
                .flex_col()
                .items_center()
                .gap(px(sp::XS))
                .cursor_pointer()
                .hover(move |s| s.border_color(rgb(t.base)).bg(rgb(pal().card_hover)))
                .active(|s| s.border_b_2().mt(px(2.)))
                .on_mouse_down(MouseButton::Left, |_, _, _| haptic(Haptic::Tick))
                .on_hover(cx.listener(move |this, on: &bool, _, cx| {
                    if *on {
                        if this.tools.home_saved.is_none() {
                            this.tools.home_saved = Some(this.bubble.clone());
                        }
                        this.bubble = tip.into();
                        this.dusty(cx, |d| d.st.look_at(Some((look * 0.7, 0.8))));
                    } else if let Some(b) = this.tools.home_saved.take() {
                        this.bubble = b;
                        this.dusty(cx, |d| d.st.look_at(None));
                    }
                    this.tools.bubble_gen += 1;
                    cx.notify();
                }))
                .on_click(cx.listener(move |this, _, _, cx| this.open_tool(screen, cx)))
                .child(icon_tile(e, t, 30.).group_hover(SharedString::from(format!("tool-{i}")), |s| s.mt(px(-2.))))
                .child(
                    div().w_full().px(px(sp::XS)).flex().flex_col().items_center().text_size(px(ts::CAPTION)).line_height(px(ts::CAPTION * 1.2)).font_weight(FontWeight::BLACK).text_color(rgb(p.ink))
                        .child(div().w_full().text_center().child(l1))
                        .child(div().w_full().text_center().text_color(rgb(t.text)).child(l2)),
                )
        }))
    }

    /// Home's bubble key: it changes as tool tiles are hovered, so each tip pops in.
    pub(super) fn home_bubble_key(&self) -> u64 {
        self.nav * 1000 + self.tools.bubble_gen
    }

    pub(super) fn open_tool(&mut self, s: Screen, cx: &mut Context<Self>) {
        if self.just_arrived() || self.confirm {
            return;
        }
        self.open_tool_now(s, cx);
    }

    fn open_tool_now(&mut self, s: Screen, cx: &mut Context<Self>) {
        let t = &mut self.tools;
        t.focus = None;
        t.ring = false;
        t.hovered = None;
        if let Some(b) = t.home_saved.take() {
            self.bubble = b;
        }
        self.dusty(cx, |d| {
            d.set_size(TOOL_DUSTY);
            d.st.look_at(None);
            d.st.dirt = 0.3;
            d.st.chub_target = 0.3;
        });
        match s {
            Screen::Apps => {
                if self.tools.apps.is_none() && self.tools.apps_job.is_none() {
                    self.start_apps(cx);
                } else {
                    self.bubble = "Which one should go? Type to search.".into();
                    self.dusty(cx, |d| d.st.set(Mood::Idle, None));
                }
            }
            Screen::Large => {
                if self.tools.large.is_none() && self.tools.large_job.is_none() {
                    self.start_large(cx);
                } else {
                    self.bubble = "Nothing is ticked. You pick what goes.".into();
                }
            }
            Screen::Usage => {
                if self.tools.usage_stack.is_empty() && self.tools.usage_job.is_none() {
                    self.start_usage(dusty_core::home(), cx);
                } else {
                    self.bubble = "Tap a block to look inside.".into();
                }
            }
            Screen::History => self.load_history(cx),
            Screen::Settings => self.load_settings(cx),
            _ => {}
        }
        self.go(s, cx);
    }

    /// `DUSTY_OPEN=apps|large|usage|history|settings` opens a tool at launch (for screenshots and testing).
    pub(super) fn open_from_env(&mut self, cx: &mut Context<Self>) {
        let s = match std::env::var("DUSTY_OPEN").as_deref() {
            Ok("apps") => Screen::Apps,
            Ok("large") => Screen::Large,
            Ok("usage") => Screen::Usage,
            Ok("history") => Screen::History,
            Ok("settings") => Screen::Settings,
            _ => return,
        };
        self.intro = None;
        self.open_tool_now(s, cx);
    }

    pub(super) fn tools_cancel_all(&mut self) {
        let t = &mut self.tools;
        for j in [t.apps_job.take().map(|j| j.cancel), t.left_job.take().map(|j| j.cancel), t.large_job.take().map(|j| j.cancel), t.usage_job.take().map(|j| j.cancel)].into_iter().flatten() {
            j.store(true, Ordering::Relaxed);
        }
    }

    fn tool_home(&mut self, cx: &mut Context<Self>) {
        // a finished list stays for next time; a half-done one is stopped
        let t = &mut self.tools;
        if let Some(j) = t.apps_job.take() {
            j.stop();
        }
        if let Some(j) = t.large_job.take() {
            j.stop();
        }
        if let Some(j) = t.usage_job.take() {
            j.stop();
        }
        self.enter_home(cx);
    }

    pub(super) fn tool_back(&mut self, cx: &mut Context<Self>) {
        if self.just_arrived() && self.screen != Screen::AppDetail {
            return;
        }
        match self.screen {
            Screen::Apps if !self.tools.search.is_empty() => {
                self.tools.search.clear();
                self.tools.focus = None;
                cx.notify();
            }
            Screen::AppDetail if self.tools.app_busy => {}
            Screen::AppDetail => self.back_to_apps(cx),
            Screen::Usage if self.tools.usage_job.is_some() && !self.tools.usage_stack.is_empty() => {
                // stop drilling into a folder, stay where we were
                if let Some(j) = self.tools.usage_job.take() {
                    j.stop();
                }
                self.dusty(cx, |d| d.st.set(Mood::Idle, None));
                cx.notify();
            }
            Screen::Usage if self.tools.usage_stack.len() > 1 => {
                self.tools.usage_stack.pop();
                self.tools.usage_hover = None;
                if let Some(n) = self.tools.usage_stack.last().cloned() {
                    self.usage_mood(&n, cx);
                }
                self.nav_t = Instant::now();
                cx.notify();
            }
            Screen::Large if self.tools.large_busy => {}
            _ => self.tool_home(cx),
        }
    }

    pub(super) fn tool_primary(&mut self, cx: &mut Context<Self>) {
        if self.just_arrived() {
            return;
        }
        match self.screen {
            Screen::Apps => {
                let apps = self.filtered_apps();
                let pick = match self.tools.focus.filter(|_| self.tools.ring) {
                    Some(i) => apps.get(i).cloned(),
                    // with a search that leaves one app, Return opens it
                    None if apps.len() == 1 => apps.first().cloned(),
                    None => None,
                };
                if let Some(a) = pick {
                    self.open_app(a, cx);
                }
            }
            Screen::AppDetail => {
                if self.tools.app_result.is_some() {
                    self.back_to_apps(cx);
                } else if self.can_uninstall() {
                    self.open_sheet(ConfirmKind::Uninstall, cx);
                }
            }
            Screen::Large => {
                if !self.tools.large_sel.is_empty() && !self.tools.large_busy {
                    self.open_sheet(ConfirmKind::TrashFiles, cx);
                }
            }
            Screen::Usage => {
                if let Some(i) = self.tools.usage_hover {
                    self.usage_click(i, cx);
                }
            }
            _ => {}
        }
    }

    /// ↑ ↓ on the app list and the big files list.
    pub(super) fn tool_move_focus(&mut self, delta: i32, cx: &mut Context<Self>) {
        let n = match self.screen {
            Screen::Apps => self.filtered_apps().len(),
            Screen::Large => self.tools.large.as_ref().map(|l| l.len()).unwrap_or(0),
            _ => 0,
        };
        if n == 0 {
            return;
        }
        let t = &mut self.tools;
        let next = match (t.focus.filter(|i| *i < n), t.ring) {
            (Some(i), false) => i,
            (None, _) => if delta > 0 { 0 } else { n - 1 },
            (Some(i), true) => (i as i32 + delta).clamp(0, n as i32 - 1) as usize,
        };
        t.scroll.scroll_to_item(next);
        t.focus = Some(next);
        t.ring = true;
        cx.notify();
    }

    /// Space: ticks the focused big file. False when the key should type a space instead (app search).
    pub(super) fn tool_toggle(&mut self, cx: &mut Context<Self>) -> bool {
        match self.screen {
            Screen::Apps => false,
            Screen::Large => {
                if let Some(f) = self.tools.focus.filter(|_| self.tools.ring).and_then(|i| self.tools.large.as_ref()?.get(i).map(|f| f.path.clone())) {
                    haptic(Haptic::Tick);
                    self.toggle_large(&f, cx);
                } else {
                    self.tool_move_focus(1, cx);
                }
                true
            }
            _ => true,
        }
    }

    /// ⌘R: look again.
    pub(super) fn tool_rescan(&mut self, cx: &mut Context<Self>) {
        match self.screen {
            Screen::Apps => self.start_apps(cx),
            Screen::Large if !self.tools.large_busy => self.start_large(cx),
            Screen::Usage => {
                if let Some(p) = self.tools.usage_stack.last().and_then(|n| n.path.clone()) {
                    self.tools.usage_stack.pop();
                    self.start_usage(p, cx);
                }
            }
            Screen::History => self.load_history(cx),
            Screen::Settings => self.load_settings(cx),
            _ => {}
        }
    }

    /// Typing on Uninstall apps filters the list.
    pub(super) fn tool_key(&mut self, e: &KeyDownEvent, cx: &mut Context<Self>) {
        if self.screen != Screen::Apps || self.modal_open() {
            return;
        }
        let k = &e.keystroke;
        if k.modifiers.platform || k.modifiers.control {
            return;
        }
        if k.key == "backspace" {
            if self.tools.search.pop().is_some() {
                self.tools.focus = None;
                cx.notify();
            }
            return;
        }
        if let Some(c) = k.key_char.as_deref().filter(|c| !c.chars().any(|ch| ch.is_control())) {
            if self.tools.search.chars().count() < 40 && !(c == " " && self.tools.search.is_empty()) {
                self.tools.search.push_str(c);
                self.tools.focus = None;
                self.tools.scroll.scroll_to_item(0);
                cx.notify();
            }
        }
    }

    /// Per frame: Dusty's chomps and big-find reactions. True while something is moving or loading.
    pub(super) fn tick_tools(&mut self, _dt: f32, cx: &mut Context<Self>) -> bool {
        let t = &mut self.tools;
        let mut moving = t.apps_job.is_some() || t.left_job.is_some() || t.large_job.is_some() || t.usage_job.is_some() || t.app_busy || t.large_busy || t.schedule_busy || t.history_busy.is_some() || t.icons_loading.load(Ordering::Relaxed);
        let mut wow = None;
        if let Some(j) = &t.large_job {
            let p = j.progress.lock().unwrap();
            for f in p.found.iter().skip(t.large_seen) {
                t.large_found += f.bytes;
                if f.bytes >= WOW_FILE {
                    wow = Some(format!("Whoa, {} is {}!", dusty_core::name_of(&f.path), format_bytes(f.bytes)));
                }
            }
            t.large_seen = p.found.len();
        }
        let chomp = t.chomps > 0 && Instant::now() >= t.next_chomp;
        if chomp {
            t.chomps -= 1;
            t.next_chomp = Instant::now() + Duration::from_millis(300);
        }
        if t.chomps > 0 {
            moving = true;
        }
        if let Some(w) = wow {
            self.bubble = w.into();
            self.dusty(cx, |d| d.st.set(Mood::Wow, Some(0.8)));
        }
        if chomp {
            haptic(Haptic::Tick);
            self.dusty(cx, |d| d.st.chomp());
        }
        moving
    }

    pub(super) fn tool_screen(&mut self, cx: &mut Context<Self>) -> AnyElement {
        match self.screen {
            Screen::Apps => self.apps_screen(cx).into_any_element(),
            Screen::AppDetail => self.app_screen(cx).into_any_element(),
            Screen::Large => self.large_screen(cx).into_any_element(),
            Screen::Usage => self.usage_screen(cx).into_any_element(),
            Screen::History => self.history_screen(cx).into_any_element(),
            Screen::Settings => self.settings_screen(cx).into_any_element(),
            _ => div().into_any_element(),
        }
    }

    /// Dusty with the tool's title and his bubble as the subtitle.
    fn tool_header(&self, title: impl Into<SharedString>) -> Div {
        div().flex().items_center().gap(px(sp::M)).child(div().flex_none().child(self.dusty.clone())).child(
            div().flex().flex_col().gap(px(sp::XS)).flex_1().min_w(px(0.))
                .child(div().text_size(px(ts::H1)).line_height(px(ts::H1 * 1.15)).font(tabular(FontWeight::BLACK)).child(title.into()))
                .child(subtitle(self.bubble.clone())),
        )
    }

    /// While a tool is looking: a counter, the busy bar and a Stop button.
    fn tool_busy(&self, title: &str, big: String, what: String, note: String, cx: &mut Context<Self>) -> Div {
        self.page()
            .child(self.tool_header(title.to_string()))
            .child(
                card().w_full().mt(px(sp::L)).p(px(CARD_PAD)).flex().flex_col().items_center().gap(px(sp::S))
                    .child(number(big, ts::HERO - 6., pal().grape.base))
                    .child(label_small(what))
                    .child(div().w_full().mt(px(sp::S)).child(busy_bar()))
                    .child(label_small(note).font(tabular(FontWeight::BOLD)).one_line()),
            )
            .child(div().flex_1())
            .child(button("tool-stop", "Stop", Btn::Ghost, true).on_click(cx.listener(|this, _, _, cx| this.tool_home(cx))))
            .child(div().h(px(sp::L)))
    }

    fn tool_ring(&self, i: usize) -> bool {
        self.tools.ring && self.tools.focus == Some(i)
    }

    fn set_tool_hover(&mut self, id: &str, on: bool, cx: &mut Context<Self>) {
        let before = self.tools.hovered.clone();
        if on {
            self.tools.hovered = Some(id.to_string());
        } else if self.tools.hovered.as_deref() == Some(id) {
            self.tools.hovered = None;
        }
        if before != self.tools.hovered {
            cx.notify();
        }
    }

    fn reveal_path(&mut self, p: PathBuf, cx: &mut Context<Self>) {
        if !platform::reveal_in_finder(&[p], cx) {
            self.toast("It's already gone from the disk.");
        }
    }

    // ───────────────────────── put back ─────────────────────────

    /// "X is in the Trash, empty it to get the space back" with a Put back button (or, for a practice run, what
    /// would have gone there).
    pub(super) fn trash_note(&self, bytes: u64, practice: bool, state: &PutBack, entry: Option<String>, which: PutBackFor, cx: &mut Context<Self>) -> Div {
        let p = pal();
        let size = format_bytes(bytes);
        if practice {
            return notice(p.sky, p.sky.text).child(div().font_weight(FontWeight::BLACK).child(icon_label(Ico::Practice, ts::BODY, format!("{size} would go to the Trash"))))
                .child(div().text_color(rgb(p.ink)).font_weight(FontWeight::SEMIBOLD).child("Practice run, so nothing moved. For real, it would wait in the Trash, so you could still put it back."));
        }
        let (ico, title, sub) = match state {
            PutBack::Done => (Ico::Sparkle, "Put back where it was".to_string(), "Everything that was still in the Trash is home again.".to_string()),
            _ => (Ico::Trash, format!("{size} is in the Trash"), "Empty the Trash to get the space back. Changed your mind? Put it back.".to_string()),
        };
        card().p(px(CARD_PAD)).flex().items_center().gap(px(sp::M))
            .child(icon_tile(ico, if *state == PutBack::Done { p.mint } else { p.coral }, 40.))
            .child(
                div().flex_1().min_w(px(0.)).flex().flex_col().gap(px(sp::XXS))
                    .child(div().text_size(px(ts::LEAD)).font(tabular(FontWeight::BLACK)).child(title))
                    .child(label_small(sub)),
            )
            .when_some(entry.filter(|_| *state != PutBack::Done), |d, id| {
                let busy = *state == PutBack::Busy;
                d.child(mini_button_icon(SharedString::from(format!("putback-{which:?}")), Some(Ico::PutBack), if busy { "Putting back…" } else { "Put back" }, p.grape, !busy).on_click(cx.listener(move |this, _, _, cx| this.put_back_entry(id.clone(), which, cx))))
            })
    }

    pub(super) fn put_back_entry(&mut self, entry: String, which: PutBackFor, cx: &mut Context<Self>) {
        // one put back at a time per place
        let state = match which {
            PutBackFor::Clean => Some(&mut self.done_put_back),
            PutBackFor::Uninstall => Some(&mut self.tools.app_put_back),
            PutBackFor::Large => Some(&mut self.tools.large_put_back),
            PutBackFor::History => None,
        };
        match state {
            Some(st) if *st != PutBack::No => return,
            Some(st) => *st = PutBack::Busy,
            None if self.tools.history_busy.is_some() => return,
            None => self.tools.history_busy = Some(entry.clone()),
        }
        cx.notify();
        let task = cx.background_spawn(async move { put_back(&entry, None) });
        cx.spawn(async move |this, cx| {
            let r = task.await;
            let _ = this.update(cx, |app, cx| {
                let n = r.restored.len();
                match which {
                    PutBackFor::Clean => app.done_put_back = PutBack::Done,
                    PutBackFor::Uninstall => {
                        app.tools.app_put_back = PutBack::Done;
                        app.tools.apps = None; // the app is back: list again next time
                    }
                    PutBackFor::Large => {
                        app.tools.large_put_back = PutBack::Done;
                        app.tools.large = None;
                    }
                    PutBackFor::History => {
                        app.tools.history_busy = None;
                        app.load_history(cx);
                    }
                }
                if let Some(e) = r.errors.first() {
                    app.toast(&if n > 0 { format!("Put back {n}, but: {e}") } else { e.clone() });
                    app.bubble = "Hm, not everything wanted to come back.".into();
                    app.dusty(cx, |d| d.st.set(Mood::Sad, Some(1.)));
                } else {
                    app.toast(&format!("Put back {n} item{} where {} lived.", plural_s(n), if n == 1 { "it" } else { "they" }));
                    app.bubble = "Back where it was. No harm done!".into();
                    app.dusty(cx, |d| d.st.set(Mood::Happy, Some(1.2)));
                }
                haptic(Haptic::Thump);
                cx.notify();
            });
        })
        .detach();
    }

    /// The confirm sheet's words for the tools: (emoji, title, subtitle, button, button tone).
    pub(super) fn tool_sheet_text(&self, kind: ConfirmKind) -> (Ico, String, String, &'static str, Tone) {
        let dry = self.dry_run;
        match kind {
            ConfirmKind::Uninstall => {
                let name = self.tools.app.as_ref().map(|a| a.name.clone()).unwrap_or_default();
                let n = self.tools.left_sel.len();
                let total = self.uninstall_bytes();
                let extra = if n == 0 { String::new() } else { format!(" and {n} leftover{}", plural_s(n)) };
                if dry {
                    (Ico::Practice, format!("Practice uninstalling {name}?"), format!("Nothing will move. Dusty checks {name}{extra} and writes it in History as a practice run."), "Start practice", pal().sky)
                } else {
                    {
                    let (verb, them) = if n == 0 { ("goes", "it") } else { ("go", "them") };
                    (Ico::Uninstall, format!("Uninstall {name}?"), format!("{name}{extra} {verb} to the Trash, about {}. You can put {them} back from History until you empty it.", format_bytes(total)), "Yes, uninstall", pal().coral)
                }
                }
            }
            _ => {
                let n = self.tools.large_sel.len();
                let bytes = self.large_sel_bytes();
                let files = if n == 1 { "1 file".to_string() } else { format!("{n} files") };
                if dry {
                    (Ico::Practice, format!("Practice with {files}?"), "Nothing will move. Dusty just checks they could go.".into(), "Start practice", pal().sky)
                } else {
                    (Ico::Trash, format!("Move {files} to the Trash?"), format!("About {}. The space comes back when you empty the Trash, and until then you can put them back.", format_bytes(bytes)), "Move to Trash", pal().coral)
                }
            }
        }
    }
}
