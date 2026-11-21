//! Uninstall apps: the app list, one app's leftovers, and moving them to the Trash.

use super::*;

impl DustyApp {
    pub(super) fn start_apps(&mut self, cx: &mut Context<Self>) {
        if let Some(j) = self.tools.apps_job.take() {
            j.stop();
        }
        let job: Job<AppListProgress> = Job::new();
        let (progress, cancel) = (job.progress.clone(), job.cancel.clone());
        self.tools.apps_job = Some(job);
        self.tools.focus = None;
        self.bubble = "*sniff* Let me count your apps…".into();
        self.dusty(cx, |d| d.st.set(Mood::Think, None));
        let flag = cancel.clone();
        let task = cx.background_spawn(async move { uninstall::list_apps(&AppListOptions::default(), &progress, &cancel) });
        cx.spawn(async move |this, cx| {
            let apps = task.await;
            let _ = this.update(cx, |app, cx| app.apps_done(apps, flag, cx));
        })
        .detach();
        cx.notify();
    }

    pub(super) fn apps_done(&mut self, mut apps: Vec<InstalledApp>, flag: Arc<AtomicBool>, cx: &mut Context<Self>) {
        if !self.tools.apps_job.as_ref().is_some_and(|j| j.is(&flag)) {
            return;
        }
        self.tools.apps_job = None;
        apps.sort_by(|a, b| b.bytes.cmp(&a.bytes).then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase())));
        if let Some(big) = apps.first().filter(|a| a.bytes >= WOW_APP) {
            self.bubble = format!("{} takes {}. Chonky! Pick one to see its crumbs.", big.name, format_bytes(big.bytes)).into();
            self.dusty(cx, |d| d.st.set(Mood::Wow, Some(1.)));
        } else if apps.is_empty() {
            self.bubble = "No apps I'm allowed to touch. Apple's own stay put.".into();
            self.dusty(cx, |d| d.st.set(Mood::Sad, Some(1.)));
        } else {
            self.bubble = format!("{} apps, biggest first. Pick one to see its crumbs.", apps.len()).into();
            self.dusty(cx, |d| d.st.set(Mood::Happy, Some(1.)));
        }
        // icons load in the background, biggest apps first
        let paths: Vec<PathBuf> = apps.iter().map(|a| a.path.clone()).collect();
        let (icons, loading) = (self.tools.icons.clone(), self.tools.icons_loading.clone());
        loading.store(true, Ordering::Relaxed);
        std::thread::spawn(move || {
            for p in paths {
                if icons.lock().unwrap().contains_key(&p) {
                    continue;
                }
                if let Some(png) = platform::app_icon_png(&p, 72.) {
                    icons.lock().unwrap().insert(p, Arc::new(Image::from_bytes(ImageFormat::Png, png)));
                }
            }
            loading.store(false, Ordering::Relaxed);
        });
        self.tools.apps = Some(apps);
        cx.notify();
    }

    pub(super) fn filtered_apps(&self) -> Vec<InstalledApp> {
        let q = self.tools.search.trim().to_lowercase();
        self.tools
            .apps
            .iter()
            .flatten()
            .filter(|a| q.is_empty() || a.name.to_lowercase().contains(&q) || a.bundle_id.as_deref().is_some_and(|b| b.to_lowercase().contains(&q)))
            .cloned()
            .collect()
    }

    pub(super) fn app_icon(&self, a: &InstalledApp, side: f32) -> AnyElement {
        match self.tools.icons.lock().unwrap().get(&a.path).cloned() {
            Some(i) => img(i).size(px(side)).flex_none().into_any_element(),
            None => icon_tile(Ico::Uninstall, pal().sky, side).into_any_element(),
        }
    }

    pub(super) fn open_app(&mut self, a: InstalledApp, cx: &mut Context<Self>) {
        self.tools.app = Some(a.clone());
        self.tools.leftovers = None;
        self.tools.left_sel.clear();
        self.tools.app_result = None;
        self.tools.app_put_back = PutBack::No;
        self.bubble = format!("Sniffing around ~/Library for {}…", a.name).into();
        self.dusty(cx, |d| d.st.set(Mood::Think, None));
        self.check_app(cx);
        self.go(Screen::AppDetail, cx);
    }

    /// Look the app up again (is it open now?) and find its leftovers, in the background.
    pub(super) fn check_app(&mut self, cx: &mut Context<Self>) {
        let Some(a) = self.tools.app.clone() else { return };
        if let Some(j) = self.tools.left_job.take() {
            j.stop();
        }
        let job: Job<()> = Job::new();
        let flag = job.cancel.clone();
        self.tools.left_job = Some(job);
        let task = cx.background_spawn(async move {
            let fresh = uninstall::app_at(&a.path);
            let lefts = uninstall::find_leftovers(&a);
            (fresh, lefts)
        });
        cx.spawn(async move |this, cx| {
            let (fresh, lefts) = task.await;
            let _ = this.update(cx, |app, cx| {
                if !app.tools.left_job.as_ref().is_some_and(|j| j.is(&flag)) {
                    return;
                }
                app.tools.left_job = None;
                let Some(cur) = app.tools.app.as_mut() else { return };
                match fresh {
                    Some(f) => {
                        cur.running = f.running;
                        cur.writable = f.writable;
                        if f.bytes > 0 {
                            cur.bytes = f.bytes;
                        }
                    }
                    None => {
                        app.toast("That app isn't there any more.");
                        app.back_to_apps(cx);
                        return;
                    }
                }
                let (name, running, writable) = (cur.name.clone(), cur.running, cur.writable);
                app.tools.left_sel = lefts.iter().filter(|l| l.preselected).map(|l| l.path.clone()).collect();
                let left_bytes: u64 = lefts.iter().map(|l| l.bytes).sum();
                if running {
                    app.bubble = format!("{name} is open right now. Quit it first, then I'll help.").into();
                    app.dusty(cx, |d| d.st.set(Mood::Sad, Some(1.2)));
                } else if !writable {
                    app.bubble = format!("{name} belongs to every user on this Mac. That needs an admin.").into();
                    app.dusty(cx, |d| d.st.set(Mood::Think, Some(1.2)));
                } else if lefts.is_empty() {
                    app.bubble = "No crumbs in your Library. Tidy app!".into();
                    app.dusty(cx, |d| d.st.set(Mood::Happy, Some(1.)));
                } else {
                    app.bubble = format!("Found {} leftover{}, {} in all.", lefts.len(), plural_s(lefts.len()), format_bytes(left_bytes)).into();
                    app.dusty(cx, |d| d.st.set(if left_bytes >= 1024 * 1024 * 1024 { Mood::Wow } else { Mood::Happy }, Some(1.)));
                }
                app.tools.leftovers = Some(lefts);
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    pub(super) fn back_to_apps(&mut self, cx: &mut Context<Self>) {
        if let Some(j) = self.tools.left_job.take() {
            j.stop();
        }
        self.tools.app_result = None;
        self.tools.focus = None;
        self.tools.ring = false;
        if self.tools.apps.is_none() {
            self.start_apps(cx);
        } else {
            self.bubble = "Which one should go next?".into();
            self.dusty(cx, |d| d.st.set(Mood::Idle, None));
        }
        self.go(Screen::Apps, cx);
    }

    pub(super) fn uninstall_bytes(&self) -> u64 {
        let app = self.tools.app.as_ref().map(|a| a.bytes).unwrap_or(0);
        app + self.tools.leftovers.iter().flatten().filter(|l| self.tools.left_sel.contains(&l.path)).map(|l| l.bytes).sum::<u64>()
    }

    pub(super) fn can_uninstall(&self) -> bool {
        self.tools.app.as_ref().is_some_and(|a| a.blocked().is_none()) && self.tools.leftovers.is_some() && self.tools.left_job.is_none() && !self.tools.app_busy && self.tools.app_result.is_none()
    }

    pub(in crate::ui) fn start_uninstall(&mut self, cx: &mut Context<Self>) {
        if self.just_arrived() || self.confirm_closing.is_some() {
            return;
        }
        self.confirm = false;
        if !self.can_uninstall() {
            return;
        }
        let Some(a) = self.tools.app.clone() else { return };
        // in the order they're listed (biggest first)
        let lefts: Vec<PathBuf> = self.tools.leftovers.iter().flatten().filter(|l| self.tools.left_sel.contains(&l.path)).map(|l| l.path.clone()).collect();
        let dry = self.dry_run;
        self.tools.app_busy = true;
        self.tools.chomps = 1 + lefts.len().min(5) as u32;
        self.tools.next_chomp = Instant::now() + Duration::from_millis(200);
        self.bubble = if dry { "Practice run: I'll only pretend to eat it.".into() } else { format!("Om nom… {} goes to the Trash.", a.name).into() };
        self.dusty(cx, |d| d.st.set(Mood::Hungry, None));
        let task = cx.background_spawn(async move { uninstall::uninstall(&a, &lefts, dry) });
        cx.spawn(async move |this, cx| {
            let r = task.await;
            let _ = this.update(cx, |app, cx| app.uninstall_done(r, cx));
        })
        .detach();
        self.nav_t = Instant::now();
        cx.notify();
    }

    pub(super) fn uninstall_done(&mut self, r: UninstallReport, cx: &mut Context<Self>) {
        self.tools.app_busy = false;
        let name = self.tools.app.as_ref().map(|a| a.name.clone()).unwrap_or_default();
        let practice = r.report.entry.practice;
        if r.app_trashed {
            let path = self.tools.app.as_ref().map(|a| a.path.clone());
            if let Some(list) = self.tools.apps.as_mut() {
                list.retain(|a| Some(&a.path) != path.as_ref());
            }
            self.bubble = format!("Gone! {name} is napping in the Trash.").into();
            self.dusty(cx, |d| d.st.set(Mood::Happy, Some(2.)));
            self.burst_center(90, cx);
        } else if practice && r.report.errors.is_empty() {
            self.bubble = "Practice done! Nothing moved.".into();
            self.dusty(cx, |d| d.st.set(Mood::Happy, Some(1.5)));
        } else {
            self.tools.chomps = 0;
            self.bubble = if r.needs_admin { "That one needs an admin. I won't ask for your password.".into() } else { "Hm, that didn't go as planned.".into() };
            self.dusty(cx, |d| d.st.set(Mood::Sad, Some(1.5)));
        }
        self.tools.app_result = Some(r);
        self.nav_t = Instant::now();
        refresh_status_title();
        cx.notify();
    }

    pub(super) fn apps_screen(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        if self.tools.apps.is_none() {
            let (msg, found) = self.tools.apps_job.as_ref().map(|j| {
                let p = j.progress.lock().unwrap();
                (p.message.clone(), p.found)
            }).unwrap_or_default();
            return self.tool_busy("Uninstall apps", found.to_string(), "apps found so far".into(), if msg.is_empty() { "Looking for apps…".into() } else { msg }, cx).into_any_element();
        }
        let apps = self.filtered_apps();
        let total = self.tools.apps.as_ref().map(|a| a.len()).unwrap_or(0);
        let q = self.tools.search.clone();
        let search = div()
            .mt(px(sp::M))
            .h(px(44.))
            .px(px(sp::M + sp::XXS))
            .flex()
            .items_center()
            .gap(px(sp::S))
            .rounded(px(radius::L))
            .bg(rgb(pal().card))
            .border_2()
            .border_color(rgb(if q.is_empty() { pal().line } else { pal().grape.base }))
            .child(icon(Ico::Search, 20.))
            .child(
                div().flex_1().min_w(px(0.)).flex().items_center().text_size(px(ts::LEAD)).font_weight(FontWeight::BOLD)
                    .when(q.is_empty(), |d| d.text_color(rgb(pal().muted)).child(format!("Type to search {total} apps")))
                    .when(!q.is_empty(), |d| d.text_color(rgb(pal().ink)).child(div().one_line().child(q.clone())).child(div().ml(px(1.)).w(px(2.)).h(px(18.)).rounded_full().bg(rgb(pal().grape.base)))),
            )
            .when(!q.is_empty(), |d| {
                d.child(div().id("search-clear").size(px(HIT)).flex().items_center().justify_center().rounded_full().text_color(rgb(pal().muted)).font_weight(FontWeight::BLACK).cursor_pointer().hover(|s| s.bg(rgb(pal().line))).child("✕").on_click(cx.listener(|this, _, _, cx| {
                    this.tools.search.clear();
                    cx.notify();
                })))
            });
        let list = div()
            .id("apps-list")
            .flex_1()
            .min_h(px(0.))
            .overflow_y_scroll()
            .track_scroll(&self.tools.scroll)
            .px(px(GUTTER))
            .when(apps.is_empty(), |d| {
                d.child(div().mt(px(sp::XL)).flex().flex_col().items_center().gap(px(sp::XS)).child(icon(Ico::Search, 48.)).child(subtitle(if q.is_empty() { "No apps Dusty can remove here. Apple's own apps always stay.".to_string() } else { format!("No app called “{q}”.") }).text_center()))
            })
            .children(apps.into_iter().enumerate().map(|(i, a)| {
                let key = a.path.to_string_lossy().into_owned();
                let ringed = self.tool_ring(i);
                let blocked = a.running || !a.writable;
                let sub = if a.running {
                    "Open right now: quit it first".to_string()
                } else {
                    let mut s = a.version.clone().map(|v| format!("v{v}")).unwrap_or_default();
                    if let Some(t) = a.last_used {
                        if !s.is_empty() {
                            s.push_str(" · ");
                        }
                        s.push_str(&format!("opened {}", ago(secs_since(t))));
                    }
                    s
                };
                let open = a.clone();
                let row = div()
                    .id(ElementId::Name(format!("app-{key}").into()))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.tools.focus = Some(i);
                        this.tools.ring = false;
                        this.open_app(open.clone(), cx);
                    }))
                    .flex()
                    .items_center()
                    .gap(px(sp::M))
                    .px(px(sp::M))
                    .py(px(sp::S + sp::XXS))
                    .rounded(px(radius::L))
                    .border_2()
                    .border_b_4()
                    .border_color(rgb(pal().line))
                    .bg(rgb(pal().card))
                    .when(ringed, |d| d.shadow(focus_ring(pal().bg)))
                    .cursor_pointer()
                    .hover(|s| s.border_color(rgb(pal().sky.base)).bg(rgb(pal().card_hover)))
                    .active(|s| s.border_b_2().mt(px(2.)))
                    .on_mouse_down(MouseButton::Left, |_, _, _| haptic(Haptic::Tick))
                    .child(self.app_icon(&a, 36.))
                    .child(
                        div().flex_1().min_w(px(0.)).flex().flex_col().gap(px(sp::XXS))
                            .child(div().text_size(px(ts::LEAD)).font_weight(FontWeight::BLACK).one_line().child(a.name.clone()))
                            .when(!sub.is_empty(), |d| d.child(label_small(sub).one_line().when(a.running, |d| d.text_color(rgb(pal().warn))))),
                    )
                    .child(
                        div().flex_none().flex().flex_col().items_end().gap(px(sp::XXS))
                            .child(number(format_bytes(a.bytes), ts::LEAD, if blocked { pal().muted } else { pal().sky.text }))
                            .when(!a.writable, |d| d.child(small_chip("NEEDS ADMIN", pal().sky)))
                            .when(a.running && a.writable, |d| d.child(small_chip("OPEN", pal().sun))),
                    );
                div().mt(px(sp::S - 2. * RING_PAD)).py(px(RING_PAD)).child(row)
            }))
            .child(div().h(px(sp::XL)));
        div()
            .flex_1()
            .min_h(px(0.))
            .flex()
            .flex_col()
            .child(div().px(px(GUTTER)).pb(px(sp::S)).child(self.tool_header("Uninstall apps")).child(search))
            .child(list)
            .into_any_element()
    }

    pub(super) fn app_screen(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(a) = self.tools.app.clone() else { return div().into_any_element() };
        let p = pal();
        let total = self.uninstall_bytes();
        let n_sel = self.tools.left_sel.len();
        let result = self.tools.app_result.clone();
        let nav = self.nav;
        let mut list = div()
            .id("app-detail")
            .flex_1()
            .min_h(px(0.))
            .overflow_y_scroll()
            .px(px(GUTTER))
            .child(self.tool_header(a.name.clone()))
            .child(
                card().mt(px(sp::M)).p(px(CARD_PAD)).flex().items_center().gap(px(sp::M))
                    .child(self.app_icon(&a, 52.))
                    .child(
                        div().flex_1().min_w(px(0.)).flex().flex_col().gap(px(sp::XXS))
                            .child(div().flex().items_center().gap(px(sp::S)).child(div().text_size(px(ts::TITLE)).font_weight(FontWeight::BLACK).one_line().child(a.name.clone())).when(a.app_store, |d| d.child(small_chip("APP STORE", p.sky))))
                            .child(label_small(tildify(&a.path)).one_line())
                            .child(label_small([a.version.clone().map(|v| format!("Version {v}")), a.last_used.map(|t| format!("opened {}", ago(secs_since(t))))].into_iter().flatten().collect::<Vec<_>>().join(" · ")).one_line()),
                    )
                    .child(number(format_bytes(a.bytes), ts::TITLE + 1., p.sky.text)),
            );

        if let Some(r) = result {
            // ── after uninstalling ──
            let practice = r.report.entry.practice;
            let moved: u64 = r.report.entry.bytes;
            let n_left = r.report.entry.items.len().saturating_sub(1);
            if r.app_trashed || (practice && r.report.errors.is_empty()) {
                list = list.child(pop_in(
                    div().mt(px(sp::M)).flex().flex_col().items_center().gap(px(sp::XS))
                        .child(hero_icon(if practice { Ico::Practice } else { Ico::Uninstall }))
                        .child(div().text_size(px(ts::H2)).font_weight(FontWeight::BLACK).text_center().child(if practice { format!("Practice: {} would go", a.name) } else { format!("Uninstalled {}", a.name) }))
                        .child(subtitle(if n_left > 0 { format!("The app and {n_left} leftover{}.", plural_s(n_left)) } else { "Just the app: it left nothing behind.".to_string() }).text_center()),
                    ElementId::NamedInteger("un-head".into(), nav),
                    motion::STAGGER,
                ));
                list = list.child(pop_in(
                    self.trash_note(moved, practice, &self.tools.app_put_back, (!practice).then(|| r.report.entry.id.clone()), PutBackFor::Uninstall, cx).mt(px(sp::M)),
                    ElementId::NamedInteger("un-trash".into(), nav),
                    2 * motion::STAGGER,
                ));
            }
            if r.needs_admin {
                list = list.child(notice(p.sky, p.sky.text).mt(px(sp::M)).child(icon_label(Ico::Lock, ts::BODY, "This one needs an administrator").font_weight(FontWeight::BLACK)).child(div().text_color(rgb(p.ink)).font_weight(FontWeight::SEMIBOLD).child(format!("{} was installed for every user on this Mac. Dusty won't ask for your password: drag it to the Trash in Finder, and macOS will ask.", a.name))));
            }
            let errs: Vec<String> = r.report.errors.iter().filter(|_| !r.needs_admin).cloned().collect();
            if !errs.is_empty() {
                list = list.child(notice(p.sun, p.warn).mt(px(sp::M)).child(div().font_weight(FontWeight::BLACK).child(format!("{} thing{} stayed put", errs.len(), plural_s(errs.len())))).children(errs.iter().take(4).map(|e| div().child(e.clone()))));
            }
        } else {
            // ── before: blocked notices, then the leftovers to pick ──
            if a.running {
                list = list.child(
                    notice(p.sun, p.warn).mt(px(sp::M)).child(icon_label(Ico::Warning, ts::BODY, format!("{} is open", a.name)).font_weight(FontWeight::BLACK)).child(div().child("Quit it first (⌘Q in the app), then check again. Dusty never closes your apps for you."))
                        .child(div().flex().mt(px(sp::XS)).child(mini_button("recheck", if self.tools.left_job.is_some() { "Checking…" } else { "Check again" }, p.sun, self.tools.left_job.is_none()).on_click(cx.listener(|this, _, _, cx| this.check_app(cx))))),
                );
            } else if !a.writable {
                list = list.child(
                    notice(p.sky, p.sky.text).mt(px(sp::M)).child(icon_label(Ico::Lock, ts::BODY, "Needs an administrator").font_weight(FontWeight::BLACK))
                        .child(div().text_color(rgb(p.ink)).font_weight(FontWeight::SEMIBOLD).child(format!("{} was installed for every user on this Mac, so removing it needs an admin password. Dusty won't ask for it: drag the app to the Trash in Finder instead. Its leftovers below are still yours to clear afterwards.", a.name))),
                );
            }
            match &self.tools.leftovers {
                None => {
                    list = list.child(caption("Leftovers in your Library")).child(card().p(px(CARD_PAD)).flex().flex_col().gap(px(sp::S)).child(label_small("Sniffing ~/Library…")).child(busy_bar()));
                }
                Some(lefts) if lefts.is_empty() => {
                    list = list.child(caption("Leftovers in your Library")).child(card().p(px(CARD_PAD)).flex().items_center().gap(px(sp::M)).child(icon_tile(Ico::Sparkle, pal().sun, 40.)).child(subtitle("Nothing else to clean: this app keeps its things inside itself.")));
                }
                Some(lefts) => {
                    let ids: Vec<PathBuf> = lefts.iter().map(|l| l.path.clone()).collect();
                    let all_on = ids.iter().all(|i| self.tools.left_sel.contains(i));
                    list = list.child(
                        div().mt(px(sp::L)).mb(px(sp::XS)).flex().items_center().gap(px(sp::S))
                            .child(div().flex_1().child(label_small(format!("Leftovers in your Library · {}", lefts.len()))))
                            .child(text_link("left-all", if all_on { "Untick all" } else { "Tick all" }).on_click(cx.listener(move |this, _, _, cx| {
                                if ids.iter().all(|i| this.tools.left_sel.contains(i)) {
                                    this.tools.left_sel.clear();
                                } else {
                                    this.tools.left_sel = ids.iter().cloned().collect();
                                }
                                cx.notify();
                            }))),
                    );
                    list = list.children(lefts.iter().enumerate().map(|(i, l)| {
                        let on = self.tools.left_sel.contains(&l.path);
                        let t = p.sky;
                        let path = l.path.clone();
                        let rpath = l.path.clone();
                        let hid = format!("left-{}", l.path.to_string_lossy());
                        let hid2 = hid.clone();
                        let hovered = self.tools.hovered.as_deref() == Some(hid.as_str());
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
                            .cursor_pointer()
                            .hover(move |s| s.border_color(rgb(t.base)))
                            .active(|s| s.border_b_2().mt(px(2.)))
                            .on_mouse_down(MouseButton::Left, |_, _, _| haptic(Haptic::Tick))
                            .on_hover(cx.listener(move |this, on: &bool, _, cx| this.set_tool_hover(&hid2, *on, cx)))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                if !this.tools.left_sel.remove(&path) {
                                    this.tools.left_sel.insert(path.clone());
                                }
                                cx.notify();
                            }))
                            .child(checkbox(format!("lck-{i}").into(), on, t, 24., false))
                            .child(
                                div().flex_1().min_w(px(0.)).flex().flex_col().gap(px(sp::XXS))
                                    .child(div().text_size(px(ts::BODY)).font_weight(FontWeight::BLACK).one_line().child(l.kind.label()))
                                    .child(label_small(tildify(&l.path)).one_line()),
                            )
                            .when(hovered, |d| d.child(row_action(format!("lreveal-{i}"), Ico::Reveal, "Show in Finder", p.grape).on_click(cx.listener(move |this, _, _, cx| this.reveal_path(rpath.clone(), cx)))))
                            .child(
                                div().flex_none().flex().flex_col().items_end().gap(px(sp::XXS))
                                    .child(number(format_bytes(l.bytes), ts::BODY, if on { t.text } else { p.ink }))
                                    .when(!l.preselected, |d| d.child(small_chip("MAY HOLD DATA", p.sun))),
                            );
                        div().mt(px(sp::S)).child(row)
                    }));
                }
            }
        }
        list = list.child(div().h(px(sp::XL)));

        let footer_el = if self.tools.app_result.is_some() {
            footer(p.bg).child(button("app-done", "Done", Btn::Primary(p.mint), true).on_click(cx.listener(|this, _, _, cx| this.back_to_apps(cx))))
        } else {
            let label = if self.tools.app_busy {
                "Uninstalling…".to_string()
            } else if a.running {
                format!("Quit {} first", a.name)
            } else if !a.writable {
                "Needs an administrator".to_string()
            } else if self.dry_run {
                format!("Practice uninstall · {}", format_bytes(total))
            } else {
                format!("Uninstall · {}", format_bytes(total))
            };
            let path = a.path.clone();
            footer(p.bg)
                .child(div().flex().justify_between().child(label_small(format!("The app + {n_sel} leftover{}", plural_s(n_sel)))).child(label_small("Everything goes to the Trash")))
                .child(button("app-go", label, Btn::Primary(p.coral), self.can_uninstall()).on_click(cx.listener(|this, _, _, cx| {
                    // a disabled button still gets clicks: check again
                    if this.can_uninstall() {
                        this.open_sheet(ConfirmKind::Uninstall, cx)
                    }
                })))
                .child(button("app-reveal", "Show in Finder", Btn::Ghost, true).on_click(cx.listener(move |this, _, _, cx| this.reveal_path(path.clone(), cx))))
        };
        div().flex_1().min_h(px(0.)).flex().flex_col().child(list).child(footer_el).into_any_element()
    }
}
