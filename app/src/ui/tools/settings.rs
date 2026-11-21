//! Settings: the weekly clean and your ignore list.

use super::*;

impl DustyApp {
    pub(super) fn load_settings(&mut self, cx: &mut Context<Self>) {
        self.bubble = "Little knobs. Nothing here deletes anything.".into();
        self.dusty(cx, |d| d.st.set(Mood::Idle, None));
        self.tools.ignore = Some(dusty_core::ignore_entries());
        let task = cx.background_spawn(async move { schedule::is_scheduled() });
        cx.spawn(async move |this, cx| {
            let on = task.await;
            let _ = this.update(cx, |app, cx| {
                app.tools.scheduled = Some(on);
                cx.notify();
            });
        })
        .detach();
    }

    pub(super) fn toggle_schedule(&mut self, cx: &mut Context<Self>) {
        let Some(on) = self.tools.scheduled else { return };
        if self.tools.schedule_busy {
            return;
        }
        self.tools.schedule_busy = true;
        self.tools.schedule_note = None;
        let task = cx.background_spawn(async move {
            if on {
                schedule::remove_schedule().map(|_| false)
            } else {
                schedule::install_schedule(&Schedule::default()).map(|_| true)
            }
        });
        cx.spawn(async move |this, cx| {
            let r = task.await;
            let _ = this.update(cx, |app, cx| {
                app.tools.schedule_busy = false;
                match r {
                    Ok(now_on) => {
                        app.tools.scheduled = Some(now_on);
                        app.bubble = if now_on { "Sundays at 11 it is. I'll only take the safe stuff.".into() } else { "Okay, no more Sunday snacks.".into() };
                        app.dusty(cx, |d| d.st.set(if now_on { Mood::Happy } else { Mood::Idle }, Some(1.)));
                        haptic(Haptic::Tick);
                    }
                    Err(e) => {
                        app.tools.schedule_note = Some(e);
                        app.dusty(cx, |d| d.st.set(Mood::Sad, Some(1.)));
                    }
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    pub(super) fn unignore(&mut self, entry: String, cx: &mut Context<Self>) {
        match dusty_core::remove_from_ignore(&entry) {
            Ok(_) => {
                haptic(Haptic::Tick);
                self.toast("Off the list. Dusty may suggest it again.");
                self.bubble = "Back on the menu, then!".into();
            }
            Err(e) => self.toast(&e),
        }
        self.tools.ignore = Some(dusty_core::ignore_entries());
        cx.notify();
    }

    pub(super) fn settings_screen(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let p = pal();
        let sched = self.tools.scheduled;
        let sched_on = sched == Some(true);
        let menu_on = platform::menu_bar_enabled();
        let dry = self.dry_run;
        let toggle_row = |id: &'static str, ico: Ico, title: &'static str, on: bool, t: Tone, enabled: bool| {
            div()
                .id(id)
                .flex()
                .items_center()
                .gap(px(sp::M))
                .when(enabled, |d| d.cursor_pointer().on_mouse_down(MouseButton::Left, |_, _, _| haptic(Haptic::Tick)))
                .when(!enabled, |d| d.opacity(0.6))
                .child(icon_label(ico, ts::LEAD, title).flex_1().min_w(px(0.)).text_size(px(ts::LEAD)).font_weight(FontWeight::BLACK))
                .child(switch(id, on, t))
        };
        let exe = std::env::current_exe().ok().map(|e| tildify(&e)).unwrap_or_default();
        let in_apps = exe.starts_with("/Applications/") || exe.starts_with("~/Applications/");
        let ignore = self.tools.ignore.clone().unwrap_or_else(|| Ok(vec![]));
        let nav = self.nav;
        let list = div()
            .id("settings-list")
            .flex_1()
            .min_h(px(0.))
            .overflow_y_scroll()
            .px(px(GUTTER))
            .child(self.tool_header("Settings"))
            // weekly clean
            .child(pop_in(
                card().mt(px(sp::M)).p(px(CARD_PAD)).flex().flex_col().gap(px(sp::S))
                    .child(toggle_row("sched", Ico::Schedule, "Weekly safe clean", sched_on, p.mint, sched.is_some() && !self.tools.schedule_busy).on_click(cx.listener(|this, _, _, cx| this.toggle_schedule(cx))))
                    .child(subtitle("Every Sunday at 11:00, or as soon as your Mac wakes up. Safe picks only: never anything you'd have to re-download, and build folders you touched this week stay. Caches are deleted, the rest goes to the Trash. Each run shows up in History."))
                    .child(label_small(if self.tools.schedule_busy { "Talking to launchd…".to_string() } else if sched_on { "On · runs while Dusty is closed, too".to_string() } else if sched.is_none() { "Checking…".to_string() } else { "Off".to_string() }).font(tabular(FontWeight::BOLD)))
                    .when(!in_apps, |d| d.child(label_small(format!("It runs Dusty from {exe}. Move Dusty to Applications first, or turn it off and on again after moving it."))))
                    .when_some(self.tools.schedule_note.clone(), |d, e| d.child(notice(p.sun, p.warn).child(e))),
                ElementId::NamedInteger("set-sched".into(), nav),
                motion::STAGGER,
            ))
            // menu bar + practice
            .child(pop_in(
                card().mt(px(sp::M)).p(px(CARD_PAD)).flex().flex_col().gap(px(sp::M))
                    .child(toggle_row("menubar", Ico::Paw, "Free space in the menu bar", menu_on, p.grape, true).on_click(cx.listener(|_, _, window, cx| {
                        // through the window: the app-level dispatch can't reach it while this click holds it
                        window.dispatch_action(Box::new(crate::ToggleMenuBar), cx);
                        cx.notify();
                    })))
                    .child(div().h(px(2.)).bg(rgb(p.line)))
                    .child(toggle_row("practice", Ico::Practice, "Practice run", dry, p.sky, true).on_click(cx.listener(|this, _, _, cx| {
                        this.dry_run = !this.dry_run;
                        this.bubble = if this.dry_run { "Practice run: I'll only pretend, everywhere.".into() } else { "For real this time!".into() };
                        cx.notify();
                    })))
                    .child(label_small("While it's on, cleaning, uninstalling and moving files only rehearse. Nothing is deleted or moved.")),
                ElementId::NamedInteger("set-misc".into(), nav),
                2 * motion::STAGGER,
            ))
            // ignore list
            .child(pop_in(
                card().mt(px(sp::M)).p(px(CARD_PAD)).flex().flex_col().gap(px(sp::S))
                    .child(div().flex().items_center().gap(px(sp::S)).child(icon_label(Ico::Never, ts::LEAD, "Never clean").flex_1().text_size(px(ts::LEAD)).font_weight(FontWeight::BLACK)).child(text_link("ignore-file", "Show file").on_click(cx.listener(|this, _, _, cx| {
                        let f = dusty_core::ignore_file();
                        if !platform::reveal_in_finder(&[f], cx) {
                            this.toast("There's no ignore file yet. “Never clean this” on a row makes one.");
                        }
                    }))))
                    .child(label_small(format!("Dusty skips these everywhere ({}).", tildify(&dusty_core::ignore_file()))))
                    .map(|d| match ignore {
                        Err(e) => d.child(notice(p.sun, p.warn).child(e)),
                        Ok(list) if list.is_empty() => d.child(subtitle("Nothing on it yet. Hover an item on Results and tap “Never clean this” to add one.")),
                        Ok(list) => d.children(list.into_iter().enumerate().map(|(i, e)| {
                            let entry = e.clone();
                            div().flex().items_center().gap(px(sp::S)).py(px(sp::XS)).when(i > 0, |d| d.border_t_1().border_color(rgb(p.line)))
                                .child({
                                    let (ico, text) = ignore_display(&e);
                                    div().flex_1().min_w(px(0.)).flex().items_center().gap(px(sp::XS + 2.)).when_some(ico, |d, i| d.child(icon(i, 16.)))
                                        .child(div().flex_1().min_w(px(0.)).text_size(px(ts::SMALL)).font_family("Menlo").text_color(rgb(p.ink)).one_line().child(text))
                                })
                                .child(row_action_text(format!("unignore-{i}"), "✕", "Take it off the list", p.coral).on_click(cx.listener(move |this, _, _, cx| this.unignore(entry.clone(), cx))))
                        })),
                    }),
                ElementId::NamedInteger("set-ignore".into(), nav),
                3 * motion::STAGGER,
            ))
            .child(div().h(px(sp::XL)));
        list
    }
}
