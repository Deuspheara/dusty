//! Dusty the cat: the character-sheet art rigged like a Live2D model and animated with springs.
//! Every frame is composed on the CPU (see `raster`) and handed to GPUI as an image.

#[cfg(target_os = "macos")]
pub mod gpu;
pub mod raster;
pub mod rig;

use std::cell::Cell;
use std::f32::consts::PI;
use std::rc::Rc;
use std::sync::{Arc, OnceLock};
use std::time::Instant;

use gpui::{
    canvas, div, point, px, Bounds, Context, Corners, InteractiveElement, IntoElement, MouseButton, MouseDownEvent, MouseMoveEvent,
    ParentElement, Pixels, Point, Render, RenderImage, Styled, Window,
};
use raster::*;
use rig::{p, Rig};

static RIG: OnceLock<Option<Arc<Rig>>> = OnceLock::new();
const SHEET: &[u8] = include_bytes!("../../assets/dusty-sheet.png");

/// The rig if it's ready, without waiting (main thread: never stall a frame on the 100 ms build).
pub fn try_rig() -> Option<Arc<Rig>> {
    RIG.get().cloned().flatten()
}

pub fn rig() -> Option<Arc<Rig>> {
    RIG.get_or_init(|| rig::build(SHEET).map(Arc::new).map_err(|e| eprintln!("dusty: {e}")).ok()).clone()
}

// ───────────────────────── moods ─────────────────────────

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Mood {
    Idle,
    Happy,
    Pet,
    Sad,
    Wow,
    Think,
    Squish,
    Sleepy,
    #[allow(dead_code)] // drawn and animated, not used by a flow yet
    Melt,
    Hungry,
}

#[derive(Clone, Copy, PartialEq)]
enum Eyes {
    Open,
    Teary,
    Happy,
    Sleep,
    Squeeze,
}
#[derive(Clone, Copy, PartialEq)]
enum Mouth {
    Cat,
    Open,
    O,
    Wavy,
    Flat,
}
#[derive(Clone, Copy, PartialEq)]
enum Fx {
    None,
    Marks,
    Hearts,
    Q,
    Zzz,
    Tears,
}

struct Pose {
    eyes: Eyes,
    mouth: Mouth,
    fx: Fx,
    blush: bool,
    ear: f32,
    head_y: f32,
    tilt: f32,
    eye_s: f32,
    squash: f32,
    tail: [f32; 3], // wag speed, amplitude, base angle (degrees)
    hop: u8,
    breath: f32,
    look: Option<(f32, f32)>,
    purr: bool,
}

fn pose(m: Mood) -> Pose {
    let base = Pose { eyes: Eyes::Open, mouth: Mouth::Cat, fx: Fx::None, blush: false, ear: 0., head_y: 0., tilt: 0., eye_s: 1., squash: 1., tail: [1.5, 5., 0.], hop: 0, breath: 1., look: None, purr: false };
    match m {
        Mood::Idle => base,
        Mood::Happy => Pose { eyes: Eyes::Happy, mouth: Mouth::Open, fx: Fx::Marks, ear: 10., head_y: -3., tail: [10., 6., -3.], hop: 2, ..base },
        Mood::Pet => Pose { eyes: Eyes::Happy, fx: Fx::Hearts, blush: true, ear: -14., head_y: 4., tilt: 4., squash: 0.965, tail: [3., 5., -2.], purr: true, ..base },
        Mood::Sad => Pose { eyes: Eyes::Teary, mouth: Mouth::Wavy, fx: Fx::Tears, ear: -30., head_y: 7., tilt: -2., eye_s: 1.04, squash: 0.95, tail: [1., 1.5, 4.], look: Some((0., 0.6)), ..base },
        Mood::Wow => Pose { mouth: Mouth::O, fx: Fx::Marks, ear: 16., head_y: -8., eye_s: 1.13, squash: 1.05, tail: [7., 4., -4.], ..base },
        Mood::Think => Pose { mouth: Mouth::Flat, fx: Fx::Q, ear: -6., tilt: -5., tail: [1.2, 5., 0.], look: Some((0.8, -0.9)), ..base },
        Mood::Squish => Pose { eyes: Eyes::Squeeze, mouth: Mouth::O, ear: -18., head_y: 3., tail: [8., 5., 0.], ..base },
        Mood::Sleepy => Pose { eyes: Eyes::Sleep, fx: Fx::Zzz, ear: -12., head_y: 10., tilt: 4., squash: 0.965, tail: [0.5, 1.5, 3.], breath: 2.4, look: Some((0., 0.3)), ..base },
        Mood::Melt => Pose { eyes: Eyes::Teary, mouth: Mouth::Wavy, fx: Fx::Tears, ear: -36., head_y: 16., eye_s: 1.03, squash: 0.7, tail: [0.5, 1., 5.], look: Some((0., 0.7)), ..base },
        Mood::Hungry => Pose { mouth: Mouth::O, ear: 8., head_y: -4., eye_s: 1.07, tail: [5., 5., -3.], ..base },
    }
}

// ───────────────────────── springs ─────────────────────────

#[derive(Clone, Copy, Default)]
struct Spring {
    x: f32,
    v: f32,
}
impl Spring {
    fn at(x: f32) -> Self {
        Spring { x, v: 0. }
    }
    fn step(&mut self, target: f32, k: f32, damp: f32) {
        self.v += (target - self.x) * k;
        self.v *= damp;
        self.x += self.v;
    }
}

struct Rand(u64);
impl Rand {
    fn f(&mut self) -> f32 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        (self.0 % 10_000) as f32 / 10_000.
    }
}

pub struct State {
    mood: Mood,
    mood_until: Option<f32>,
    t: f32,
    acc: f32,
    rng: Rand,
    sq: Spring,
    lean: Spring,
    ear_l: Spring,
    ear_r: Spring,
    head_y: Spring,
    tilt: Spring,
    eye_s: Spring,
    tail_lag: Spring,
    gape: Spring,
    puff: Spring,
    gape_target: f32,
    chew_until: f32,
    look: (f32, f32),
    look_override: Option<(f32, f32)>,
    sacc: (f32, f32, f32),
    pointer: Option<(f32, f32)>,
    pointer_t: f32,
    hop_y: f32,
    hop_v: f32,
    hops_left: u8,
    hop_delay: u8,
    tail_ph: f32,
    tail: [f32; 3],
    blink: f32,
    next_blink: f32,
    next_twitch: f32,
    pet: f32,
    pub dirt: f32,
    /// 0 = slim, 1 = stuffed (a full disk); eased toward `chub_target`
    chub: f32,
    pub chub_target: f32,
}

impl State {
    pub fn new() -> Self {
        let seed = Instant::now().elapsed().as_nanos() as u64 | 0x9E37_79B9;
        State {
            mood: Mood::Idle,
            mood_until: None,
            t: 0.,
            acc: 0.,
            rng: Rand(seed),
            sq: Spring::at(1.),
            lean: Spring::at(0.),
            ear_l: Spring::at(0.),
            ear_r: Spring::at(0.),
            head_y: Spring::at(0.),
            tilt: Spring::at(0.),
            eye_s: Spring::at(1.),
            tail_lag: Spring::at(0.),
            gape: Spring::at(0.),
            puff: Spring::at(0.),
            gape_target: 0.,
            chew_until: 0.,
            look: (0., 0.),
            look_override: None,
            sacc: (0., 0., 0.),
            pointer: None,
            pointer_t: -100.,
            hop_y: 0.,
            hop_v: 0.,
            hops_left: 0,
            hop_delay: 0,
            tail_ph: 0.,
            tail: [1.5, 5., 0.],
            blink: 0.,
            next_blink: 1.2,
            next_twitch: 2.,
            pet: 0.,
            dirt: 0.,
            chub: 0.,
            chub_target: 0.,
        }
    }

    /// Something visibly moving right now (worth 60 fps); otherwise Dusty is just breathing.
    pub fn lively(&self) -> bool {
        self.hop_y < 0.
            || self.hop_v != 0.
            || self.gape.x > 0.03
            || self.gape_target > 0.
            || self.t < self.chew_until
            || self.t - self.pointer_t < 1.
            || self.mood_until.is_some()
            || self.look_override.is_some()
            || self.sq.v.abs() > 0.002
            || self.ear_l.v.abs() > 0.3
            || self.ear_r.v.abs() > 0.3
            || self.blink > 0.
    }

    /// Change mood; `secs` returns to idle afterwards.
    pub fn set(&mut self, m: Mood, secs: Option<f32>) {
        let prev = self.mood;
        self.mood = m;
        self.mood_until = secs.map(|s| self.t + s);
        let ps = pose(m);
        if ps.hop > 0 {
            self.hops_left = ps.hop;
            self.sq.v -= 0.06; // anticipation crouch
            self.hop_delay = 6;
        }
        if prev != m {
            self.sq.v += if matches!(m, Mood::Sad | Mood::Melt) { -0.03 } else { 0.035 };
        }
        if m == Mood::Wow {
            self.ear_l.v += 5.;
            self.ear_r.v += 5.;
            self.head_y.v -= 2.;
        }
    }

    pub fn poke(&mut self, side: f32) {
        self.sq.v -= 0.2;
        self.lean.v += side * 0.05;
        self.ear_l.v -= 8.;
        self.ear_r.v -= 6.;
        self.head_y.v += 3.;
        if !matches!(self.mood, Mood::Sad | Mood::Melt) {
            self.set(Mood::Squish, Some(0.45));
        }
    }

    pub fn stroke(&mut self, dist: f32) {
        self.pet += dist;
        if self.pet > 600. && !matches!(self.mood, Mood::Pet | Mood::Sad | Mood::Melt) {
            self.set(Mood::Pet, Some(1.8));
        }
    }

    /// Open wide for an incoming item.
    pub fn gape(&mut self, open: bool) {
        self.gape_target = if open { 1. } else { 0. };
    }
    /// Crunch: close the mouth, puff the cheeks, squash and wiggle the ears.
    pub fn chomp(&mut self) {
        self.gape_target = 0.;
        self.gape.v -= 0.25;
        self.puff.v += 0.035;
        self.sq.v -= 0.12;
        self.head_y.v += 2.5;
        self.ear_l.v += 6.;
        self.ear_r.v += 6.;
        self.chew_until = self.t + 0.5;
    }

    pub fn look_at(&mut self, target: Option<(f32, f32)>) {
        self.look_override = target;
    }

    /// Pointer offset from Dusty's centre (logical px), or None when the pointer left.
    pub fn pointer(&mut self, offset: Option<(f32, f32)>) {
        if offset != self.pointer {
            self.pointer = offset;
            self.pointer_t = self.t;
        }
    }

    pub fn advance(&mut self, dt: f32) {
        self.acc += dt.min(0.25);
        let step = 1. / 60.;
        while self.acc >= step {
            self.acc -= step;
            self.t += step;
            self.tick();
        }
    }

    fn tick(&mut self) {
        if let Some(until) = self.mood_until {
            if self.t >= until {
                self.mood_until = None;
                self.mood = Mood::Idle;
            }
        }
        let m = pose(self.mood);
        let t = self.t;

        // where to look
        let (tx, ty) = if let Some(o) = self.look_override {
            o
        } else if let Some(l) = m.look {
            l
        } else if let (Some((dx, dy)), true) = (self.pointer, t - self.pointer_t < 3.5) {
            ((dx / 280.).clamp(-1., 1.), (dy / 280.).clamp(-1., 1.))
        } else {
            if t > self.sacc.2 {
                self.sacc = ((self.rng.f() - 0.5) * 1.4, (self.rng.f() - 0.5) * 0.7, t + 1. + self.rng.f() * 2.6);
            }
            (self.sacc.0, self.sacc.1)
        };
        self.look.0 += (tx - self.look.0) * 0.14;
        self.look.1 += (ty - self.look.1) * 0.14;

        // hops: anticipation → launch → gravity → landing squish
        if self.hop_delay > 0 {
            self.hop_delay -= 1;
            if self.hop_delay == 0 && self.hops_left > 0 {
                self.hop_v = -9.;
                self.sq.v += 0.1;
                self.hops_left -= 1;
            }
        }
        if self.hop_y < 0. || self.hop_v != 0. {
            self.hop_v += 0.65;
            self.hop_y += self.hop_v;
            if self.hop_y >= 0. {
                self.sq.v -= self.hop_v * 0.022;
                self.hop_y = 0.;
                self.hop_v = 0.;
                if self.hops_left > 0 {
                    self.hop_delay = 5;
                }
            }
        }

        let purr = if m.purr { (t * 45.).sin() * 0.004 } else { 0. };
        let chewing = t < self.chew_until;
        let chew = if chewing { (t * 26.).sin() * 0.012 } else { 0. };
        self.sq.step(m.squash + purr + chew, 0.13, 0.8);
        self.lean.step(0., 0.1, 0.82);
        self.head_y.step(m.head_y + self.hop_v * 1.1 + (1. - self.sq.x) * 30., 0.09, 0.76);
        self.tilt.step(m.tilt + self.look.0 * 3., 0.08, 0.8);
        self.eye_s.step(m.eye_s + self.gape.x * 0.06, 0.16, 0.72);
        self.tail_lag.step(self.hop_v * 0.5 + self.lean.x * 25., 0.1, 0.8);
        self.gape.step(self.gape_target, 0.2, 0.7);
        self.puff.step(0., 0.12, 0.8);

        if t > self.next_twitch {
            let dir = if self.rng.f() < 0.5 { -1. } else { 1. };
            let amt = 4. + self.rng.f() * 5.;
            if self.rng.f() < 0.5 { self.ear_l.v += dir * amt } else { self.ear_r.v += dir * amt }
            self.next_twitch = t + 1.6 + self.rng.f() * 3.6;
        }
        let curious = if t - self.pointer_t < 0.6 && m.look.is_none() { 3. } else { 0. };
        let ear_t = m.ear + curious + self.gape.x * 8.;
        self.ear_l.step(ear_t - self.look.0 * 2., 0.14, 0.74);
        self.ear_r.step(ear_t + self.look.0 * 2., 0.14, 0.74);
        self.pet *= 0.96;
        self.chub += (self.chub_target - self.chub) * 0.05;

        if t > self.next_blink {
            self.blink = 1.;
            self.next_blink = t + if self.rng.f() < 0.2 { 0.26 } else { 1.8 + self.rng.f() * 3.2 };
        }
        if self.blink > 0. {
            self.blink = (self.blink - 0.13).max(0.);
        }
        for i in 0..3 {
            self.tail[i] += (m.tail[i] - self.tail[i]) * 0.06;
        }
        self.tail_ph += self.tail[0] / 60.;
    }

    fn lid(&self) -> f32 {
        if self.blink > 0. { (1. - self.blink * 2.).abs() * 0.92 + 0.08 } else { 1. }
    }

    /// Where the mouth is, as a fraction of the rendered square (for particles and flying items).
    pub fn mouth_frac(&self) -> (f32, f32) {
        let (w, h) = (rig::CROP.2 as f32, rig::CROP.3 as f32);
        let (mx, my) = p(rig::MOUTH.0 .0, rig::MOUTH.0 .1 + 4.);
        let k = 1. / (w * 1.16);
        ((w * 0.08 + mx) * k, (h * 0.1 + my + self.head_y.x + self.hop_y) * k)
    }
}

// ───────────────────────── rendering ─────────────────────────

fn deg(d: f32) -> f32 {
    d * PI / 180.
}

/// Compose one frame into `cv` (square canvas, device pixels).
pub fn render(r: &Rig, s: &State, cv: &mut dyn Painter) {
    cv.begin();
    let m = pose(s.mood);
    let (w, h) = (r.w as f32, r.h as f32);
    let k = cv.side() as f32 / (w * 1.16);
    let view = Affine::translate(w * 0.08 * k, h * 0.1 * k).then(Affine::scale(k, k));
    let t = s.t;

    // squash & stretch with breathing, volume preserved
    let sy = s.sq.x * (1. + (t * 2.2 / m.breath).sin() * 0.01 * m.breath);
    let sx = (1. + (1. - sy) * 0.9) * (1. + s.chub * 0.13);
    let sy = sy * (1. - s.chub * 0.04);
    let g = p(rig::GROUND.0, rig::GROUND.1);

    // ground shadow
    let sh = 1. + s.hop_y / 140.;
    cv.fill_radial(view, (g.0, g.1 + 6.), (150. * sh * sx, 11.), rgba(r.line, 0.16 * sh));

    let root = view
        .then(Affine::translate(0., s.hop_y))
        .then(Affine::translate(g.0, g.1))
        .then(Affine { a: sx, b: 0., c: s.lean.x, d: sy, e: 0., f: 0. })
        .then(Affine::translate(-g.0, -g.1));

    let tail_deg = s.tail[2] + s.tail_ph.sin() * s.tail[1] + s.tail_lag.x;
    cv.draw_layer(&r.tail, root.rotate_about(p(rig::TAIL_PIVOT.0, rig::TAIL_PIVOT.1), deg(tail_deg)), 1., None);
    cv.draw_layer(&r.body, root, 1., None);
    if s.dirt > 0. {
        dirt(cv, root, s.dirt, false, t);
    }

    let puff = s.puff.x;
    let neck = p(rig::NECK.0, rig::NECK.1);
    let head = root
        .then(Affine::translate(s.look.0 * 3., s.head_y.x + (t * 1.1).sin() * 0.8))
        .rotate_about(neck, deg(s.tilt.x))
        .scale_about(neck, 1. + puff, 1. + puff * 0.4);
    cv.draw_layer(&r.head, head, 1., None);
    bent_ear(cv, &r.ear_l, head, rig::EAR_L_PIVOT, rig::EAR_L_TIP, s.ear_l.x);
    bent_ear(cv, &r.ear_r, head, rig::EAR_R_PIVOT, rig::EAR_R_TIP, -s.ear_r.x);
    if s.dirt > 0. {
        dirt(cv, head, s.dirt, true, t);
    }
    if m.blush || puff > 0.01 {
        let a = if m.blush { 0.7 } else { (puff * 18.).min(0.6) };
        for c in [p(192., 246.), p(377., 246.)] {
            cv.fill_radial(head, c, (32., 20.), rgba(0xF78CAA, a));
        }
    }
    eyes(cv, r, s, head, m.eyes);
    mouth(cv, r, s, head, m.mouth);
    fx(cv, head, m.fx, t);
    cv.draw_layer(&r.knot, root, 1., None);
}

/// Bend an ear like a warp deformer: bands chained from base to tip (forward kinematics);
/// the lower third stays rigid where it meets the head.
fn bent_ear(cv: &mut dyn Painter, ear: &Layer, head: Affine, pivot: (f32, f32), tip: (f32, f32), degs: f32) {
    if degs.abs() < 0.25 {
        cv.draw_layer(ear, head, 1., None);
        return;
    }
    let (px, py) = p(pivot.0, pivot.1);
    let (tx, ty) = p(tip.0, tip.1);
    let l = ((tx - px).powi(2) + (ty - py).powi(2)).sqrt();
    let (ux, uy) = ((tx - px) / l, (ty - py) / l);
    let n = 30;
    let reach = l * 1.25;
    let weight = |t: f32| {
        let k = ((t - 0.28) / 0.72).clamp(0., 1.);
        k * k * (3. - 2. * k)
    };
    let u = |i: usize| reach * i as f32 / n as f32;
    // the ear's extent along (u) and across (v) its axis, from its layer corners
    let (vx, vy) = (-uy, ux);
    let (lx0, ly0) = (ear.x0 as f32, ear.y0 as f32);
    let (lx1, ly1) = (lx0 + ear.w as f32, ly0 + ear.h as f32);
    let (mut umin, mut umax, mut vmin, mut vmax) = (f32::MAX, f32::MIN, f32::MAX, f32::MIN);
    for (cx, cy) in [(lx0, ly0), (lx1, ly0), (lx0, ly1), (lx1, ly1)] {
        let (du, dv) = ((cx - px) * ux + (cy - py) * uy, (cx - px) * vx + (cy - py) * vy);
        umin = umin.min(du);
        umax = umax.max(du);
        vmin = vmin.min(dv);
        vmax = vmax.max(dv);
    }
    let mut m = head;
    for i in 0..n {
        if i > 0 {
            let d = deg(degs) * (weight(i as f32 / n as f32) - weight((i - 1) as f32 / n as f32));
            m = m.rotate_about((px + ux * u(i), py + uy * u(i)), d);
        }
        let u0 = if i == 0 { umin } else { u(i) - 0.7 };
        let u1 = if i == n - 1 { umax } else { u(i + 1) + 0.7 };
        // only the strip of this band (in layer space), not the whole ear
        let at = |uu: f32, vv: f32| (px + ux * uu + vx * vv, py + uy * uu + vy * vv);
        let region = [at(u0, vmin), at(u1, vmin), at(u0, vmax), at(u1, vmax)];
        cv.draw_band(ear, m, Band { p: (px, py), u: (ux, uy), u0, u1 }, region);
    }
}

fn eyes(cv: &mut dyn Painter, r: &Rig, s: &State, head: Affine, kind: Eyes) {
    let line = rgba(r.line, 1.);
    for (i, (c, rad)) in rig::EYES.iter().enumerate() {
        let (cx, cy) = p(c.0, c.1);
        let dir = if i == 0 { 1. } else { -1. };
        match kind {
            Eyes::Open | Eyes::Teary => {
                let lid = s.lid();
                let e = head
                    .then(Affine::translate(cx + s.look.0 * 4.5, cy + s.look.1 * 3.5))
                    .then(Affine::scale(s.eye_s.x, s.eye_s.x * lid))
                    .then(Affine::translate(-cx, -cy));
                cv.draw_layer(&r.eyes[i], e, 1., None);
                if kind == Eyes::Teary {
                    let clip = Some(((cx, cy), *rad));
                    cv.fill_ellipse(e, (cx, cy + 22.), (26., 13.), rgba(0x7ECBFF, 0.55), clip);
                    cv.fill_ellipse(e, (cx - 2., cy + 16.), (15., 3.2), rgba(0xFFFFFF, 0.75), clip);
                }
                if lid < 0.35 {
                    cv.stroke(head, &quad((cx - 24., cy + 2.), (cx, cy + 13.), (cx + 24., cy + 2.)), 6.5, line);
                }
            }
            Eyes::Happy => cv.stroke(head, &quad((cx - 24., cy + 10.), (cx, cy - 22.), (cx + 24., cy + 10.)), 7.5, line),
            Eyes::Sleep => {
                cv.stroke(head, &quad((cx - 24., cy + 2.), (cx, cy + 18.), (cx + 24., cy + 2.)), 7.5, line);
                cv.stroke(head, &[(cx - dir * 24., cy + 2.), (cx - dir * 30., cy - 3.)], 6., line);
            }
            Eyes::Squeeze => cv.stroke(head, &[(cx - dir * 20., cy - 15.), (cx + dir * 14., cy), (cx - dir * 20., cy + 15.)], 7.5, line),
        }
    }
}

fn mouth(cv: &mut dyn Painter, r: &Rig, s: &State, head: Affine, kind: Mouth) {
    let (cx, cy) = p(rig::MOUTH.0 .0, rig::MOUTH.0 .1);
    let m = head.then(Affine::translate(s.look.0 * 2.5, s.look.1 * 1.5));
    let line = rgba(r.line, 1.);
    let g = s.gape.x.clamp(0., 1.2);
    // a gaping mouth (eating) overrides the mood's mouth
    if g > 0.06 || kind == Mouth::Open {
        let (hw, depth) = if g > 0.06 { (13. + 9. * g, 6. + 28. * g) } else { (18., 18.) };
        let top = cy - 5. - 3. * g;
        let mut shape = quad((cx - hw, top), (cx, top + 4.), (cx + hw, top));
        shape.extend(quad((cx + hw, top), (cx + hw * 0.9, top + depth * 1.25), (cx, top + depth * 1.2)).into_iter().skip(1));
        shape.extend(quad((cx, top + depth * 1.2), (cx - hw * 0.9, top + depth * 1.25), (cx - hw, top)).into_iter().skip(1));
        cv.fill_poly(m, &shape, rgba(0x7A3550, 1.));
        cv.fill_ellipse(m, (cx, top + depth * 0.95), (hw * 0.55, depth * 0.28), rgba(0xF4859F, 1.), Some(((cx, top + depth * 0.55), (hw * 0.95, depth * 0.62))));
        cv.stroke(m, &shape, 5.5, line);
        return;
    }
    match kind {
        Mouth::Cat => cv.draw_layer(&r.mouth, m, 1., None),
        Mouth::O => {
            cv.fill_ellipse(m, (cx, cy + 3.), (7., 9.), rgba(0x7A3550, 1.), None);
            let ring: Vec<(f32, f32)> = (0..=24).map(|i| {
                let a = i as f32 / 24. * PI * 2.;
                (cx + a.cos() * 7., cy + 3. + a.sin() * 9.)
            }).collect();
            cv.stroke(m, &ring, 4.5, line);
        }
        Mouth::Wavy => {
            let mut w = quad((cx - 17., cy + 5.), (cx - 8.5, cy - 5.), (cx, cy + 2.));
            w.extend(quad((cx, cy + 2.), (cx + 8.5, cy - 5.), (cx + 17., cy + 5.)));
            cv.stroke(m, &w, 5.5, line);
        }
        Mouth::Flat => cv.stroke(m, &quad((cx - 10., cy + 3.), (cx, cy), (cx + 12., cy + 1.)), 5.5, line),
        Mouth::Open => {}
    }
}

fn fx(cv: &mut dyn Painter, head: Affine, kind: Fx, t: f32) {
    match kind {
        Fx::None => {}
        Fx::Marks => {
            let sc = 1. + (t * 9.).sin() * 0.12;
            let (x, y) = p(452., 40.);
            let m = head.then(Affine::translate(x, y)).then(Affine::scale(sc, sc));
            let c = rgba(0xF7B928, 1.);
            cv.stroke(m, &[(0., 28.), (10., 4.)], 9., c);
            cv.stroke(m, &[(18., 38.), (40., 22.)], 9., c);
            cv.stroke(m, &[(22., 56.), (48., 55.)], 9., c);
        }
        Fx::Zzz => {
            for i in 0..2 {
                let ph = (t * 0.45 + i as f32 * 0.5) % 1.;
                let (x, y) = p(438. + ph * 30., 72. - ph * 50.);
                let sz = 18. + ph * 14.;
                let a = (ph * PI).sin();
                cv.stroke(head, &[(x, y - sz), (x + sz, y - sz), (x, y), (x + sz, y)], 5., rgba(0x8B6FE0, a));
            }
        }
        Fx::Hearts => {
            for i in 0..3 {
                let ph = (t * 0.6 + i as f32 / 3.) % 1.;
                let (x, y) = p(if i == 1 { 130. } else { 440. + i as f32 * 10. }, 110. - ph * 70.);
                let sc = 1.2 + ph * 0.6;
                let heart: Vec<(f32, f32)> = (0..40).map(|j| {
                    let a = j as f32 / 40. * PI * 2.;
                    let hx = 16. * a.sin().powi(3);
                    let hy = -(13. * a.cos() - 5. * (2. * a).cos() - 2. * (3. * a).cos() - (4. * a).cos());
                    (x + hx * 0.55 * sc, y + hy * 0.55 * sc)
                }).collect();
                cv.fill_poly(head, &heart, rgba(0xFF7A9A, (ph * PI).sin()));
            }
        }
        Fx::Q => {
            let (x, y) = p(450., 76.);
            let m = head.then(Affine::translate(x, y)).then(Affine::rotate((t * 3.).sin() * 0.2)).then(Affine::scale(1.7, 1.7));
            let c = rgba(0x7C5CFF, 1.);
            let mut q = quad((-8., -6.), (-6., -16.), (2., -16.));
            q.extend(quad((2., -16.), (10., -15.), (8., -6.)));
            q.extend(quad((8., -6.), (0., -1.), (0., 5.)));
            cv.stroke(m, &q, 4.5, c);
            cv.fill_ellipse(m, (0., 12.), (2.8, 2.8), c, None);
        }
        Fx::Tears => {
            for (i, (sx, sy)) in [(196., 244.), (374., 244.)].iter().enumerate() {
                let ph = (t * 0.9 + i as f32 * 0.5) % 1.;
                let (x, y) = p(*sx, *sy);
                let sc = 0.7 + ph * 0.5;
                let m = head.then(Affine::translate(x, y + ph * 60.)).then(Affine::scale(sc, sc));
                let mut drop = quad((0., -10.), (9., 4.), (0., 9.));
                drop.extend(quad((0., 9.), (-9., 4.), (0., -10.)));
                cv.fill_poly(m, &drop, rgba(0x7CCBFF, 1. - ph));
            }
        }
    }
}

fn dirt(cv: &mut dyn Painter, m: Affine, k: f32, head: bool, t: f32) {
    let smudge = |cv: &mut dyn Painter, (x, y): (f32, f32), r: (f32, f32), a: f32| cv.fill_radial(m, p(x, y), r, rgba(0x807062, a * k * 1.6));
    let speck = |cv: &mut dyn Painter, (x, y): (f32, f32), r: f32| cv.fill_ellipse(m, p(x, y), (r, r), rgba(0x6E6054, 0.6 * k), None);
    if head {
        smudge(cv, (395., 140.), (26., 14.), 0.28);
        smudge(cv, (150., 185.), (16., 10.), 0.25);
        speck(cv, (350., 120.), 3.);
        speck(cv, (170., 150.), 2.5);
        speck(cv, (420., 190.), 2.5);
    } else {
        smudge(cv, (185., 385.), (22., 13.), 0.28);
        smudge(cv, (380., 360.), (16., 10.), 0.25);
        speck(cv, (230., 420.), 3.);
        speck(cv, (350., 410.), 2.5);
        speck(cv, (470., 330.), 3.);
        for i in 0..5 {
            let pt = (120. + i as f32 * 95., 200. + (t * 0.8 + i as f32).sin() * 30. + (i % 2) as f32 * 120.);
            cv.fill_ellipse(m, p(pt.0, pt.1), (3., 3.), rgba(0x968A7C, 0.7 * k), None);
        }
    }
}

// ───────────────────────── GPUI view ─────────────────────────

/// `DUSTY_TRACE=1`: every 3 s, print how many frames Dusty composed and what they cost.
fn trace_frame(cost: std::time::Duration, active: bool, lively: bool) {
    use std::sync::Mutex;
    static STATS: Mutex<Option<(Instant, u32, f64, bool, bool)>> = Mutex::new(None);
    if !crate::tracing() {
        return;
    }
    let mut g = STATS.lock().unwrap();
    let e = g.get_or_insert((Instant::now(), 0, 0., active, lively));
    e.1 += 1;
    e.2 += cost.as_secs_f64() * 1000.;
    e.3 = active;
    e.4 |= lively;
    let secs = e.0.elapsed().as_secs_f64();
    if secs >= 3. {
        eprintln!("dusty: {:.0} fps, {:.2} ms/frame compose, window {}, {}", e.1 as f64 / secs, e.2 / e.1 as f64, if e.3 { "active" } else { "inactive" }, if e.4 { "lively" } else { "breathing" });
        *g = None;
    }
}

pub struct Dusty {
    pub st: State,
    size: f32,
    canvas: Canvas,
    frame: Option<Arc<RenderImage>>,
    stale: Vec<Arc<RenderImage>>,
    last: Instant,
    bounds: Rc<Cell<Option<Bounds<Pixels>>>>,
    last_mouse: Option<Point<Pixels>>,
    last_compose: Option<Instant>,
    wake_pending: bool,
    /// Metal renderer (None until first frame, or when unavailable / DUSTY_CPU=1)
    #[cfg(target_os = "macos")]
    gpu: Option<Option<gpu::Gpu>>,
}

impl Dusty {
    pub fn new(size: f32) -> Self {
        Dusty { st: State::new(), size, canvas: Canvas::new(1, 1), frame: None, stale: vec![], last: Instant::now(), bounds: Rc::new(Cell::new(None)), last_mouse: None, last_compose: None, wake_pending: false, #[cfg(target_os = "macos")] gpu: None }
    }
    pub fn set_size(&mut self, size: f32) {
        self.size = size;
    }
    pub fn size(&self) -> f32 {
        self.size
    }
    /// Centre of the rendered square in window coordinates.
    pub fn center(&self) -> Option<Point<Pixels>> {
        self.bounds.get().map(|b| b.center())
    }
    /// The mouth position in window coordinates (for flying items and crumbs).
    pub fn mouth_point(&self) -> Option<Point<Pixels>> {
        let b = self.bounds.get()?;
        let (fx, fy) = self.st.mouth_frac();
        Some(point(b.origin.x + b.size.width * fx, b.origin.y + b.size.height * fy))
    }
}

impl Render for Dusty {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let now = Instant::now();
        let dt = now.duration_since(self.last).as_secs_f32();
        self.last = now;

        // pointer → gaze
        let mouse = window.mouse_position();
        if let Some(b) = self.bounds.get() {
            if self.last_mouse != Some(mouse) {
                let c = b.center();
                self.st.pointer(Some((f32::from(mouse.x - c.x), f32::from(mouse.y - c.y))));
                self.last_mouse = Some(mouse);
            }
        }
        self.st.advance(dt);

        // frame budget: 60 fps while something moves, 30 fps while breathing, 12 fps in the background.
        // Composing costs ~1-5 ms on the CPU, so this keeps an idle window cheap.
        let active = window.is_window_active();
        let lively = self.st.lively();
        let interval = std::time::Duration::from_secs_f32(match (active, lively) {
            (true, true) => 1. / 60.,
            (true, false) | (false, true) => 1. / 30.,
            (false, false) => 1. / 12.,
        });
        let side = (self.size * window.scale_factor()).round().max(8.) as usize;
        let due = self.last_compose.is_none_or(|t| now.duration_since(t) + std::time::Duration::from_millis(2) >= interval) || self.canvas.w != side;
        if let (Some(r), true) = (try_rig(), due) {
            self.last_compose = Some(now);
            if self.canvas.w != side {
                self.canvas = Canvas::new(side, side);
            }
            let tc = Instant::now();
            #[cfg(target_os = "macos")]
            let gpu_bytes = {
                let g = self.gpu.get_or_insert_with(|| if std::env::var_os("DUSTY_CPU").is_some() { None } else { gpu::Gpu::new() });
                let st = &self.st;
                g.as_mut().and_then(|g| g.frame(side, |p| render(&r, st, p)))
            };
            #[cfg(not(target_os = "macos"))]
            let gpu_bytes: Option<Vec<u8>> = None;
            let bytes = match gpu_bytes {
                Some(b) => b,
                None => {
                    render(&r, &self.st, &mut self.canvas);
                    self.canvas.to_bgra()
                }
            };
            trace_frame(tc.elapsed(), active, lively);
            if let Some(buf) = image::RgbaImage::from_raw(side as u32, side as u32, bytes) {
                let img = Arc::new(RenderImage::new(vec![image::Frame::new(buf)]));
                if let Some(old) = self.frame.replace(img) {
                    self.stale.push(old);
                }
            }
        }
        if interval.as_secs_f32() <= 1. / 59. {
            window.request_animation_frame();
        } else if !self.wake_pending {
            // sleep until the next frame is due instead of waking on every vsync
            self.wake_pending = true;
            let wait = interval.saturating_sub(self.last_compose.map(|t| now.duration_since(t)).unwrap_or_default());
            let ex = cx.background_executor().clone();
            cx.spawn(async move |this, cx| {
                ex.timer(wait).await;
                let _ = this.update(cx, |d, cx| {
                    d.wake_pending = false;
                    cx.notify();
                });
            })
            .detach();
        }

        let frame = self.frame.clone();
        let stale = std::mem::take(&mut self.stale);
        let bounds_cell = self.bounds.clone();
        div()
            .id("dusty")
            .size(px(self.size))
            .flex_none()
            .cursor_pointer()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, e: &MouseDownEvent, _, cx| {
                    let side = this.bounds.get().map(|b| (e.position.x - b.center().x) / (b.size.width / 2.)).unwrap_or(0.);
                    this.st.poke(side);
                    cx.notify();
                }),
            )
            .on_mouse_move(cx.listener(|this, e: &MouseMoveEvent, _, _| {
                if let Some(prev) = this.last_mouse {
                    let d = e.position - prev;
                    this.st.stroke(f32::from(d.x).abs() + f32::from(d.y).abs());
                }
            }))
            .child(
                canvas(
                    move |b, _, _| bounds_cell.set(Some(b)),
                    move |b, _, window, _| {
                        for s in stale {
                            let _ = window.drop_image(s);
                        }
                        if let Some(f) = frame {
                            let _ = window.paint_image(b, Corners::default(), f, 0, false);
                        }
                    },
                )
                .size_full(),
            )
    }
}
