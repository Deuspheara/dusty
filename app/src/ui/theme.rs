//! Design tokens: spacing, radii, type sizes, motion, the light/dark palettes and group tones.

use super::*;

pub const FONT: &str = ".SystemUIFont";
pub(super) const TOP: f32 = 54.; // the draggable band under the transparent titlebar
pub(super) const ROW_H: f32 = 36.; // a row in the checkup's "latest finds" card

/// Spacing scale (px). Every padding, gap and margin comes from here.
#[allow(dead_code)]
pub(super) mod sp {
    pub const XXS: f32 = 2.;
    pub const XS: f32 = 4.;
    pub const S: f32 = 8.;
    pub const M: f32 = 12.;
    pub const L: f32 = 16.;
    pub const XL: f32 = 24.;
    pub const XXL: f32 = 32.;
}
/// Page side padding: content, sticky footers and the sheet all line up on it.
pub(super) const GUTTER: f32 = sp::XL;
/// Inner padding of every card.
pub(super) const CARD_PAD: f32 = sp::L;
/// Minimum size of anything clickable.
pub(super) const HIT: f32 = 28.;

/// Corner radii.
pub(super) mod radius {
    pub const XS: f32 = 6.;
    pub const S: f32 = 8.;
    pub const M: f32 = 12.;
    pub const L: f32 = 16.;
    pub const XL: f32 = 20.;
    pub const SHEET: f32 = 28.;
}

/// Type scale (px). Display text is BLACK, body is BOLD/SEMIBOLD.
pub(super) mod ts {
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
pub(super) mod motion {
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
pub(super) fn reduced() -> bool {
    REDUCED.load(std::sync::atomic::Ordering::Relaxed)
}
pub(super) static REDUCED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
pub(super) fn refresh_reduced() {
    let v = match std::env::var("DUSTY_REDUCE_MOTION").as_deref() {
        Ok("1") => true,
        Ok("0") => false,
        _ => platform::reduce_motion(),
    };
    REDUCED.store(v, std::sync::atomic::Ordering::Relaxed);
}

/// A one-shot animation of `ms`, or (almost) instant under Reduce motion.
pub(super) fn anim(ms: u64) -> Animation {
    Animation::new(Duration::from_millis(if reduced() { 1 } else { ms }))
}
/// The same, for durations measured by hand (seconds).
pub(super) fn secs(ms: u64) -> f32 {
    if reduced() {
        0.001
    } else {
        ms as f32 / 1000.
    }
}
/// Progress 0..1 of something that started at `t0` and lasts `ms` (instant under Reduce motion).
pub(super) fn since(t0: Instant, ms: u64) -> f32 {
    (t0.elapsed().as_secs_f32() / secs(ms)).clamp(0., 1.)
}

/// "just now", "5 minutes ago", "yesterday", "3 days ago", "2 weeks ago"…
pub(super) fn ago(secs: u64) -> String {
    let plural = |n: u64, unit: &str| if n == 1 { format!("a{} {unit} ago", if unit.starts_with('h') { "n" } else { "" }) } else { format!("{n} {unit}s ago") };
    match secs {
        0..=59 => "just now".into(),
        60..=3599 => plural(secs / 60, "minute"),
        3600..=86_399 => plural(secs / 3600, "hour"),
        86_400..=172_799 => "yesterday".into(),
        172_800..=1_209_599 => format!("{} days ago", secs / 86_400),
        1_209_600..=5_183_999 => format!("{} weeks ago", secs / 604_800),
        5_184_000..=62_207_999 => format!("{} months ago", secs / 2_592_000),
        _ => format!("{} years ago", secs / 31_536_000),
    }
}

/// Dusty on the home screen: full size, a bit smaller when the window is short (the tools row needs room).
pub(super) fn home_dusty_side(window_h: f32) -> f32 {
    (window_h - 590.).clamp(120., 250.)
}

pub(super) use dusty_core::now_secs;

pub(super) fn ease_out_cubic(t: f32) -> f32 {
    1. - (1. - t).powi(3)
}

/// System font with tabular digits, so counting numbers don't jiggle.
pub(super) fn tabular(weight: FontWeight) -> Font {
    Font { family: FONT.into(), features: FontFeatures(Arc::new(vec![("tnum".into(), 1)])), fallbacks: None, weight, style: FontStyle::Normal }
}

#[derive(Clone, Copy)]
pub(super) struct Tone {
    pub(super) base: u32,
    /// the solid bottom edge of a chunky button
    pub(super) edge: u32,
    /// tinted background behind a glyph or a selected row
    pub(super) soft: u32,
    pub(super) hover: u32,
    /// the tone as readable text on a card (sizes, group names)
    pub(super) text: u32,
}

/// Every color the UI paints. Two of these exist: warm paper by day, plum night (same as the website).
pub(super) struct Palette {
    pub(super) bg: u32,
    pub(super) ink: u32,
    pub(super) muted: u32,
    pub(super) line: u32,
    pub(super) card: u32,
    pub(super) card_hover: u32,
    /// "other used space" on the disk bar
    pub(super) used: u32,
    pub(super) disabled_edge: u32,
    /// dims the window behind the confirm sheet, and drop shadows
    pub(super) scrim: u32,
    pub(super) scrim_a: f32,
    /// text on the busy-items warning (on `sun.soft`)
    pub(super) warn: u32,
    pub(super) grape: Tone,
    pub(super) mint: Tone,
    pub(super) coral: Tone,
    pub(super) sun: Tone,
    pub(super) sky: Tone,
    pub(super) blue: Tone,
    pub(super) teal: Tone,
}

pub(super) const LIGHT: Palette = Palette {
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
pub(super) const DARK: Palette = Palette {
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
pub(super) static DARK_MODE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

pub(super) fn is_dark() -> bool {
    DARK_MODE.load(std::sync::atomic::Ordering::Relaxed)
}

pub(super) fn pal() -> &'static Palette {
    if DARK_MODE.load(std::sync::atomic::Ordering::Relaxed) {
        &DARK
    } else {
        &LIGHT
    }
}

/// Returns true when the palette actually changed.
pub(super) fn set_appearance(a: WindowAppearance) -> bool {
    let dark = matches!(a, WindowAppearance::Dark | WindowAppearance::VibrantDark);
    DARK_MODE.swap(dark, std::sync::atomic::Ordering::Relaxed) != dark
}

pub(super) fn tone(g: Group) -> Tone {
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
pub(super) fn glyph(g: Group) -> Ico {
    match g {
        Group::Build => Ico::Build,
        Group::Apps => Ico::CacheApp,
        Group::Cache => Ico::CacheDev,
        Group::Junk => Ico::Junk,
        Group::Docker => Ico::Docker,
        Group::Redownload => Ico::Redownload,
    }
}
/// Overshoot then settle (GPUI's `bounce` goes there and back, which is not what we want).
pub(super) fn back_out(t: f32) -> f32 {
    let (c1, c3) = (1.70158, 2.70158);
    let u = t - 1.;
    1. + c3 * u * u * u + c1 * u * u
}

pub(super) fn alpha(hex: u32, a: f32) -> Hsla {
    let mut c: Hsla = rgb(hex).into();
    c.a = a;
    c
}
