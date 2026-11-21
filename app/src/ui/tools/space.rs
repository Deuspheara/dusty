//! What's using space: the treemap of your home folder, one level at a time.

use super::*;

impl DustyApp {
    pub(super) fn start_usage(&mut self, root: PathBuf, cx: &mut Context<Self>) {
        if let Some(j) = self.tools.usage_job.take() {
            j.stop();
        }
        let job: Job<UsageProgress> = Job::new();
        let (progress, cancel) = (job.progress.clone(), job.cancel.clone());
        self.tools.usage_job = Some(job);
        self.tools.usage_hover = None;
        let name = if root == dusty_core::home() { "your home folder".to_string() } else { dusty_core::name_of(&root) };
        self.bubble = format!("Measuring {name}… big folders take a moment.").into();
        self.dusty(cx, |d| d.st.set(Mood::Think, None));
        let flag = cancel.clone();
        let task = cx.background_spawn(async move {
            let opts = UsageOptions { root, depth: 2, top_n: 14, budget: Duration::from_secs(40) };
            usage::usage_tree(&opts, &progress, &cancel)
        });
        cx.spawn(async move |this, cx| {
            let node = task.await;
            let _ = this.update(cx, |app, cx| {
                if !app.tools.usage_job.as_ref().is_some_and(|j| j.is(&flag)) {
                    return;
                }
                app.tools.usage_job = None;
                let Some(node) = node else {
                    app.toast("Dusty couldn't read that folder.");
                    app.dusty(cx, |d| d.st.set(Mood::Sad, Some(1.)));
                    cx.notify();
                    return;
                };
                app.usage_mood(&node, cx);
                app.tools.usage_stack.push(Rc::new(node));
                app.nav_t = Instant::now();
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    pub(super) fn usage_mood(&mut self, node: &UsageNode, cx: &mut Context<Self>) {
        let top = node.children.iter().find(|c| !matches!(c.kind, NodeKind::Other { .. }));
        match top {
            Some(c) if node.bytes > 0 && c.bytes as f64 / node.bytes as f64 > 0.4 => {
                self.bubble = format!("Whoa, {} alone is {}!", c.name, format_bytes(c.bytes)).into();
                self.dusty(cx, |d| d.st.set(Mood::Wow, Some(1.)));
            }
            Some(c) => {
                self.bubble = format!("{} is the biggest block here: {}.", c.name, format_bytes(c.bytes)).into();
                self.dusty(cx, |d| d.st.set(Mood::Happy, Some(1.)));
            }
            None => {
                self.bubble = "Nothing in here. Light as air.".into();
                self.dusty(cx, |d| d.st.set(Mood::Idle, None));
            }
        }
    }

    pub(super) fn usage_click(&mut self, i: usize, cx: &mut Context<Self>) {
        if self.tools.usage_job.is_some() {
            return;
        }
        let Some(c) = self.tools.usage_stack.last().and_then(|n| n.children.get(i)).cloned() else { return };
        match c.kind {
            NodeKind::Other { count } => self.toast(&format!("{count} smaller things, lumped together.")),
            NodeKind::File => {
                if let Some(p) = c.path {
                    self.reveal_path(p, cx);
                }
            }
            NodeKind::Dir if c.in_cloud => self.toast("That one lives in iCloud, so it takes no space here."),
            NodeKind::Dir if !c.children.is_empty() => {
                haptic(Haptic::Tick);
                self.usage_mood(&c, cx);
                self.tools.usage_stack.push(Rc::new(c));
                self.tools.usage_hover = None;
                self.nav_t = Instant::now();
                cx.notify();
            }
            NodeKind::Dir => {
                if let Some(p) = c.path {
                    haptic(Haptic::Tick);
                    self.start_usage(p, cx);
                }
            }
        }
    }

    pub(super) fn usage_screen(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let p = pal();
        let Some(node) = self.tools.usage_stack.last().cloned() else {
            let (msg, n) = self.tools.usage_job.as_ref().map(|j| {
                let pr = j.progress.lock().unwrap();
                (pr.message.clone(), pr.entries)
            }).unwrap_or_default();
            return self.tool_busy("What's using space", n.to_string(), "folders and files so far".into(), if msg.is_empty() { "Listing folders…".into() } else { msg }, cx).into_any_element();
        };
        let drilling = self.tools.usage_job.is_some();
        let depth = self.tools.usage_stack.len();
        let tones = [p.grape, p.sky, p.mint, p.sun, p.coral, p.teal, p.blue];
        // the map fills the room left between the header and the footer
        let (w, h) = ((self.view.0 - 2. * GUTTER).max(200.), (self.view.1 - TOP - 296.).clamp(200., 900.));
        let rects = node.layout(Rect::new(0., 0., w, h));
        let hover = self.tools.usage_hover.and_then(|i| node.children.get(i).map(|c| (i, c)));
        let info: String = match &hover {
            Some((_, c)) => {
                let pct = if node.bytes > 0 { c.bytes as f64 * 100. / node.bytes as f64 } else { 0. };
                let what = match c.kind {
                    NodeKind::Dir if c.in_cloud => " · in iCloud".to_string(),
                    NodeKind::Dir => " · tap to look inside".to_string(),
                    NodeKind::File => " · tap to show in Finder".to_string(),
                    NodeKind::Other { .. } => String::new(),
                };
                format!("{} · {} · {:.0}%{what}", c.name, format_bytes(c.bytes), pct)
            }
            None => format!("{} in {} · tap a block to look inside", format_bytes(node.bytes), if depth == 1 { "your home folder".to_string() } else { node.name.clone() }),
        };
        let crumbs = div().flex().flex_wrap().items_center().gap(px(sp::XS)).mt(px(sp::M)).children(self.tools.usage_stack.iter().enumerate().map(|(i, n)| {
            let last = i + 1 == depth;
            let name = if i == 0 { "Home".to_string() } else { n.name.clone() };
            div().flex().items_center().gap(px(sp::XS))
                .when(i > 0, |d| d.child(label_small("›")))
                .child(
                    div().id(("crumb", i)).px(px(sp::S)).h(px(HIT - 2.)).flex().items_center().rounded_full().text_size(px(ts::SMALL)).font_weight(FontWeight::BLACK)
                        .text_color(rgb(if last { p.ink } else { p.grape.text }))
                        .bg(rgb(if last { p.card } else { p.grape.soft }))
                        .when(last, |d| d.border_1().border_color(rgb(p.line)))
                        .when(!last, |d| {
                            d.cursor_pointer().hover(|s| s.bg(rgb(pal().card))).on_mouse_down(MouseButton::Left, |_, _, _| haptic(Haptic::Tick)).on_click(cx.listener(move |this, _, _, cx| {
                                if let Some(j) = this.tools.usage_job.take() {
                                    j.stop();
                                }
                                this.tools.usage_stack.truncate(i + 1);
                                this.tools.usage_hover = None;
                                if let Some(n) = this.tools.usage_stack.last().cloned() {
                                    this.usage_mood(&n, cx);
                                }
                                this.nav_t = Instant::now();
                                cx.notify();
                            }))
                        })
                        .child(div().max_w(px(140.)).one_line().child(name)),
                )
        }));
        let gen = self.nav * 100 + depth as u64;
        let tiles = node.children.iter().zip(rects.iter()).enumerate().filter(|(_, (_, r))| r.w >= 6. && r.h >= 6.).map(|(i, (c, r))| {
            let other = matches!(c.kind, NodeKind::Other { .. });
            let t = if other { Tone { base: p.used, edge: p.used, soft: p.line, hover: p.used, text: p.muted } } else { tones[i % tones.len()] };
            let gap = 2.;
            let (x, y, tw, th) = (r.x + gap, r.y + gap, (r.w - 2. * gap).max(0.), (r.h - 2. * gap).max(0.));
            let big = tw > 72. && th > 54.;
            // tiny tiles: no padding or chunky edge, or they'd grow past their slot (and the map)
            let tiny = tw.min(th) < 14.;
            // a size alone, only where it fits whole (about 6.5 px per character at MICRO)
            let mid = tw > format_bytes(c.bytes).len() as f32 * 8. + 2. * sp::XS + 6. && th > 22.;
            let hovered = self.tools.usage_hover == Some(i);
            let rpath = c.path.clone();
            // files get the file icon; iCloud folders say so (they take no space here)
            let glyph = matches!(c.kind, NodeKind::File).then_some(Ico::BigFiles);
            let cloud = if matches!(c.kind, NodeKind::Dir) && c.in_cloud { " (iCloud)" } else { "" };
            div()
                .id(("tile", i))
                .absolute()
                .left(px(x))
                .top(px(y))
                .w(px(tw))
                .h(px(th))
                .overflow_hidden()
                .rounded(px(if tw.min(th) > 40. { radius::M } else { radius::XS }))
                .bg(if hovered { alpha(t.base, 0.38) } else { rgb(t.soft).into() })
                .border_color(rgb(t.base))
                .when(tw.min(th) > 24., |d| d.border_2().border_b_4())
                .when(!tiny && tw.min(th) <= 24., |d| d.border_1())
                .when(tiny, |d| d.bg(rgb(t.base)).rounded(px(2.)))
                .when(!tiny, |d| d.px(px(if big { sp::S + sp::XXS } else { sp::XS })).py(px(if big { sp::S } else { sp::XXS })))
                .flex()
                .flex_col()
                .when(!other, |d| d.cursor_pointer())
                .on_hover(cx.listener(move |this, on: &bool, _, cx| {
                    if *on {
                        this.tools.usage_hover = Some(i);
                    } else if this.tools.usage_hover == Some(i) {
                        this.tools.usage_hover = None;
                    }
                    cx.notify();
                }))
                .on_click(cx.listener(move |this, _, _, cx| this.usage_click(i, cx)))
                .when(big, |d| {
                    d.child(
                        div().flex().items_center().gap(px(sp::XS)).min_w(px(0.)).when_some(glyph, |d, i| d.child(icon(i, 16.)))
                            .child(div().flex_1().min_w(px(0.)).text_size(px(ts::SMALL)).line_height(px(ts::SMALL * 1.3)).font_weight(FontWeight::BLACK).text_color(rgb(t.text)).one_line().child(format!("{}{cloud}", c.name))),
                    )
                        .child(div().text_size(px(ts::CAPTION)).font(tabular(FontWeight::BOLD)).text_color(rgb(p.ink)).one_line().child(format_bytes(c.bytes)))
                })
                .when(!big && mid, |d| d.child(div().text_size(px(ts::MICRO)).font(tabular(FontWeight::BLACK)).text_color(rgb(t.text)).one_line().child(format_bytes(c.bytes))))
                .when(hovered && big && th > 64. && rpath.is_some(), |d| {
                    let rp = rpath.clone().unwrap();
                    d.child(div().absolute().right(px(sp::XS)).bottom(px(sp::XS)).child(row_action(format!("treveal-{i}"), Ico::Reveal, "Show in Finder", p.grape).on_click(cx.listener(move |this, _, _, cx| this.reveal_path(rp.clone(), cx)))))
                })
                .with_animation(ElementId::NamedInteger(format!("tile-{i}").into(), gen), anim(motion::ENTER + (i.min(10) as u64) * motion::STAGGER / 2), move |el, k| {
                    let start = (i.min(10) as f32 * motion::STAGGER as f32 / 2.) / (motion::ENTER as f32 + i.min(10) as f32 * motion::STAGGER as f32 / 2.);
                    let k = ((k - start) / (1. - start)).clamp(0., 1.);
                    el.opacity(k).top(px(y + 8. * (1. - back_out(k))))
                })
        });
        let map = div()
            .relative()
            .w(px(w))
            .h(px(h))
            .mt(px(sp::S))
            .when(node.children.is_empty(), |d| d.flex().items_center().justify_center().rounded(px(radius::XL)).border_2().border_color(rgb(p.line)).child(subtitle("Empty folder.")))
            .children(tiles)
            .when(drilling, |d| {
                d.child(div().absolute().inset_0().rounded(px(radius::M)).bg(alpha(p.bg, 0.75)).flex().flex_col().items_center().justify_center().gap(px(sp::S)).px(px(sp::XXL))
                    .child(label_small("Measuring…"))
                    .child(div().w(px(220.)).child(busy_bar())))
            });
        let reveal = node.path.clone();
        let here = if depth == 1 { "Home".to_string() } else if node.name.chars().count() > 16 { "this folder".to_string() } else { node.name.clone() };
        div()
            .flex_1()
            .min_h(px(0.))
            .flex()
            .flex_col()
            .child(
                self.page()
                    .child(self.tool_header("What's using space"))
                    .child(crumbs)
                    .when(node.partial, |d| d.child(notice(p.sun, p.warn).mt(px(sp::S)).py(px(sp::S)).child(icon_label(Ico::Warning, ts::BODY, "Partial: I ran out of time, so some sizes are low. ⌘R measures again.").items_start())))
                    .child(div().mt(px(sp::S)).h(px(ts::BODY * 1.5)).text_size(px(ts::BODY)).font(tabular(FontWeight::BOLD)).text_color(rgb(if hover.is_some() { p.ink } else { p.muted })).one_line().child(info))
                    .child(map),
            )
            .child(footer(p.bg).child(button("usage-reveal", format!("Show {here} in Finder"), Btn::Ghost, reveal.is_some()).on_click(cx.listener(move |this, _, _, cx| {
                if let Some(p) = reveal.clone() {
                    this.reveal_path(p, cx);
                }
            }))))
            .into_any_element()
    }
}
