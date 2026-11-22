//! The checkup flow's screens: home, results, choose items, confirm sheet, cleaning and done.

use super::*;

/// Row height of an item inside an open group on Results.
pub(super) const ITEM_H: f32 = 40.;
/// About how tall a Choose items row is (two lines), for closing its slot once it has faded out.
pub(super) const PICK_ROW_H: f32 = 66.;

/// Wide windows: a fixed left pane (summary and actions) beside a right pane that scrolls.
pub(super) fn split(top: impl IntoElement, bottom: impl IntoElement, right: impl IntoElement) -> Div {
    div()
        .flex_1()
        .min_h(px(0.))
        .flex()
        .gap(px(sp::XL))
        .px(px(GUTTER))
        .pt(px(sp::S))
        .child(div().w(px(SIDE_W)).flex_none().flex().flex_col().gap(px(sp::XL)).pb(px(sp::XL)).child(top).child(bottom))
        .child(div().flex_1().min_w(px(0.)).flex().flex_col().child(right))
}

impl DustyApp {
    /// True when the window is wide enough for two panes.
    pub(super) fn is_wide(&self) -> bool {
        self.view.0 >= WIDE_W
    }

    pub(super) fn set_hover(&mut self, id: &str, on: bool, cx: &mut Context<Self>) {
        let before = self.hovered_row.clone();
        if on {
            self.hovered_row = Some(id.to_string());
        } else if self.hovered_row.as_deref() == Some(id) {
            self.hovered_row = None;
        }
        if before != self.hovered_row {
            cx.notify();
        }
    }

    /// "Show in Finder" (when something is on disk) and "Never clean this", popping in on hover.
    pub(super) fn row_actions(&self, t: &Target, cx: &Context<Self>) -> impl IntoElement {
        let (a, b) = (t.id.clone(), t.id.clone());
        let can_reveal = t.paths.iter().any(|p| p.exists());
        div()
            .flex_none()
            .flex()
            .items_center()
            .gap(px(sp::XS))
            .when(can_reveal, |d| d.child(row_action(format!("reveal-{a}"), Ico::Reveal, "Show in Finder", pal().grape).on_click(cx.listener(move |this, _, _, cx| this.reveal(&a, cx)))))
            .child(row_action(format!("never-{b}"), Ico::Never, "Never clean this", pal().coral).on_click(cx.listener(move |this, _, _, cx| this.never_clean(&b, cx))))
            .with_animation(ElementId::Name(format!("acts-{}", t.id).into()), anim(motion::QUICK), |el, k| el.opacity(k).ml(px(sp::XS * (1. - back_out(k)))))
    }

    /// The window's top: content runs edge to edge under the transparent titlebar; this band drags the window.
    /// Dusty where a screen wants him; in a wide window with a sidebar he lives there instead (one place, one size).
    pub(super) fn cat_here(&self) -> bool {
        !(self.has_sidebar(self.screen) && self.screen != Screen::Home)
    }

    pub(super) fn cat(&self) -> Div {
        if self.has_sidebar(self.screen) && self.screen != Screen::Home { div() } else { div().child(self.dusty.clone()) }
    }

    /// Wide windows: a sidebar for the top-level places (Apple split view), so the tools stop being stranded columns.
    pub(super) fn has_sidebar(&self, scr: Screen) -> bool {
        self.is_wide() && self.intro.is_none() && !matches!(scr, Screen::Scanning | Screen::Cleaning)
    }

    pub(super) fn sidebar(&self, scr: Screen, cx: &mut Context<Self>) -> impl IntoElement {
        let p = pal();
        let items: [(&'static str, Screen, Ico, Tone, &'static str); 6] = [
            ("nav-home", Screen::Home, Ico::Broom, p.mint, "Checkup"),
            ("nav-apps", Screen::Apps, Ico::Uninstall, p.sky, "Uninstall apps"),
            ("nav-large", Screen::Large, Ico::BigFiles, p.sun, "Big & old files"),
            ("nav-usage", Screen::Usage, Ico::Usage, p.grape, "Space map"),
            ("nav-history", Screen::History, Ico::History, p.teal, "Clean history"),
            ("nav-settings", Screen::Settings, Ico::Settings, p.blue, "Settings"),
        ];
        let at = |s: Screen| match scr {
            Screen::Results | Screen::Pick | Screen::Done => s == Screen::Home,
            Screen::AppDetail => s == Screen::Apps,
            x => x == s,
        };
        div().w(px(SIDE_NAV)).h_full().flex_none().flex().flex_col().gap(px(sp::XS)).pl(px(sp::L)).pr(px(sp::S)).pt(px(sp::S)).children(items.into_iter().map(|(id, target, ico, t, label)| {
            let on = at(target);
            div()
                .id(id)
                .h(px(HIT + 8.))
                .px(px(sp::S))
                .flex()
                .items_center()
                .gap(px(sp::S))
                .rounded(px(radius::M))
                .bg(rgb(if on { p.grape.soft } else { p.bg }))
                .text_size(px(ts::SMALL))
                .font_weight(FontWeight::BLACK)
                .text_color(rgb(if on { p.grape.text } else { p.ink }))
                .cursor_pointer()
                .when(!on, |d| d.hover(|s| s.bg(rgb(p.card))))
                .active(|s| s.opacity(0.7))
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .on_click(cx.listener(move |this, _, _, cx| {
                    if target == Screen::Home {
                        if this.screen != Screen::Home {
                            this.enter_home(cx);
                        }
                    } else if this.screen != target {
                        this.open_tool(target, cx);
                    }
                }))
                .child(icon_tile(ico, t, 24.))
                .child(div().flex_1().min_w(px(0.)).one_line().child(label))
        }))
        .child(div().flex_1())
        .when(scr != Screen::Home, |d| d.child(div().w_full().pb(px(sp::L)).flex().flex_col().items_center().child(div().w(px(SIDE_CAT)).h(px(SIDE_CAT)).flex_none().child(self.dusty.clone()))))
    }

    pub(super) fn top_band(&self, scr: Screen, cx: &mut Context<Self>) -> impl IntoElement {
        let title = match scr {
            Screen::Home => "",
            Screen::Scanning => "Checkup",
            // these two carry their own big heading
            Screen::Results | Screen::Pick | Screen::Done => "",
            Screen::Cleaning => "Cleaning",
            // tool pages carry their own heading
            Screen::Apps | Screen::AppDetail | Screen::Large | Screen::Usage | Screen::History | Screen::Settings => "",
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
            // the pills sit at the content column's right edge (the window's edge when it is narrow)
            .child(div().absolute().inset_0().flex().justify_center().when(self.has_sidebar(scr), |d| d.pl(px(SIDE_NAV)).justify_start()).child(
                div().relative().w_full().max_w(px(self.col_max(scr))).h_full().when(self.has_sidebar(scr), |d| d.mr(px(0.)))
            .when(scr == Screen::Pick, |d| {
                d.child(pill("back", None, "Done").on_click(cx.listener(|this, _, _, cx| this.back_to_results(cx))))
            })
            .when(scr == Screen::Results, |d| d.child(pill("rescan", None, "Rescan").on_click(cx.listener(|this, _, _, cx| this.start_scan(cx)))))
            .when(scr == Screen::Home && self.intro.is_none() && !self.has_sidebar(scr), |d| d.child(pill("settings", Some(Ico::Settings), "Settings").on_click(cx.listener(|this, _, _, cx| this.open_tool(Screen::Settings, cx)))))
            .when(scr.is_tool() && (scr == Screen::AppDetail || !self.has_sidebar(scr)), |d| {
                let (ico, label) = if scr == Screen::AppDetail { (Some(Ico::Back), "Apps") } else { (None, "Done") };
                d.child(pill("tool-back", ico, label).on_click(cx.listener(|this, _, _, cx| this.tool_back(cx))))
            })
            ))
            .when_some(self.update_pill_label(), |d, label| {
                d.child(
                    update_pill(label)
                        .on_click(cx.listener(|_, _, _, cx| {
                            updater::act(UpdAction::Show);
                            cx.notify();
                        }))
                        .with_animation("update-pill-in", anim(motion::ENTER), |el, t| el.opacity(t.min(1.)).mt(px(-sp::S * (1. - back_out(t))))),
                )
            })
    }

    pub(super) fn page(&self) -> Div {
        div().flex_1().min_h(px(0.)).flex().flex_col().px(px(GUTTER))
    }

    pub(super) fn home(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let d = self.disk;
        let bar_t = self.enter_k(0.3).min(since(self.st(), motion::FILL));
        let compact = self.view.1 < COMPACT_H;
        if self.is_wide() {
            return self.home_wide(cx).into_any_element();
        }
        self.page()
            .items_center()
            .child(div().h(px(if compact { sp::XS } else { sp::L })))
            .child(self.enter(bubble(self.bubble.clone(), self.home_bubble_key()), 0.12))
            .child(div().mt(px(sp::S)).child(self.cat()))
            .child(self.enter(div().text_size(px(ts::DISPLAY)).font_weight(FontWeight::BLACK).child("Disk checkup"), 0.16))
            // short windows drop the subtitle so the tools row still fits
            .when(self.view.1 >= 720., |d| d.child(self.enter(subtitle("Dusty sniffs out build leftovers and caches you can safely clear.").text_size(px(ts::LEAD)).line_height(px(ts::LEAD * 1.4)).mt(px(sp::XS)).text_center(), 0.2)))
            .child(self.enter(
                card().w_full().mt(px(if compact { sp::M } else { sp::XL })).p(px(CARD_PAD)).flex().flex_col().gap(px(sp::M))
                    .child(
                        div().flex().justify_between().items_end()
                            .child(div().flex().items_end().gap(px(sp::S - 2.)).child(number(format_bytes(d.free), ts::H2, pal().ink)).child(label_small("free").mb(px(sp::XS))))
                            .child(label_small(format!("of {}", format_bytes(d.total))).mb(px(sp::XS))),
                    )
                    .child(disk_bar(d, &[], bar_t)),
                0.24,
            ))
            .when_some(self.last_scan.clone(), |el, last| {
                let ago = ago(last.age_secs());
                let found = if last.total_bytes > 0 { format!("~{} found", format_bytes(last.total_bytes)) } else { "spotless".into() };
                el.child(self.enter(label_small(format!("Last checked {ago} · {found}")).font(tabular(FontWeight::BOLD)).mt(px(sp::S)), 0.27))
            })
            .child(div().flex_1().min_h(px(sp::M)))
            .child(self.enter(button("start", "Start checkup", Btn::Primary(pal().grape), true).on_click(cx.listener(|this, _, _, cx| this.start_scan(cx))), 0.3))
            .when(!compact, |d| d.child(self.enter(label_small("Nothing is deleted until you say so."), 0.34).mt(px(sp::XS))))
            .child(self.enter(self.tool_row(cx), 0.38).mt(px(sp::M)).mb(px(sp::XL)))
            .into_any_element()
    }

    /// Home in a wide window: Dusty on the left, the checkup and the tools on the right.
    fn home_wide(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let d = self.disk;
        let bar_t = self.enter_k(0.3).min(since(self.st(), motion::FILL));
        let left = div().flex_1().min_w(px(0.)).flex().flex_col().items_center().justify_center()
            .child(self.enter(bubble(self.bubble.clone(), self.home_bubble_key()), 0.12))
            .child(div().mt(px(-sp::XL)).child(self.cat()));
        let hero = false;
        let right = div().w(px(if hero { 600. } else { SIDE_W + 90. })).flex_none().flex().flex_col().justify_center().gap(px(sp::M))
            .child(self.enter(div().w_full().text_size(px(ts::HERO * 0.7)).font_weight(FontWeight::BLACK).child("Disk checkup"), 0.16))
            .child(self.enter(subtitle("Dusty sniffs out build leftovers and caches you can safely clear.").w_full(), 0.2))
            .child(self.enter(
                card().w_full().p(px(CARD_PAD)).flex().flex_col().gap(px(sp::M))
                    .child(
                        div().flex().justify_between().items_end()
                            .child(div().flex().items_end().gap(px(sp::S - 2.)).child(number(format_bytes(d.free), if hero { ts::HERO } else { ts::H2 }, pal().ink)).child(label_small("free").mb(px(sp::XS))))
                            .child(label_small(format!("of {}", format_bytes(d.total))).mb(px(sp::XS))),
                    )
                    .child(disk_bar(d, &[], bar_t))
                    .when_some(self.last_scan.clone(), |el, last| {
                        let found = if last.total_bytes > 0 { format!("~{} found", format_bytes(last.total_bytes)) } else { "spotless".into() };
                        el.child(label_small(format!("Last checked {} · {found}", ago(last.age_secs()))).font(tabular(FontWeight::BOLD)))
                    }),
                0.24,
            ))
            .child(self.enter(button("start", "Start checkup", Btn::Primary(pal().grape), true).on_click(cx.listener(|this, _, _, cx| this.start_scan(cx))), 0.3))
            .child(self.enter(label_small("Nothing is deleted until you say so."), 0.34));
        div().flex_1().min_h(px(0.)).flex().items_center().justify_center().gap(px(sp::XXL)).px(px(GUTTER)).pb(px(sp::XL)).child(left).child(right)
    }


    pub(super) fn scanning(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let (shown, chips) = self.scan.as_ref().map(|s| (s.shown, s.chips.clone())).unwrap_or_default();
        let newest = chips.last().map(|c| c.0.clone()).unwrap_or_default();
        // five rows in a four-row window: the list slides down one slot as a find arrives, the oldest slides out
        let slots = (((self.view.1 - 650.) / ROW_H).floor() as usize).clamp(4, 9);
        let rows: Vec<_> = chips.into_iter().rev().take(slots + 1).collect();
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
            .child(div().mt(px(sp::XS)).child(self.cat()))
            .child(number(format_bytes(shown as u64), ts::HERO, pal().grape.base))
            .child(label_small("of leftovers found so far"))
            .child(div().w_full().mt(px(sp::L)).child(busy_bar()))
            // latest finds: fixed slots in a fixed-height window, so nothing below ever moves
            .child(
                card().w_full().mt(px(sp::L)).px(px(CARD_PAD)).py(px(sp::M)).flex().flex_col()
                    .child(label_small("Latest finds").mb(px(sp::XS)))
                    .child(div().h(px(slots as f32 * ROW_H)).overflow_hidden().child(finds)),
            )
            .child(div().flex_1())
            .child(div().w_full().flex().justify_center().child(div().w(px(220.)).child(button("cancel", "Stop", Btn::Ghost, true).on_click(cx.listener(|this, _, _, cx| this.cancel_scan(cx))))))
            .child(div().h(px(sp::L)))
    }

    /// Open-ness (0..1) of row `i` in a group on Results: rows unfold in a quick stagger and fold together.
    pub(super) fn group_k(&self, g: Group, i: usize) -> f32 {
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

    pub(super) fn results(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let total: u64 = self.live().map(|t| t.bytes).sum();
        let safe = self.safe_bytes();
        let max = self.live().map(|t| t.bytes).max().unwrap_or(1).max(1);
        let segs: Vec<(u64, u32)> = Group::ORDER.iter().map(|g| (self.live().filter(|t| t.group == *g).map(|t| t.bytes).sum(), tone(*g).base)).collect();
        // a group whose last item is leaving stays to fade out with it
        let groups: Vec<Group> = Group::ORDER.iter().copied().filter(|g| self.targets.iter().any(|t| t.group == *g)).collect();
        let legend: Vec<Group> = groups.iter().copied().filter(|g| self.live().any(|t| t.group == *g)).collect();
        let empty = self.targets.is_empty();
        // the cards play their entrance once per scan, not each time you come back to Results
        let fresh = self.screen_t.elapsed() < Duration::from_secs(3);

        let wide = self.is_wide();
        let head = div().flex().items_center().gap(px(sp::M)).when(self.cat_here() && wide, |d| d.flex_col().text_center()).when(self.cat_here(), |d| d.child(div().flex_none().child(self.dusty.clone()))).child(
            div().flex().flex_col().gap(px(sp::XS)).flex_1().min_w(px(0.)).when(wide && self.cat_here(), |d| d.items_center())
                .child(div().text_size(px(if wide { ts::DISPLAY } else { ts::H1 })).line_height(px(ts::H1 * 1.15)).font(tabular(FontWeight::BLACK)).child(format!("Found {}", format_bytes(count_up(total, self.st(), motion::FILL)))))
                .child(subtitle(if safe > 0 { format!("{} is safe to toss right now.", format_bytes(safe)) } else { "Spotless already.".to_string() })),
        );
        let disk_card = card().w_full().p(px(if wide { CARD_PAD } else { sp::M + 2. })).mt(px(if wide { sp::M } else { sp::S })).flex().flex_col().gap(px(sp::S + sp::XXS))
            .child(div().flex().justify_between().child(label_small(format!("{} free", format_bytes(self.disk.free)))).child(label_small(format!("of {}", format_bytes(self.disk.total)))))
            .child(disk_bar(self.disk, &segs, since(self.st(), motion::FILL + 400)))
            .when(wide && !legend.is_empty(), |d| {
                d.child(div().flex().flex_wrap().gap_x(px(sp::M)).gap_y(px(sp::XS)).children(legend.iter().map(|g| {
                    div().flex().items_center().gap(px(sp::XS + 1.)).child(div().size(px(10.)).rounded(px(3.)).bg(rgb(tone(*g).base))).child(label_small(g.name()))
                })))
            });
        let mut side = None;
        let mut list = div()
            .id("results-list")
            .flex_1()
            .min_h(px(0.))
            .overflow_y_scroll()
            .px(px(GUTTER));
        if wide {
            side = Some((head, disk_card));
        } else {
            list = list.child(head).child(disk_card);
        }
        let list = list
            .when(empty, |d| {
                d.child(pop_in_if(
                    fresh,
                    card().mt(px(sp::M)).px(px(sp::XL)).py(px(sp::XL)).flex().flex_col().items_center().gap(px(sp::XS))
                        .child(hero_icon(Ico::Sparkle))
                        .child(div().text_size(px(ts::TITLE)).font_weight(FontWeight::BLACK).child("Nothing to clean"))
                        .child(subtitle("No build leftovers or caches worth clearing right now. Check again after a few builds.").text_center()),
                    ElementId::NamedInteger("empty".into(), self.results_gen),
                    motion::STAGGER,
                ))
            })
            .children(groups.iter().enumerate().map(|(gi, g)| {
                let g = *g;
                let t = tone(g);
                let items: Vec<Target> = self.targets.iter().filter(|x| x.group == g).cloned().collect();
                let live_n = items.iter().filter(|x| !self.removing.contains_key(&x.id)).count();
                let sum: u64 = items.iter().filter(|x| !self.removing.contains_key(&x.id)).map(|x| x.bytes).sum();
                let open = self.expanded.contains(&g);
                let gen = self.group_anim.get(&g).map(|(_, n)| *n).unwrap_or(0);
                let shown = open || self.group_k(g, 0) > 0.;
                let rows: Vec<(Target, f32)> = if shown { items.iter().enumerate().map(|(i, x)| (x.clone(), self.group_k(g, i))).collect() } else { vec![] };
                let tail = rows.first().map(|r| r.1).unwrap_or(0.);
                // the whole card leaves when its last item does
                let (g_fade, g_left) = if live_n == 0 { items.iter().map(|x| self.removal(&x.id)).fold((1f32, 0f32), |a, b| (a.0.min(b.0), a.1.max(b.1))) } else { (0., 1.) };
                let card_el = pop_in_if(
                    fresh,
                    card().mt(px(if wide { sp::M } else { sp::S })).overflow_hidden().child(
                        div()
                            .id(ElementId::Name(format!("group-{gi}").into()))
                            .flex()
                            .items_center()
                            .gap(px(sp::M))
                            .p(px(if wide { CARD_PAD } else { sp::M }))
                            .when(wide, |d| d.min_h(px(((self.view.1 - TOP - 120.) / groups.len().max(1) as f32 - sp::M).clamp(88., 140.))))
                            // hover tint follows the card's inner corners (overflow doesn't clip to the radius)
                            .rounded_t(px(radius::XL - 2.))
                            .when(!shown, |d| d.rounded_b(px(radius::XL - 4.)))
                            .cursor_pointer()
                            .hover(|s| s.bg(rgb(pal().card_hover)))
                            .on_mouse_down(MouseButton::Left, |_, _, _| haptic(Haptic::Tick))
                            .on_click(cx.listener(move |this, _, _, cx| this.toggle_group(g, cx)))
                            .child(glyph_tile(g, if wide { 44. } else { 40. }))
                            .child(
                                // name and size share the first line; the caption gets the full width under them
                                div().flex_1().min_w(px(0.)).flex().flex_col().gap(px(sp::XXS))
                                    .child(
                                        div().flex().items_center().gap(px(sp::S))
                                            .child(div().flex_1().min_w(px(0.)).text_size(px(ts::TITLE)).line_height(px(ts::TITLE * 1.25)).font_weight(FontWeight::BLACK).child(g.name()))
                                            .child(number(format_bytes(sum), ts::TITLE + 1., pal().ink)),
                                    )
                                    .child(label_small(if wide { format!("{} · {}", g.blurb(), n_items(live_n)) } else { n_items(live_n) })),
                            )
                            .child(
                                div().w(px(sp::L)).flex_none().flex().justify_center().text_color(rgb(pal().muted)).font_weight(FontWeight::BLACK).text_size(px(ts::LEAD)).child(if open { "▾" } else { "▸" })
                                    .with_animation(ElementId::NamedInteger(format!("chev-{gi}").into(), gen * 2 + open as u64), anim(motion::QUICK + 60), |el, k| el.opacity(0.3 + 0.7 * k).mt(px(-3. * (1. - back_out(k))))),
                            ),
                    ).when(shown, |d| {
                        d.child(div().px(px(CARD_PAD)).pb(px(sp::M * tail)).flex().flex_col().children(rows.into_iter().enumerate().map(|(i, (x, k))| {
                            let w = x.bytes as f32 / max as f32;
                            let (fade, left) = self.removal(&x.id);
                            let hovered = fade == 0. && self.hovered_row.as_deref() == Some(x.id.as_str());
                            let hid = x.id.clone();
                            div()
                                .id(ElementId::Name(format!("res-{}", x.id).into()))
                                .h(px(ITEM_H * k * left))
                                .overflow_hidden()
                                .opacity(k * (1. - fade))
                                .on_hover(cx.listener(move |this, on: &bool, _, cx| this.set_hover(&hid, *on, cx)))
                                .child(
                                div().h(px(ITEM_H)).pt(px(sp::XS + sp::XXS)).pl(px(sp::XL * fade)).flex().flex_col().gap(px(sp::XS))
                                    .child(
                                        div().flex().items_center().gap(px(sp::S)).text_size(px(ts::BODY)).font_weight(FontWeight::BOLD)
                                            .child(div().size(px(10.)).flex_none().rounded_full().border_2().border_color(rgb(t.base)).when(x.default_on, |d| d.bg(rgb(t.base))))
                                            .child(fill_line(div().child(x.label.clone())))
                                            .when_some(x.detail.clone().filter(|_| !hovered), |d, det| d.child(div().flex_none().max_w(px(120.)).one_line().text_color(rgb(pal().muted)).text_size(px(ts::CAPTION)).child(det)))
                                            // the actions sit in the text line without making it taller
                                            .when(hovered, |d| d.child(div().flex_none().my(px(-(HIT - ts::BODY * 1.25) / 2.)).child(self.row_actions(&x, cx))))
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
                    ElementId::NamedInteger("gcard".into(), gi as u64 + self.results_gen * 10),
                    motion::STAGGER * (gi as u64 + 1),
                );
                if live_n > 0 {
                    return div().child(card_el);
                }
                div().opacity(1. - g_fade).max_h(px(260. * g_left)).overflow_hidden().child(card_el)
            }))
            .child(div().h(px(sp::XL)));

        let clean_btn = button("clean-safe", if safe > 0 { format!("Clean {}", format_bytes(safe)) } else { "Nothing safe to clean".into() }, Btn::Primary(pal().mint), safe > 0).on_click(cx.listener(|this, _, _, cx| {
            this.selected = this.safe_ids();
            this.open_confirm(cx);
        }));
        let choose_btn = button("choose", if wide { "Choose items" } else { "Choose" }, Btn::Ghost, self.live().next().is_some()).on_click(cx.listener(|this, _, _, cx| this.open_pick(cx)));
        if let Some((head, disk_card)) = side {
            // wide window: the summary and the actions stay put on the left, the groups scroll on the right
            return split(
                div().flex().flex_col().gap(px(sp::S)).child(head).child(disk_card),
                div().flex().flex_col().gap(px(sp::XS)).child(clean_btn).child(choose_btn),
                list.px(px(sp::S)),
            )
            .into_any_element();
        }
        div()
            .flex_1()
            .min_h(px(0.))
            .flex()
            .flex_col()
            .child(list)
            // one row: the primary action leads, the secondary one is smaller beside it
            .child(footer(pal().bg).child(div().flex().gap(px(sp::M)).child(div().w(relative(0.62)).min_w(px(0.)).child(clean_btn)).child(div().flex_1().min_w(px(0.)).child(choose_btn))))
            .into_any_element()
    }

    pub(super) fn pick(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let sel_n = self.selected.len();
        let dry = self.dry_run;
        let chip = |id: &'static str, ico: Option<Ico>, label: &'static str, on: bool| {
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
                .when_some(ico, |d, i| d.pl(px(sp::S + 2.)).gap(px(sp::XS + 2.)).child(icon(i, 18.)))
                .child(label)
        };
        let all_safe = self.selected == self.safe_ids();
        let all = self.selected.len() == self.live().count();

        let wide = self.is_wide();
        let title_el = div().text_size(px(ts::H1)).line_height(px(ts::H1 * 1.15)).font_weight(FontWeight::BLACK).child("What should Dusty eat?").into_any_element();
        let sub_el = subtitle("Pre-ticked items rebuild themselves. Purple ones cost a download later.").mt(px(sp::XS)).into_any_element();
        let chips_el = 
                div().flex().flex_wrap().items_center().gap(px(sp::S)).mt(px(sp::M))
                    .child(chip("q-safe", Some(Ico::Sparkle), "Safe picks", all_safe).on_click(cx.listener(|this, _, _, cx| {
                        this.selected = this.safe_ids();
                        this.bubble = "The safe picks. Nothing to re-download.".into();
                        this.sync_pick_dusty(cx);
                        cx.notify();
                    })))
                    .child(chip("q-all", None, "Everything", all).on_click(cx.listener(|this, _, _, cx| {
                        this.selected = this.live().map(|t| t.id.clone()).collect();
                        this.bubble = "EVERYTHING? Bold. I like it.".into();
                        this.dusty(cx, |d| d.st.set(Mood::Wow, Some(1.)));
                        this.sync_pick_dusty(cx);
                        cx.notify();
                    })))
                    .child(chip("q-none", None, "Nothing", self.selected.is_empty()).on_click(cx.listener(|this, _, _, cx| {
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
                            .child(switch("practice", dry, pal().sky)),
                    )
            .into_any_element();
        let mut intro = None;
        let mut list = div()
            .id("pick-list")
            .flex_1()
            .min_h(px(0.))
            .overflow_y_scroll()
            .track_scroll(&self.pick_scroll)
            .px(px(GUTTER));
        if wide {
            intro = Some((title_el, sub_el, chips_el));
        } else {
            list = list.child(title_el).child(sub_el).child(chips_el);
        }
        let list = list
            // headers and items are direct children of the scroll list, so ↑ ↓ can scroll a row into view
            .children(self.pick_rows().into_iter().map(|(g, item)| {
                let t = tone(g);
                let Some(x) = item else {
                    let ids: Vec<String> = self.live().filter(|x| x.group == g).map(|x| x.id.clone()).collect();
                    let all_on = ids.iter().all(|i| self.selected.contains(i));
                    // the header leaves with its group's last item
                    let (fade, left) = if ids.is_empty() {
                        self.targets.iter().filter(|x| x.group == g).map(|x| self.removal(&x.id)).fold((1f32, 0f32), |a, b| (a.0.min(b.0), a.1.max(b.1)))
                    } else {
                        (0., 1.)
                    };
                    return div().mt(px(sp::XL * left)).when(left < 1., |d| d.h(px(HIT * left)).overflow_hidden()).opacity(1. - fade).flex().items_center().gap(px(sp::S))
                        .child(icon_label(glyph(g), ts::TITLE, g.name()).flex_none().text_size(px(ts::TITLE)).font_weight(FontWeight::BLACK).text_color(rgb(t.text)))
                        .child(if wide { fill_line(label_small(g.blurb())) } else { div().flex_1() })
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
                                .child(if all_on { "CLEAR" } else { "SELECT ALL" })
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
                        )
                        .into_any_element();
                };
                let on = self.selected.contains(&x.id);
                let id = x.id.clone();
                let hid = x.id.clone();
                let risky = x.wipes_data;
                let (fade, left) = self.removal(&x.id);
                let hovered = fade == 0. && self.hovered_row.as_deref() == Some(x.id.as_str());
                let ringed = fade == 0. && self.pick_ring && self.pick_focus.as_deref() == Some(x.id.as_str());
                let detail = x.detail.clone().unwrap_or_else(|| if !x.paths.is_empty() { format!("{} path{}", x.paths.len(), plural_s(x.paths.len())) } else if let Some(c) = x.commands.first() { format!("runs {}", c.cmd) } else { String::new() });
                let row = div()
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
                    .when(ringed, |d| d.shadow(focus_ring(pal().bg)))
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
                    .on_hover(cx.listener(move |this, on: &bool, _, cx| this.set_hover(&hid, *on, cx)))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        // a click moves the (hidden) keyboard focus here too, so ↓ carries on from this row
                        this.pick_focus = Some(id.clone());
                        this.pick_ring = false;
                        this.toggle(&id, cx)
                    }))
                    .child(checkbox(format!("ck-{}", x.id).into(), on, t, 26., false))
                    .child(
                        // labels are direct children of the column so their one-line cut measures at its width
                        div().flex_1().min_w(px(0.)).flex().flex_col().gap(px(sp::XXS))
                            .child(div().text_size(px(ts::LEAD)).font_weight(FontWeight::BLACK).one_line().child(x.label.clone()))
                            .when(!detail.is_empty(), |d| d.child(label_small(detail).one_line())),
                    )
                    .when(hovered || ringed, |d| d.child(self.row_actions(&x, cx)))
                    .child(
                        div().flex_none().flex().flex_col().items_end().gap(px(sp::XXS))
                            .child(number(format_bytes(x.bytes), ts::LEAD, if on { t.text } else { pal().ink }))
                            .when(risky, |d| d.child(div().px(px(radius::XS)).py(px(1.)).rounded(px(radius::XS)).bg(rgb(pal().coral.soft)).text_color(rgb(pal().coral.text)).text_size(px(ts::MICRO)).font_weight(FontWeight::BLACK).child("wipes data"))),
                    );
                // rows keep RING_PAD of room above and below (still S apart), so a row scrolled to the list's edge
                // shows its whole focus ring
                if fade == 0. {
                    return div().mt(px(sp::S - 2. * RING_PAD)).py(px(RING_PAD)).child(row).into_any_element();
                }
                // leaving: fade and slide right, then the slot closes
                div().mt(px((sp::S - 2. * RING_PAD) * left)).py(px(RING_PAD * left)).h(px((PICK_ROW_H + 2. * RING_PAD) * left)).overflow_hidden().opacity(1. - fade).child(row.ml(px(sp::XL * fade)).mr(px(-sp::XL * fade))).into_any_element()
            }))
            .child(div().h(px(sp::XL)));

        let footer_el = footer(pal().card)
                .gap(px(sp::S - 2.))
                .child(
                    div().flex().items_center().gap(px(sp::M)).when(self.cat_here(), |d| d.child(div().flex_none().child(self.dusty.clone()))).child(
                        div().flex().flex_col().flex_1().min_w(px(0.))
                            .child(
                                div().flex().items_end().gap(px(sp::S - 2.))
                                    .child(div().flex_none().text_size(px(ts::LEAD)).font_weight(FontWeight::BLACK).mb(px(sp::XXS)).child("Free up"))
                                    .child(number(format_bytes(self.pick_shown.round() as u64), ts::H2, pal().mint.text))
                                    .child(label_small(format!("· {}", n_items(sel_n))).mb(px(sp::XS)).font(tabular(FontWeight::BOLD))),
                            )
                            .child(div().text_size(px(ts::SMALL)).font_weight(FontWeight::SEMIBOLD).text_color(rgb(pal().muted)).one_line().child(self.bubble.clone())),
                    ),
                )
                .child(button("pick-go", if dry { format!("Practice with {}", n_items(sel_n)) } else { format!("Clean {}", n_items(sel_n)) }, Btn::Primary(pal().mint), sel_n > 0).on_click(cx.listener(|this, _, _, cx| this.open_confirm(cx))));
        if let Some((t, sub, chips)) = intro {
            // wide window: title, quick picks and the total stay on the left; the items scroll on the right
            let summary = div().flex().flex_col().gap(px(sp::M)).child(t).child(sub).child(chips);
            let total = div().flex().flex_col().gap(px(sp::S)).child(
                div().flex().items_center().gap(px(sp::M)).when(self.cat_here(), |d| d.child(div().flex_none().child(self.dusty.clone()))).child(
                    div().flex().flex_col().flex_1().min_w(px(0.))
                        .child(div().flex().items_end().gap(px(sp::S - 2.)).child(div().flex_none().text_size(px(ts::LEAD)).font_weight(FontWeight::BLACK).mb(px(sp::XXS)).child("Free up")).child(number(format_bytes(self.pick_shown.round() as u64), ts::H2, pal().mint.text)))
                        .child(label_small(n_items(sel_n)).font(tabular(FontWeight::BOLD))),
                ),
            );
            let go = button("pick-go", if dry { format!("Practice with {}", n_items(sel_n)) } else { format!("Clean {}", n_items(sel_n)) }, Btn::Primary(pal().mint), sel_n > 0).on_click(cx.listener(|this, _, _, cx| this.open_confirm(cx)));
            let _ = footer_el;
            return split(summary, div().flex().flex_col().gap(px(sp::L)).child(total).child(go), list.px(px(sp::S))).into_any_element();
        }
        div().flex_1().min_h(px(0.)).flex().flex_col().child(list).child(footer_el).into_any_element()
    }

    pub(super) fn confirm_sheet(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let items = self.selected_targets();
        let n = items.len();
        let bytes: u64 = items.iter().map(|t| t.bytes).sum();
        let risky = items.iter().filter(|t| t.wipes_data).count();
        let redl = items.iter().filter(|t| t.group == Group::Redownload).count();
        let dry = self.dry_run;
        let (emoji, title, sub, go, go_tone) = match self.confirm_kind {
            ConfirmKind::Clean => (
                if risky > 0 { Ico::Oops } else if dry { Ico::Practice } else { Ico::Broom },
                if dry { format!("Practice with {}?", n_items(n)) } else { format!("Clean {}, about {}?", n_items(n), format_bytes(bytes)) },
                if dry {
                    "Nothing will be deleted. Dusty just rehearses.".to_string()
                } else if risky > 0 {
                    format!("{risky} of them wipe real data (not just caches). This can't be undone.")
                } else if redl > 0 {
                    format!("{redl} will need a re-download next time you use them, so they go to the Trash. Caches are deleted and rebuild by themselves.")
                } else {
                    "Caches are deleted and rebuild by themselves; logs go to the Trash. Nothing personal is touched.".to_string()
                },
                if dry { "Start practice" } else { "Yes, clean it" },
                if risky > 0 { pal().coral } else { pal().mint },
            ),
            kind => self.tool_sheet_text(kind),
        };
        // in: backdrop fades, sheet springs up. out: both leave faster than they came.
        let k_in = since(self.sheet_t, motion::SHEET);
        let k_out = self.confirm_closing.map(|t| since(t, motion::EXIT)).unwrap_or(0.);
        let backdrop = since(self.sheet_t, motion::BASE) * (1. - k_out);
        let wide = self.is_wide();
        let drop = if wide { 24. * (1. - ease_out_cubic(k_in)) + 16. * k_out } else { 380. * (1. - back_out(k_in)) + 420. * k_out * k_out };
        div()
            .id("sheet-wrap")
            .absolute()
            .inset_0()
            .occlude()
            .flex()
            .flex_col()
            .when(wide, |d| d.justify_center().items_center())
            .when(!wide, |d| d.justify_end())
            .child(div().id("sheet-bg").absolute().inset_0().bg(alpha(pal().scrim, pal().scrim_a)).opacity(backdrop).on_click(cx.listener(|this, _, _, cx| this.close_confirm(cx))))
            .child(
                div()
                    .id("sheet")
                    .relative()
                    .w_full()
                    .max_w(px(if wide { 460. } else { MAX_W }))
                    .mx_auto()
                    .bg(rgb(pal().card))
                    .when(wide, |d| d.rounded(px(radius::SHEET)).border_2().pt(px(sp::XL)))
                    .when(!wide, |d| d.rounded_t(px(radius::SHEET)))
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
                    .child(sheet_icon(emoji, ElementId::NamedInteger("sheet-emoji".into(), self.nav)))
                    .child(div().text_size(px(ts::STAT)).line_height(px(ts::STAT * 1.25)).font(tabular(FontWeight::BLACK)).text_center().child(title))
                    .child(subtitle(sub).text_center())
                    .child(div().h(px(sp::S)))
                    .child(button("confirm-go", go, Btn::Primary(go_tone), true).on_mouse_up(MouseButton::Left, move |_, _, _| if !dry { haptic(Haptic::Double) }).on_click(cx.listener(|this, _, _, cx| this.confirm_go(cx))))
                    .child(button("confirm-no", "Not yet", Btn::Ghost, true).on_click(cx.listener(|this, _, _, cx| this.close_confirm(cx)))),
            )
    }

    pub(super) fn cleaning(&mut self, window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
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
            .child(div().mt(px(sp::S - 2.)).child(self.cat()))
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
                            .child(icon_raster(glyph(t.group), 22., 22. * s))
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

    pub(super) fn done(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let job_dry = self.clean.as_ref().map(|j| j.dry).unwrap_or(false);
        let ok = self.clean.as_ref().map(|j| j.ok_count).unwrap_or(0);
        let failed = self.clean.as_ref().map(|j| j.failed.clone()).unwrap_or_default();
        let freed = count_up(self.done_freed, self.st(), motion::FILL + 300);
        let estimated = self.done_estimated;
        let disk = self.done_disk;
        let before = self.done_before;
        // what free space reads after: the real disk, or (practice run / estimate) before + what was eaten
        let after_free = if job_dry || estimated { (before.free + self.done_freed).min(before.total) } else { disk.free.max(before.free) };
        // the freed slice waits for the card to land, then melts away
        let melt = since(self.st() + Duration::from_millis(3 * motion::STAGGER + motion::ENTER), motion::FILL + 200);
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
                    .child(div().mt(px(sp::XS)).child(self.cat()))
                    .child(div().text_size(px(ts::LEAD)).font_weight(FontWeight::BLACK).text_color(rgb(pal().muted)).child(if job_dry { "Would free" } else if estimated { "Freed (estimated)" } else { "Freed" }))
                    .child(number(format_bytes(freed), ts::HERO_XL, pal().mint.text)),
            )
            .child(div().flex().gap(px(sp::M)).mt(px(sp::M)).child(stat("Free space now", format_bytes(disk.free), pal().mint, 2 * motion::STAGGER, 0)).child(stat("Items cleaned", ok.to_string(), pal().sky, 4 * motion::STAGGER, 1)))
            .when(after_free > before.free, |d| {
                d.child(pop_in(
                    card().mt(px(sp::M)).p(px(CARD_PAD)).flex().flex_col().gap(px(sp::S + sp::XXS))
                        .child(
                            div().flex().justify_between().items_center()
                                .child(label_small(if job_dry { "Your disk, if this were real" } else { "Your disk, before → after" }))
                                .child(div().flex().items_center().gap(px(sp::XS + 1.)).child(div().size(px(10.)).rounded(px(3.)).bg(rgb(pal().mint.base))).child(label_small("freed"))),
                        )
                        .child(freed_bar(before.total, before.free, after_free, melt))
                        .child(
                            div().flex().items_center().gap(px(sp::S)).font(tabular(FontWeight::BLACK)).text_size(px(ts::LEAD))
                                .child(div().text_color(rgb(pal().muted)).child(format!("{} free", format_bytes(before.free))))
                                .child(div().text_color(rgb(pal().muted)).child("→"))
                                .child(div().text_color(rgb(pal().mint.text)).child(format!("{} free", format_bytes(after_free)))),
                        ),
                    ElementId::NamedInteger("before-after".into(), nav),
                    3 * motion::STAGGER,
                ))
            })
            .when(self.done_trashed > 0, |d| {
                d.child(pop_in(
                    self.trash_note(self.done_trashed, job_dry, &self.done_put_back, self.done_entry.clone(), tools::PutBackFor::Clean, cx).mt(px(sp::M)),
                    ElementId::NamedInteger("trash-note".into(), nav),
                    4 * motion::STAGGER,
                ))
            })
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
        div().flex_1().min_h(px(0.)).flex().flex_col().child(list).child(footer(pal().bg).child(button("done-home", "Done", Btn::Primary(pal().mint), true).on_click(cx.listener(|this, _, _, cx| this.leave_done(cx)))))
    }

    /// Purple splash: a white badge pops in while dust specks sweep into it, then the badge bursts
    /// into an iris that opens on Dusty and reveals the home screen. Click to skip.
    pub(super) fn splash(&self, t: f32, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
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
                        .child(icon_raster(Ico::Sparkle, 60., badge * 0.58)),
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

    pub(super) fn particles_layer(&self) -> impl IntoElement {
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
