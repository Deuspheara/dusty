//! Big & old files: finding them, picking some, and moving the picks to the Trash.

use super::*;

impl DustyApp {
    pub(super) fn start_large(&mut self, cx: &mut Context<Self>) {
        if let Some(j) = self.tools.large_job.take() {
            j.stop();
        }
        let job: Job<LargeProgress> = Job::new();
        let (progress, cancel) = (job.progress.clone(), job.cancel.clone());
        self.tools.large_job = Some(job);
        self.tools.large = None;
        self.tools.large_seen = 0;
        self.tools.large_found = 0;
        self.tools.large_sel.clear();
        self.tools.large_result = None;
        self.tools.large_put_back = PutBack::No;
        self.tools.focus = None;
        self.bubble = "Digging through Downloads, Desktop, Movies and Documents…".into();
        self.dusty(cx, |d| d.st.set(Mood::Think, None));
        let flag = cancel.clone();
        let task = cx.background_spawn(async move {
            let opts = LargeFileOptions::default();
            let files = large::find_large_files(&opts, &progress, &cancel);
            let partial = progress.lock().unwrap().partial;
            (files, partial)
        });
        cx.spawn(async move |this, cx| {
            let (files, partial) = task.await;
            let _ = this.update(cx, |app, cx| {
                if !app.tools.large_job.as_ref().is_some_and(|j| j.is(&flag)) {
                    return;
                }
                app.tools.large_job = None;
                app.tools.large_partial = partial;
                let total: u64 = files.iter().map(|f| f.bytes).sum();
                if files.is_empty() {
                    app.bubble = "No forgotten giants. Your folders are tidy!".into();
                    app.dusty(cx, |d| d.st.set(Mood::Happy, Some(1.5)));
                } else {
                    app.bubble = format!("{} files, {} in all. Nothing is ticked: you pick.", files.len(), format_bytes(total)).into();
                    app.dusty(cx, |d| d.st.set(if total >= WOW_FILE { Mood::Wow } else { Mood::Happy }, Some(1.)));
                }
                app.tools.large = Some(Rc::new(files));
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    pub(super) fn toggle_large(&mut self, p: &PathBuf, cx: &mut Context<Self>) {
        if !self.tools.large_sel.remove(p) {
            self.tools.large_sel.insert(p.clone());
            let f = self.tools.large.iter().flat_map(|l| l.iter()).find(|f| &f.path == p);
            if f.is_some_and(|f| f.hard_linked) {
                self.bubble = "Heads up: that one has twins (hard links). Trashing it frees nothing yet.".into();
                self.dusty(cx, |d| d.st.set(Mood::Think, Some(1.)));
            } else if let Some(f) = f {
                self.bubble = format!("{}? Into the bowl it goes.", format_bytes(f.bytes)).into();
                self.dusty(cx, |d| d.st.poke(0.));
            }
        } else {
            self.bubble = "Okay, that one stays.".into();
        }
        cx.notify();
    }

    pub(super) fn large_sel_bytes(&self) -> u64 {
        self.tools.large.iter().flat_map(|l| l.iter()).filter(|f| self.tools.large_sel.contains(&f.path)).map(|f| f.bytes).sum()
    }

    pub(in crate::ui) fn start_trash_files(&mut self, cx: &mut Context<Self>) {
        if self.just_arrived() || self.confirm_closing.is_some() {
            return;
        }
        self.confirm = false;
        let files: Vec<PathBuf> = self.tools.large.iter().flat_map(|l| l.iter()).filter(|f| self.tools.large_sel.contains(&f.path)).map(|f| f.path.clone()).collect();
        if files.is_empty() || self.tools.large_busy {
            return;
        }
        let dry = self.dry_run;
        self.tools.large_busy = true;
        self.tools.chomps = files.len().min(6) as u32;
        self.tools.next_chomp = Instant::now() + Duration::from_millis(200);
        self.bubble = if dry { "Practice run: pretend-chomping…".into() } else { "Om nom nom. Off to the Trash!".into() };
        self.dusty(cx, |d| d.st.set(Mood::Hungry, None));
        let task = cx.background_spawn(async move { large::trash_files(&files, dry) });
        cx.spawn(async move |this, cx| {
            let r = task.await;
            let _ = this.update(cx, |app, cx| {
                app.tools.large_busy = false;
                let moved: HashSet<PathBuf> = r.entry.items.iter().flat_map(|i| &i.paths).filter(|p| p.action == dusty_core::history::PathAction::Trashed).map(|p| p.path.clone()).collect();
                if let Some(list) = app.tools.large.as_mut().map(Rc::make_mut) {
                    list.retain(|f| !moved.contains(&f.path));
                }
                app.tools.large_sel.retain(|p| !moved.contains(p));
                app.tools.focus = None;
                if r.entry.practice {
                    app.bubble = "Practice done! Nothing moved.".into();
                    app.dusty(cx, |d| d.st.set(Mood::Happy, Some(1.5)));
                } else if r.errors.is_empty() {
                    app.bubble = format!("Gulp. {} is in the Trash.", format_bytes(r.entry.bytes)).into();
                    app.dusty(cx, |d| d.st.set(Mood::Happy, Some(2.)));
                    app.burst_center(70, cx);
                } else {
                    app.tools.chomps = 0;
                    app.bubble = "Some of them wouldn't budge.".into();
                    app.dusty(cx, |d| d.st.set(Mood::Sad, Some(1.5)));
                }
                app.tools.large_put_back = PutBack::No;
                app.tools.large_result = Some(r);
                app.nav_t = Instant::now();
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    pub(super) fn large_screen(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let p = pal();
        let Some(files) = self.tools.large.clone() else {
            let (seen, n, msg) = self.tools.large_job.as_ref().map(|j| {
                let pr = j.progress.lock().unwrap();
                (pr.files_seen, pr.found.len(), pr.message.clone())
            }).unwrap_or_default();
            return self.tool_busy("Big & old files", format_bytes(self.tools.large_found), format!("in {n} big or forgotten files so far"), format!("{seen} files looked at · {msg}"), cx).into_any_element();
        };
        let n_sel = self.tools.large_sel.len();
        let sel_bytes = self.large_sel_bytes();
        let nav = self.nav;
        let result = self.tools.large_result.clone();
        let banner = div()
            .px(px(GUTTER))
            .pb(px(sp::S))
            .child(self.tool_header("Big & old files"))
            .when_some(result, |d, r| {
                let practice = r.entry.practice;
                d.child(pop_in(
                    div().mt(px(sp::M)).flex().flex_col().gap(px(sp::S))
                        .when(r.entry.bytes > 0, |d| d.child(self.trash_note(r.entry.bytes, practice, &self.tools.large_put_back, (!practice).then(|| r.entry.id.clone()), PutBackFor::Large, cx)))
                        .when(!r.errors.is_empty(), |d| d.child(notice(p.sun, p.warn).child(div().font_weight(FontWeight::BLACK).child(format!("{} file{} stayed put", r.errors.len(), plural_s(r.errors.len())))).children(r.errors.iter().take(3).map(|e| div().one_line().child(e.clone()))))),
                    ElementId::NamedInteger("large-result".into(), nav),
                    motion::STAGGER,
                ))
            })
            .when(self.tools.large_partial, |d| d.child(notice(p.sun, p.warn).mt(px(sp::S)).child(icon_label(Ico::Warning, ts::BODY, "I ran out of time before every folder, so a few may be missing. ⌘R looks again.").items_start())));
        let list = div()
            .id("large-list")
            .flex_1()
            .min_h(px(0.))
            .overflow_y_scroll()
            .track_scroll(&self.tools.scroll)
            .px(px(GUTTER))
            .when(files.is_empty(), |d| {
                d.child(pop_in(
                    card().mt(px(sp::M)).px(px(sp::XL)).py(px(sp::XL)).flex().flex_col().items_center().gap(px(sp::XS))
                        .child(hero_icon(Ico::Sparkle))
                        .child(div().text_size(px(ts::TITLE)).font_weight(FontWeight::BLACK).child("No forgotten giants"))
                        .child(subtitle("Nothing over 200 MB, and nothing you haven't opened in six months, in Downloads, Desktop, Movies or Documents.").text_center()),
                    ElementId::NamedInteger("large-empty".into(), nav),
                    motion::STAGGER,
                ))
            })
            .children(files.iter().enumerate().map(|(i, f)| {
                let on = self.tools.large_sel.contains(&f.path);
                let (e, t) = kind_look(f.kind);
                let key = f.path.to_string_lossy().into_owned();
                let hid = format!("large-{key}");
                let hid2 = hid.clone();
                let hovered = self.tools.hovered.as_deref() == Some(hid.as_str());
                let ringed = self.tool_ring(i);
                let (path, rpath) = (f.path.clone(), f.path.clone());
                let name = dusty_core::name_of(&f.path);
                let folder = f.path.parent().map(tildify).unwrap_or_default();
                let age = format!("opened {}", ago(secs_since(f.last_used)));
                let row = div()
                    .id(ElementId::Name(hid.clone().into()))
                    .flex()
                    .items_center()
                    .gap(px(sp::M))
                    .px(px(sp::M + sp::XXS))
                    .py(px(sp::S + sp::XXS))
                    .rounded(px(radius::L))
                    .border_2()
                    .border_b_4()
                    .border_color(rgb(if on { t.base } else { p.line }))
                    .bg(rgb(if on { t.soft } else { p.card }))
                    .when(ringed, |d| d.shadow(focus_ring(p.bg)))
                    .cursor_pointer()
                    .hover(move |s| s.border_color(rgb(t.base)))
                    .active(|s| s.border_b_2().mt(px(2.)))
                    .on_mouse_down(MouseButton::Left, |_, _, _| haptic(Haptic::Tick))
                    .on_hover(cx.listener(move |this, on: &bool, _, cx| this.set_tool_hover(&hid2, *on, cx)))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.tools.focus = Some(i);
                        this.tools.ring = false;
                        this.toggle_large(&path, cx);
                    }))
                    .child(checkbox(format!("fck-{i}").into(), on, t, 24., false))
                    .child(icon_tile(e, t, 32.))
                    .child(
                        // name and size share the first line; folder and age get the full width under them
                        div().flex_1().min_w(px(0.)).flex().flex_col().gap(px(sp::XXS))
                            .child(
                                div().flex().items_center().gap(px(sp::S))
                                    .child(fill_line(div().text_size(px(ts::BODY)).font_weight(FontWeight::BLACK).child(name)))
                                    .when(f.hard_linked, |d| d.child(small_chip("HARD LINK", p.sun)))
                                    .child(number(format_bytes(f.bytes), ts::BODY, if on { t.text } else { p.ink })),
                            )
                            .child(div().flex().items_center().gap(px(sp::XS)).child(fill_line(label_small(folder))).child(label_small(format!("· {age}")).flex_none())),
                    )
                    .when(hovered, |d| d.child(row_action(format!("freveal-{i}"), Ico::Reveal, "Show in Finder", p.grape).on_click(cx.listener(move |this, _, _, cx| this.reveal_path(rpath.clone(), cx)))));
                div().mt(px(sp::S - 2. * RING_PAD)).py(px(RING_PAD)).child(row)
            }))
            .child(div().h(px(sp::XL)));
        let go = if self.tools.large_busy {
            "Moving…".to_string()
        } else if n_sel == 0 {
            "Tick the ones to toss".to_string()
        } else if self.dry_run {
            format!("Practice with {n_sel} · {}", format_bytes(sel_bytes))
        } else {
            format!("Move {n_sel} to Trash · {}", format_bytes(sel_bytes))
        };
        div()
            .flex_1()
            .min_h(px(0.))
            .flex()
            .flex_col()
            .child(banner)
            .child(list)
            .child(footer(p.bg).child(button("large-go", go, Btn::Primary(p.coral), n_sel > 0 && !self.tools.large_busy).on_click(cx.listener(|this, _, _, cx| {
                if !this.tools.large_sel.is_empty() && !this.tools.large_busy {
                    this.open_sheet(ConfirmKind::TrashFiles, cx)
                }
            }))))
            .into_any_element()
    }
}
