//! Reusable building blocks: buttons, pills, bars, cards, checkboxes, row actions, tooltips.

use super::*;

#[derive(Clone, Copy)]
pub(super) enum Btn {
    Primary(Tone),
    Ghost,
}

pub(super) const BTN_H: f32 = 54.;
pub(super) const BTN_EDGE: f32 = 4.;
/// How far a chunky button travels when pressed: it lands on its edge.
pub(super) const BTN_TRAVEL: f32 = 3.;

/// Chunky 3D button: a thick bottom edge; pressing pushes the face down onto it. Brightens on hover.
pub(super) fn button(id: impl Into<ElementId>, label: impl Into<SharedString>, kind: Btn, enabled: bool) -> Stateful<Div> {
    let (bg, edge, fg, hover) = match kind {
        Btn::Primary(t) => (t.base, t.edge, 0xFFFFFF, t.hover),
        Btn::Ghost => (pal().card, pal().line, pal().ink, pal().card_hover),
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
        // A disabled button must never act, whatever `on_click` the caller attaches afterwards:
        // a shield on top takes the mouse, so the button's own hitbox is never hovered or clicked.
        .when(!enabled, |d| d.relative().child(div().absolute().inset_0().occlude()))
}

/// "1 item", "3 items".
pub(super) fn n_items(n: usize) -> String {
    if n == 1 { "1 item".into() } else { format!("{n} items") }
}

/// The plural "s" for `n` things: `format!("{n} file{}", plural_s(n))`.
pub(super) fn plural_s(n: usize) -> &'static str {
    if n == 1 { "" } else { "s" }
}

/// Small rounded pill in the top band ("Rescan", "Done"). Stops the band from starting a window drag.
pub(super) fn pill(id: &'static str, ico: Option<Ico>, label: &'static str) -> Stateful<Div> {
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
        .when_some(ico, |d, i| d.pl(px(sp::S)).gap(px(sp::XS + 1.)).child(icon(i, 18.)))
        .child(label)
}

/// Dusty's speech bubble; `key` changes whenever the line does, so each new line pops in.
pub(super) fn bubble(text: SharedString, key: u64) -> impl IntoElement {
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
pub(super) fn disk_bar(disk: Disk, segments: &[(u64, u32)], t: f32) -> impl IntoElement {
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

/// The disk before → after a clean: used space, then the freed slice in mint, which shrinks away as `k` goes 0 → 1
/// (it leaves from its right edge, so the free space grows toward the used part).
pub(super) fn freed_bar(total: u64, before_free: u64, after_free: u64, k: f32) -> impl IntoElement {
    let total = total.max(1) as f32;
    let used = (total - after_free as f32).max(0.) / total;
    let freed = after_free.saturating_sub(before_free) as f32 / total;
    let k = ease_out_cubic(k.clamp(0., 1.));
    canvas(
        |_, _, _| {},
        move |b, _, window, _| {
            let (x0, w, h) = (b.origin.x, b.size.width, b.size.height);
            let r = h / 2.;
            window.paint_quad(fill(b, rgb(pal().line)).corner_radii(r));
            let fw = freed * (1. - k);
            let full = (used + fw) >= 0.999;
            let used_corners = Corners { top_left: r, bottom_left: r, top_right: if full && fw <= 0. { r } else { px(0.) }, bottom_right: if full && fw <= 0. { r } else { px(0.) } };
            if used > 0. {
                window.paint_quad(fill(Bounds::new(b.origin, size(w * used, h)), rgb(pal().used)).corner_radii(used_corners));
            }
            if fw * w.to_f64() as f32 > 0.5 {
                let mint = pal().mint;
                let qb = Bounds::new(point(x0 + w * used, b.origin.y), size(w * fw, h));
                let mut c: Hsla = rgb(mint.base).into();
                c.a = 1. - 0.5 * k;
                let end = if full { r } else { px(0.) };
                window.paint_quad(fill(qb, c).corner_radii(Corners { top_left: px(0.), bottom_left: px(0.), top_right: end, bottom_right: end }));
                if w * fw > px(4.) {
                    window.paint_quad(fill(Bounds::new(qb.origin, size(px(1.5), h)), rgb(pal().card)));
                }
            }
        },
    )
    .h(px(20.))
}

/// Rounded progress bar painted directly (track, fill, soft shine), so the fill always sits inside the track.
pub(super) fn progress_bar(t: f32, tone: Tone) -> impl IntoElement {
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
pub(super) fn busy_bar() -> impl IntoElement {
    let track = div().w_full().h(px(12.)).rounded_full().bg(rgb(pal().line)).overflow_hidden().relative();
    if reduced() {
        return track.child(div().h_full().w(relative(0.5)).rounded_full().bg(rgb(pal().grape.base)).opacity(0.7));
    }
    track.child(div().absolute().top_0().h_full().w(relative(0.3)).rounded_full().bg(linear_gradient(90., linear_color_stop(hsla_of(pal().grape.base, 0.), 0.), linear_color_stop(hsla_of(pal().grape.base, 1.), 0.5))).with_animation(
        "scanbar",
        Animation::new(Duration::from_millis(1300)).repeat().with_easing(ease_in_out),
        |el, t| el.left(relative(-0.3 + 1.3 * t)),
    ))
}

/// One line, cut with an ellipsis. GPUI's `truncate()` caches the first (unconstrained) measurement of a
/// `nowrap` text, so long labels got clipped mid-letter; a one-line clamp re-measures once the width is known.
pub(super) trait OneLine: Styled + Sized {
    fn one_line(self) -> Self {
        self.overflow_hidden().text_ellipsis().line_clamp(1)
    }
}
impl<T: Styled> OneLine for T {}

/// A one-line label that takes the rest of a flex row. The text sits in a block wrapper so it is measured
/// against the wrapper's resolved width (a flex item measured directly can end up cut to a few letters).
pub(super) fn fill_line(el: Div) -> Div {
    div().flex_1().min_w(px(0.)).overflow_hidden().child(el.one_line())
}

pub(super) fn card() -> Div {
    div().rounded(px(radius::XL)).bg(rgb(pal().card)).border_2().border_color(rgb(pal().line)).border_b_4()
}

pub(super) fn label_small(s: impl Into<SharedString>) -> Div {
    div().text_size(px(ts::CAPTION)).line_height(px(ts::CAPTION * 1.4)).font_weight(FontWeight::BOLD).text_color(rgb(pal().muted)).child(s.into())
}

/// Secondary sentence under a title.
pub(super) fn subtitle(s: impl Into<SharedString>) -> Div {
    div().text_size(px(ts::BODY)).line_height(px(ts::BODY * 1.4)).font_weight(FontWeight::SEMIBOLD).text_color(rgb(pal().muted)).child(s.into())
}

/// A size or count: black weight with tabular digits, so it doesn't jiggle while it counts.
pub(super) fn number(text: impl Into<SharedString>, size: f32, color: u32) -> Div {
    // big numbers get a tight line so they sit close to their caption
    let lh = if size >= ts::DISPLAY { 1.1 } else { 1.3 };
    div().flex_none().text_size(px(size)).line_height(px((size * lh).round())).font(tabular(FontWeight::BLACK)).text_color(rgb(color)).child(text.into())
}

/// Group icon on its soft tint. Three sizes keep icons consistent across screens.
pub(super) fn glyph_tile(g: Group, side: f32) -> Div {
    icon_tile(glyph(g), tone(g), side)
}

/// An icon on a soft tint, filling about 64% of it. Sides: 30 / 32 (rows), 40 (cards).
pub(super) fn icon_tile(i: Ico, t: Tone, side: f32) -> Div {
    let r = if side >= 40. { radius::M + 2. } else if side >= 32. { radius::M - 2. } else { radius::S };
    div().size(px(side)).flex_none().rounded(px(r)).bg(rgb(t.soft)).flex().items_center().justify_center().child(icon(i, (side * 0.64).round()))
}

/// Inline icon size next to text of size `text`: a touch taller than the cap height, centered on the line.
pub(super) fn inline_icon(i: Ico, text: f32) -> Img {
    icon(i, (text * 1.25).round().clamp(16., 20.))
}

/// An icon then a label on one line, centered on each other.
pub(super) fn icon_label(i: Ico, text: f32, label: impl Into<SharedString>) -> Div {
    div().flex().items_center().gap(px(sp::XS + 2.)).child(inline_icon(i, text)).child(div().min_w(px(0.)).child(label.into()))
}

/// The big icon at the top of a sheet: pops in (scales up with an overshoot) each time `key` changes.
pub(super) fn sheet_icon(i: Ico, key: ElementId) -> impl IntoElement {
    const S: f32 = 52.;
    div().size(px(S)).flex().items_center().justify_center().child(icon_raster(i, S, S).with_animation(key, anim(motion::SHEET + 120), |el, k| {
        let k = ((k - 0.25) / 0.75).clamp(0., 1.);
        el.size(px(S * (0.4 + 0.6 * back_out(k)))).opacity(k)
    }))
}

/// A big icon heading an empty state or a result.
pub(super) fn hero_icon(i: Ico) -> Img {
    icon(i, 56.)
}

const FADE: f32 = 22.;

/// A soft fade from transparent to `bg` (top-to-bottom when `down`), like iOS under bars.
fn fade(bg: u32, down: bool) -> Div {
    let (a, b) = (hsla_of(bg, 0.), hsla_of(bg, 1.));
    let (from, to) = if down { (a, b) } else { (b, a) };
    div().absolute().left_0().right_0().h(px(FADE)).bg(linear_gradient(180., linear_color_stop(from, 0.), linear_color_stop(to, 1.)))
}

fn hsla_of(c: u32, a: f32) -> Hsla {
    let mut h: Hsla = rgb(c).into();
    h.a = a;
    h
}

/// A sticky footer under a scrolling list: the list fades out into it instead of ending on a line.
pub(super) fn footer(bg: u32) -> Div {
    div().relative().flex_none().px(px(GUTTER)).pb(px(sp::L)).pt(px(sp::XS)).flex().flex_col().gap(px(sp::XS)).bg(rgb(bg)).child(fade(bg, true).top(px(-FADE)))
}

/// A scrolling region that fades in under whatever sits above it, instead of being cut off hard.
pub(super) fn fade_top(list: impl IntoElement, bg: u32) -> Div {
    div().relative().flex_1().min_h(px(0.)).flex().flex_col().child(list).child(fade(bg, false).top_0())
}

/// Checkbox that pops its tick in. `round` for the hint list.
pub(super) fn checkbox(key: SharedString, on: bool, t: Tone, side: f32, round: bool) -> Div {
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
        .text_color(rgb(if matches!(t.base, 0xFFC83D | 0x3DB8FF | 0x22C993 | 0x2EC4B6) { 0x2B2340 } else { 0xFFFFFF }))
        .font_weight(FontWeight::BLACK)
        .when(on, |d| {
            let full = side * 0.58;
            d.child(div().child("✓").with_animation(ElementId::Name(key), anim(motion::QUICK + 80), move |el, k| el.opacity(k).text_size(px(full * (0.4 + 0.6 * back_out(k))))))
        })
}

/// Pop an element in after `delay_ms`: fade and rise with a little overshoot.
/// `pop_in` only when `on`: a screen you come back to keeps its content still instead of replaying its entrance.
pub(super) fn pop_in_if<E: IntoElement + 'static>(on: bool, el: E, id: ElementId, delay_ms: u64) -> AnyElement {
    if on {
        pop_in(el, id, delay_ms).into_any_element()
    } else {
        div().child(el).into_any_element()
    }
}

pub(super) fn pop_in<E: IntoElement + 'static>(el: E, id: ElementId, delay_ms: u64) -> impl IntoElement {
    let total = motion::ENTER + delay_ms;
    let start = delay_ms as f32 / total as f32;
    div().child(el).with_animation(id, anim(total), move |d, t| {
        let k = ((t - start) / (1. - start)).clamp(0., 1.);
        d.opacity(k).mt(px(motion::RISE * (1. - back_out(k))))
    })
}

/// A tooltip: the toast's dark pill, smaller.
pub(super) struct Tip(pub(super) SharedString);

impl Render for Tip {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .font_family(FONT)
            .px(px(sp::S + sp::XXS))
            .py(px(sp::XS + 1.))
            .rounded(px(radius::S))
            .bg(rgb(pal().ink))
            .text_color(rgb(pal().bg))
            .text_size(px(ts::CAPTION))
            .font_weight(FontWeight::BOLD)
            .shadow(vec![BoxShadow { color: alpha(pal().scrim, 0.2), offset: point(px(0.), px(3.)), blur_radius: px(10.), spread_radius: px(0.) }])
            .child(self.0.clone())
    }
}

/// A small square action that shows on a row while it's hovered ("Show in Finder", "Never clean this").
/// Stops the press from reaching the row, so it never toggles or presses it.
pub(super) fn row_action(id: String, ico: Ico, tip: &'static str, t: Tone) -> Stateful<Div> {
    row_action_base(id, tip, t).child(icon(ico, 18.).with_animation(ElementId::Name("ra-pop".into()), anim(motion::QUICK + 60), |el, k| el.size(px(18. * (0.6 + 0.4 * back_out(k))))))
}

/// `row_action` with a plain glyph ("✕") instead of an icon.
pub(super) fn row_action_text(id: String, glyph: &'static str, tip: &'static str, t: Tone) -> Stateful<Div> {
    row_action_base(id, tip, t).text_size(px(ts::SMALL)).font_weight(FontWeight::BLACK).text_color(rgb(pal().muted)).child(glyph)
}

pub(super) fn row_action_base(id: String, tip: &'static str, t: Tone) -> Stateful<Div> {
    div()
        .id(ElementId::Name(id.into()))
        .size(px(HIT))
        .flex_none()
        .rounded(px(radius::S))
        .border_1()
        .border_color(rgb(pal().line))
        .bg(rgb(pal().card))
        .flex()
        .items_center()
        .justify_center()
        .cursor_pointer()
        .hover(move |s| s.bg(rgb(t.soft)).border_color(rgb(t.base)))
        .active(|s| s.opacity(0.7).pt(px(2.)))
        .on_mouse_down(MouseButton::Left, |_, _, cx| {
            haptic(Haptic::Tick);
            cx.stop_propagation()
        })
        .tooltip(move |_, cx| cx.new(|_| Tip(tip.into())).into())
}

/// Room the focus ring needs outside a row.
pub(super) const RING_PAD: f32 = 5.;

/// The focus ring for keyboard navigation: a gap in the page color, then a grape ring.
pub(super) fn focus_ring(gap_color: u32) -> Vec<BoxShadow> {
    let g = pal().grape;
    vec![
        BoxShadow { color: alpha(g.base, 0.9), offset: point(px(0.), px(0.)), blur_radius: px(0.), spread_radius: px(4.5) },
        BoxShadow { color: rgb(gap_color).into(), offset: point(px(0.), px(0.)), blur_radius: px(0.), spread_radius: px(2.) },
    ]
}

/// `v` counting up from zero over `ms`, starting at `t0`.
pub(super) fn count_up(v: u64, t0: Instant, ms: u64) -> u64 {
    (v as f64 * ease_out_cubic(since(t0, ms)) as f64).round() as u64
}

/// An on/off switch (Practice run, Settings toggles); `key` keeps its knob animation apart from others.
pub(super) fn switch(key: &str, on: bool, t: Tone) -> Div {
    div().w(px(44.)).h(px(26.)).flex_none().rounded_full().bg(rgb(if on { t.base } else { pal().used })).relative().child(
        div()
            .absolute()
            .top(px(3.))
            .size(px(20.))
            .rounded_full()
            .bg(rgb(0xFFFFFF))
            .shadow(vec![BoxShadow { color: alpha(0x000000, 0.12), offset: point(px(0.), px(2.)), blur_radius: px(0.), spread_radius: px(0.) }])
            .with_animation(ElementId::NamedInteger(format!("sw-{key}").into(), on as u64), anim(motion::BASE + 40), move |el, k| {
                let k = back_out(k);
                let (a, b) = if on { (3., 21.) } else { (21., 3.) };
                el.left(px(a + (b - a) * k))
            }),
    )
}
