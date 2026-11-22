//! Look further: an AI agent you pick (with your own subscription) explains the big folders a checkup doesn't
//! know, you pick, picks go to the Trash.
//!
//! The engine is `dusty_ai::explore` (what the agent may see, and the checks each suggestion passes, live in
//! `dusty_core::explore`). This screen lets you pick the agent and shows the account it uses, asks before
//! anything is sent, shows each suggestion with the agent's reasons, ticks nothing, and offers to remember what
//! you tossed so checkups find it by themselves.

use super::*;
use dusty_ai::{Agent, Availability, ExploreProgress, Stage, AGENTS};
use dusty_core::explore::{self, Risk, Suggestion};

impl DustyApp {
    pub(super) fn start_explore(&mut self, cx: &mut Context<Self>) {
        if let Some(j) = self.tools.explore_job.take() {
            j.stop();
        }
        let job: Job<ExploreProgress> = Job::new();
        let (progress, cancel) = (job.progress.clone(), job.cancel.clone());
        self.tools.explore_job = Some(job);
        self.tools.explore = None;
        self.tools.explore_note = None;
        self.tools.explore_warn = None;
        self.tools.explore_used = None;
        self.tools.explore_sel.clear();
        self.tools.explore_result = None;
        self.tools.explore_put_back = PutBack::No;
        self.tools.explore_remembered = false;
        self.tools.focus = None;
        self.bubble = "This takes about a minute. Stop me any time.".into();
        self.dusty(cx, |d| d.st.set(Mood::Think, None));
        // the checkup on screen says what's handled already; without one, the trip runs its own
        let known = (!self.targets.is_empty()).then(|| self.targets.clone());
        let agent = self.agent();
        let flag = cancel.clone();
        let task = cx.background_spawn(async move { dusty_ai::explore(agent, known, &progress, &cancel) });
        cx.spawn(async move |this, cx| {
            let r = task.await;
            let _ = this.update(cx, |app, cx| {
                if !app.tools.explore_job.as_ref().is_some_and(|j| j.is(&flag)) {
                    return;
                }
                app.tools.explore_job = None;
                let total: u64 = r.suggestions.iter().map(|s| s.bytes).sum();
                match &r.error {
                    Some(e) if r.suggestions.is_empty() => {
                        app.bubble = e.message(agent).into();
                        app.dusty(cx, |d| d.st.set(Mood::Sad, Some(1.5)));
                    }
                    _ if r.suggestions.is_empty() => {
                        app.bubble = if r.asked { format!("{} found nothing else safe to toss. Tidy!", agent.name).into() } else { "Nothing new since last time. Tidy!".into() };
                        app.dusty(cx, |d| d.st.set(Mood::Happy, Some(1.5)));
                    }
                    _ => {
                        app.bubble = format!("{} more to look at, {} in all. Nothing is ticked: you pick.", r.suggestions.len(), format_bytes(total)).into();
                        app.dusty(cx, |d| d.st.set(if total >= WOW_FILE { Mood::Wow } else { Mood::Happy }, Some(1.)));
                    }
                }
                app.tools.explore_note = r.note.clone();
                app.tools.explore_warn = r.error.as_ref().filter(|_| !r.suggestions.is_empty()).map(|e| e.message(agent));
                app.tools.explore_error = r.error.as_ref().filter(|_| r.suggestions.is_empty()).map(|e| e.message(agent));
                app.tools.explore_used = r.asked.then(|| used_line(agent, &r));
                if let Some(a) = &r.account {
                    app.tools.agent_prefs.accounts.insert(agent.id.to_string(), a.clone());
                }
                // in sections (safe first), biggest first in each: keys and the screen walk the same order
                let mut list = r.suggestions;
                list.sort_by_key(|s| (kind(s).rank, std::cmp::Reverse(s.bytes)));
                if std::mem::take(&mut app.tools.explore_toss) {
                    app.tools.explore_sel = list.iter().filter(|s| kind(s).rank == 0).map(|s| s.path.clone()).collect();
                }
                app.tools.explore = Some(Rc::new(list));
                if !app.tools.explore_sel.is_empty() {
                    app.start_trash_suggestions(cx);
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    /// A Look further trip is under way.
    pub(in crate::ui) fn explore_running(&self) -> bool {
        self.tools.explore_job.is_some()
    }

    /// The agent picked for Look further.
    fn agent(&self) -> &'static Agent {
        self.tools.agent_prefs.agent()
    }

    /// Checks which agents can start here, in the background (it asks the login shell for PATH).
    pub(super) fn check_agents(&mut self, cx: &mut Context<Self>) {
        let task = cx.background_spawn(async { AGENTS.iter().map(|a| a.availability()).collect::<Vec<_>>() });
        cx.spawn(async move |this, cx| {
            let ready = task.await;
            let _ = this.update(cx, |app, cx| {
                app.tools.agents_ready = Some(ready);
                cx.notify();
            });
        })
        .detach();
    }

    fn pick_agent(&mut self, id: &'static str, cx: &mut Context<Self>) {
        self.tools.agent_prefs.selected = Some(id.to_string());
        if let Err(e) = self.tools.agent_prefs.save() {
            self.toast(&format!("Couldn't save your pick ({e})"));
        }
        let a = self.agent();
        self.tools.explore_error = None;
        self.bubble = match self.tools.agent_prefs.accounts.get(a.id) {
            Some(acc) => format!("{} it is, on your {acc}.", a.name).into(),
            None => format!("{} it is, with your {}.", a.name, a.plan).into(),
        };
        self.dusty(cx, |d| d.st.poke(0.));
        cx.notify();
    }

    /// Stops a trip and goes back to the picker (what passed the checks so far is dropped with it).
    pub(super) fn stop_explore(&mut self, cx: &mut Context<Self>) {
        if let Some(j) = self.tools.explore_job.take() {
            j.stop();
        }
        self.bubble = "Stopped. Pick who to ask.".into();
        self.dusty(cx, |d| d.st.set(Mood::Idle, None));
        cx.notify();
    }

    /// Back to the picker, to ask someone else.
    fn ask_another(&mut self, cx: &mut Context<Self>) {
        self.tools.explore = None;
        self.tools.explore_result = None;
        self.tools.explore_sel.clear();
        self.bubble = "Who should I ask?".into();
        cx.notify();
    }

    fn explore_items(&self) -> impl Iterator<Item = &Suggestion> {
        self.tools.explore.iter().flat_map(|l| l.iter())
    }

    pub(super) fn toggle_suggestion(&mut self, p: &PathBuf, cx: &mut Context<Self>) {
        if !self.tools.explore_sel.remove(p) {
            self.tools.explore_sel.insert(p.clone());
            let s = self.explore_items().find(|s| &s.path == p).cloned();
            if let Some(s) = s {
                self.bubble = if s.risk == Risk::Medium { format!("Okay. Remember: {}", lower_first(&s.comes_back)).into() } else { format!("{}? Into the bowl it goes.", format_bytes(s.bytes)).into() };
                self.dusty(cx, |d| d.st.poke(0.));
            }
        } else {
            self.bubble = "Okay, that one stays.".into();
        }
        cx.notify();
    }

    pub(super) fn explore_sel_bytes(&self) -> u64 {
        self.explore_items().filter(|s| self.tools.explore_sel.contains(&s.path)).map(|s| s.bytes).sum()
    }

    pub(in crate::ui) fn start_trash_suggestions(&mut self, cx: &mut Context<Self>) {
        if self.just_arrived() || self.confirm_closing.is_some() {
            return;
        }
        self.confirm = false;
        let items: Vec<Suggestion> = self.explore_items().filter(|s| self.tools.explore_sel.contains(&s.path)).cloned().collect();
        if items.is_empty() || self.tools.explore_busy {
            return;
        }
        let dry = self.dry_run;
        self.tools.explore_busy = true;
        self.tools.chomps = items.len().min(6) as u32;
        self.tools.next_chomp = Instant::now() + Duration::from_millis(200);
        self.bubble = if dry { "Practice run: pretend-chomping…".into() } else { "Om nom nom. Off to the Trash!".into() };
        self.dusty(cx, |d| d.st.set(Mood::Hungry, None));
        let task = cx.background_spawn(async move {
            let r = explore::trash(&items, dry);
            (r, items)
        });
        cx.spawn(async move |this, cx| {
            let (r, items) = task.await;
            let _ = this.update(cx, |app, cx| {
                app.tools.explore_busy = false;
                let moved: HashSet<PathBuf> = r.entry.items.iter().flat_map(|i| &i.paths).filter(|p| p.action == dusty_core::history::PathAction::Trashed).map(|p| p.path.clone()).collect();
                if let Some(list) = app.tools.explore.as_mut().map(Rc::make_mut) {
                    list.retain(|s| !moved.contains(&s.path));
                }
                app.tools.explore_sel.retain(|p| !moved.contains(p));
                app.tools.explore_tossed = items.into_iter().filter(|s| moved.contains(&s.path)).collect();
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
                app.tools.explore_put_back = PutBack::No;
                app.tools.explore_remembered = false;
                app.tools.explore_result = Some(r);
                app.nav_t = Instant::now();
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    /// "Look for these in checkups too": adds what you just tossed to your catalog.
    fn remember_tossed(&mut self, cx: &mut Context<Self>) {
        match explore::remember(&self.tools.explore_tossed) {
            Ok(_) => {
                self.tools.explore_remembered = true;
                self.toast_with(Ico::Sparkle, "Got it. Checkups will look for these from the next launch, unticked.");
                self.dusty(cx, |d| d.st.set(Mood::Happy, Some(1.)));
            }
            Err(e) => self.toast(&e),
        }
        cx.notify();
    }

    /// Under the checkup's results: the way to ask about what it didn't know.
    pub(in crate::ui) fn explore_card(&self, cx: &mut Context<Self>) -> Stateful<Div> {
        let p = pal();
        let wide = self.is_wide();
        card()
            .id("explore-card")
            .mt(px(if wide { sp::M } else { sp::S }))
            .p(px(if wide { CARD_PAD } else { sp::M }))
            .when(wide, |d| d.min_h(px(88.)))
            .flex()
            .items_center()
            .gap(px(sp::M))
            .cursor_pointer()
            // tinted grape: an action to take, not another kind of mess
            .bg(rgb(p.grape.soft))
            .border_color(rgb(p.grape.base))
            .hover(move |s| s.border_color(rgb(p.grape.edge)))
            .active(move |s| s.border_b_2().mt(px(if wide { sp::M } else { sp::S } + 2.)))
            .on_mouse_down(MouseButton::Left, |_, _, _| haptic(Haptic::Tick))
            .on_click(cx.listener(|this, _, _, cx| this.open_tool(Screen::Explore, cx)))
            .child(agent_tile(self.agent(), if wide { 44. } else { 40. }))
            .child(
                div().flex_1().min_w(px(0.)).flex().flex_col().gap(px(sp::XXS))
                    .child(div().text_size(px(ts::TITLE)).line_height(px(ts::TITLE * 1.25)).font_weight(FontWeight::BLACK).child(format!("Look further with {}", self.agent().name)))
                    .child(label_small("The big folders I don't know yet, explained. Nothing moves unless you pick it.")),
            )
            .child(div().w(px(sp::L)).flex_none().flex().justify_center().text_color(rgb(p.grape.text)).font_weight(FontWeight::BLACK).text_size(px(ts::LEAD)).child("▸"))
    }

    /// The agents to pick from: who, how it's locked, and whose subscription it spends. The ones that can't
    /// start here wait in a quieter list under them, each with how to get it.
    fn agent_picker(&self, cx: &mut Context<Self>) -> Div {
        let p = pal();
        let picked = self.agent().id;
        let ready = |i: usize| self.tools.agents_ready.as_ref().map(|r| r[i].clone());
        let (usable, missing): (Vec<usize>, Vec<usize>) = (0..AGENTS.len()).partition(|&i| !matches!(ready(i), Some(Availability::Missing(_))));
        let row = |i: usize, cx: &mut Context<Self>| {
            let a = &AGENTS[i];
            let on = a.id == picked;
            let status = match ready(i) {
                None => "Checking…".to_string(),
                Some(Availability::Missing(why)) => why,
                Some(Availability::Ready) => format!("Uses your {}", self.plan_of(a)),
            };
            let id = a.id;
            div()
                .id(("agent", i))
                .px(px(sp::S))
                .py(px(sp::S))
                .flex()
                .items_center()
                .gap(px(sp::M))
                .rounded(px(radius::L))
                .border_2()
                .border_color(rgb(if on { p.grape.base } else { p.card }))
                .when(on, |d| d.bg(rgb(p.grape.soft)))
                .when(!on, |d| d.cursor_pointer().hover(|s| s.bg(rgb(pal().card_hover))))
                .on_mouse_down(MouseButton::Left, |_, _, _| haptic(Haptic::Tick))
                .on_click(cx.listener(move |this, _, _, cx| this.pick_agent(id, cx)))
                .child(checkbox(format!("agent-ck-{i}").into(), on, p.grape, 22., true).when(!on, |d| d.border_color(rgb(if is_dark() { p.muted } else { p.line }))))
                .child(agent_tile(a, 36.))
                .child(
                    div().flex_1().min_w(px(0.)).flex().flex_col().gap(px(sp::XXS))
                        .child(div().flex().items_center().gap(px(sp::S)).child(div().text_size(px(ts::BODY)).font_weight(FontWeight::BLACK).child(a.name)).child(small_chip(a.lock.chip(), if a.lock == dusty_ai::Lock::NoTools { p.mint } else { p.sun }).when(on, |d| d.bg(rgb(p.card)))))
                        .child(label_small(status))
                        // the picked one says its promise right there, so it's seen before the button
                        .when(on, |d| d.child(div().mt(px(sp::XXS)).child(icon_label(Ico::Lock, ts::SMALL, a.lock.short()).text_size(px(ts::SMALL)).font_weight(FontWeight::BOLD).text_color(rgb(p.grape.text))))),
                )
        };
        let rows: Vec<_> = usable.iter().map(|&i| row(i, cx)).collect();
        card().p(px(sp::S)).flex().flex_col().gap(px(sp::XXS))
            .child(label_small("Ask with").px(px(sp::S)).pt(px(sp::XS)))
            .children(rows)
            .when(!missing.is_empty(), |d| {
                d.child(div().mx(px(sp::S)).mt(px(sp::S)).pt(px(sp::S)).border_t_2().border_color(rgb(p.line)).child(label_small("Not set up on this Mac")))
                    .children(missing.iter().map(|&i| {
                        let a = &AGENTS[i];
                        let why = match ready(i) { Some(Availability::Missing(w)) => w, _ => String::new() };
                        div().px(px(sp::S)).py(px(sp::XS)).flex().items_center().gap(px(sp::M))
                            .child(div().w(px(22.)).flex_none())
                            .child(div().w(px(36.)).flex_none().flex().justify_center().opacity(if is_dark() { 0.4 } else { 0.6 }).child(agent_tile(a, 28.)))
                            .child(div().flex_1().min_w(px(0.)).flex().items_baseline().gap(px(sp::S))
                                .child(div().flex_none().text_size(px(ts::SMALL)).font_weight(FontWeight::BLACK).text_color(rgb(p.muted)).child(a.name))
                                .child(fill_line(label_small(upper_first(why.trim_start_matches("Not installed: "))).text_color(rgb(p.grape.text)))))
                    }))
            })
    }

    /// The plan an agent spends: the account it reported last time, or what it needs.
    fn plan_of(&self, a: &Agent) -> String {
        self.tools.agent_prefs.accounts.get(a.id).map(|acc| format!("{acc} plan")).unwrap_or_else(|| a.plan.to_string())
    }

    /// Before anything is sent: who to ask, what they'll see, and the button that starts the trip.
    fn explore_intro(&self, cx: &mut Context<Self>) -> Div {
        let p = pal();
        let point = |i: Ico, s: String| div().flex().items_start().gap(px(sp::S)).child(div().pt(px(1.)).child(inline_icon(i, ts::BODY))).child(div().flex_1().min_w(px(0.)).text_size(px(ts::BODY)).line_height(px(ts::BODY * 1.4)).font_weight(FontWeight::SEMIBOLD).child(s));
        let error = self.tools.explore_error.clone();
        let a = self.agent();
        let list = div().id("explore-intro").flex_1().min_h(px(0.)).overflow_y_scroll().px(px(GUTTER)).child(
            div().flex().flex_col().gap(px(sp::M)).mt(px(sp::S)).pb(px(sp::M))
                .when_some(error, |d, e| d.child(notice(p.sun, p.warn).child(icon_label(Ico::Warning, ts::BODY, e).items_start())))
                .child(self.agent_picker(cx))
                // the promises that matter most come first: it can't touch anything, and nothing goes for good
                .child(
                    card().p(px(CARD_PAD)).flex().flex_col().gap(px(sp::M))
                        .child(point(Ico::Trash, "Nothing is ticked. What you pick goes to the Trash, so you can put it back.".into()))
                        // what it sees is said above the button; read-only agents also get the advice that comes with it
                        .when(a.lock == dusty_ai::Lock::ReadOnly, |d| d.child(point(Ico::Search, a.lock.sees(a.name))))
                        .child(point(Ico::Check, "I check every suggestion myself: nothing personal, no code, no app that's still using it.".into()))
                        .child(label_small("A trip takes about a minute.")),
                ),
        );
        div()
            .flex_1()
            .min_h(px(0.))
            .flex()
            .flex_col()
            .child(div().px(px(GUTTER)).pb(px(sp::S)).child(self.tool_header("Look further")))
            .child(fade_top(list, p.bg))
            .child(footer(p.bg).child(label_small(match a.lock { dusty_ai::Lock::NoTools => format!("{} sees names, sizes and ages. Never what's inside a file.", a.name), dusty_ai::Lock::ReadOnly => format!("{} sees names, sizes and ages, and may peek inside files.", a.name) }).text_center()).child(button("explore-go", if self.tools.explore_error.is_some() { format!("Try again with {}", a.name) } else { format!("Ask {}", a.name) }, Btn::Primary(p.grape), true).on_click(cx.listener(|this, _, _, cx| this.start_explore(cx)))))
    }

    /// While a trip runs: the steps it goes through (each one says what it did), what the agent is reading,
    /// and what it can't do.
    fn explore_busy_view(&mut self, cx: &mut Context<Self>) -> Div {
        let p = pal();
        let Some(j) = &self.tools.explore_job else { return div() };
        // only this trip's finds: earlier ones were checked on their own trip
        let (stage, n, folders, folder_bytes, biggest, bytes, msg) = {
            let pr = j.progress.lock().unwrap();
            let new = &pr.found[pr.earlier.min(pr.found.len())..];
            (pr.stage, new.len(), pr.folders, pr.folder_bytes, pr.biggest.clone(), new.iter().map(|s| s.bytes).sum::<u64>(), pr.message.clone())
        };
        let a = self.agent();
        let measured = folders > 0 || stage > Stage::Measure;
        let steps = [
            (
                if measured { Step::Done } else { Step::Now },
                "Measure the big folders I don't know".to_string(),
                if measured { format!("{folders} folder{}, {} in all", plural_s(folders), format_bytes(folder_bytes)) } else { "Library and hidden folders".into() },
            ),
            (
                match stage { Stage::Measure => Step::Later, Stage::Wake => Step::Now, Stage::Read => Step::Done },
                format!("Wake up {}", a.name),
                format!("On your {}", self.plan_of(a)),
            ),
            (
                if stage == Stage::Read { Step::Now } else { Step::Later },
                format!("{} looks them over", a.name),
                // the live line sits with the step it belongs to
                if stage == Stage::Read && !msg.is_empty() { msg.trim_end_matches('…').to_string() } else { "Then answers with what's safe to toss, and why".into() },
            ),
            (
                if stage == Stage::Read && n > 0 { Step::Now } else { Step::Later },
                "I check every suggestion".to_string(),
                if n > 0 { format!("{n} passed so far, {} in all", format_bytes(bytes)) } else { "Nothing personal, no code, no app in use".into() },
            ),
        ];
        let step_list = card().w_full().mt(px(sp::M)).p(px(CARD_PAD)).flex().flex_col().gap(px(sp::M))
            .children(steps.into_iter().enumerate().map(|(i, (st, title, detail))| step_row(i, st, title, detail)));
        let biggest_card = (!biggest.is_empty()).then(|| {
            card().w_full().p(px(CARD_PAD)).flex().flex_col().gap(px(sp::XS))
                .child(label_small(format!("The biggest of what {} gets to see", a.name)))
                .children(biggest.iter().take(if self.is_wide() { 8 } else { 5 }).map(|(path, b, days)| {
                    div().flex().items_center().gap(px(sp::S))
                        .child(div().w(px(72.)).flex_none().text_size(px(ts::SMALL)).font(tabular(FontWeight::BLACK)).child(format_bytes(*b)))
                        .child(div().flex_none().max_w(relative(0.7)).text_size(px(ts::SMALL)).font_weight(FontWeight::BOLD).one_line().child(path.clone()))
                        .child(fill_line(label_small(format!("· {}", lower_first(&age(*days))))))
                }))
        });
        let list = div().id("explore-busy").flex_1().min_h(px(0.)).overflow_y_scroll().px(px(GUTTER)).child(
            div().flex().flex_col().gap(px(sp::M)).pb(px(sp::M))
                .child(step_list)
                .child(notice(p.grape, p.grape.text).child(icon_label(Ico::Lock, ts::BODY, a.lock.promise(a.name)).items_start()))
                .children(biggest_card),
        );
        div()
            .flex_1()
            .min_h(px(0.))
            .flex()
            .flex_col()
            .child(div().px(px(GUTTER)).child(self.tool_header(format!("Asking {}", a.name))))
            .child(fade_top(list, p.bg))
            .child(
                footer(p.bg)
                    .child(label_small("Nothing moves until you pick, and picks go to the Trash.").text_center())
                    // the same as Esc: each tool decides where Stop leaves you
                    .child(button("tool-stop", "Stop", Btn::Ghost, true).on_click(cx.listener(|this, _, _, cx| this.tool_back(cx)))),
            )
    }

    /// "REBUILDS ITSELF  5 · 2.9 GB   Tick all": the start of a section of the list.
    fn section_head(&self, items: &Rc<Vec<Suggestion>>, from: usize, k: &Kind, cx: &mut Context<Self>) -> Div {
        let p = pal();
        let rank = k.rank;
        let paths: Vec<PathBuf> = items[from..].iter().take_while(|s| kind(s).rank == rank).map(|s| s.path.clone()).collect();
        let bytes: u64 = items[from..].iter().take_while(|s| kind(s).rank == rank).map(|s| s.bytes).sum();
        let all_on = paths.iter().all(|p| self.tools.explore_sel.contains(p));
        let n = paths.len();
        div().mt(px(if from == 0 { sp::S } else { sp::L })).flex().flex_col().gap(px(sp::XXS))
            .child(
                div().flex().items_center().gap(px(sp::S)).min_h(px(HIT))
                    .child(small_chip(k.chip, k.tone))
                    .child(div().text_size(px(ts::SMALL)).font(tabular(FontWeight::BLACK)).text_color(rgb(p.muted)).child(format!("{n} · {}", format_bytes(bytes))))
                    .child(div().flex_1())
                    // only the safe section gets a shortcut: the others are one-by-one decisions
                    .when(rank == 0 && n > 1, |d| {
                        d.child(
                            div().id(("explore-tick-all", rank as usize)).h(px(HIT)).px(px(sp::S)).flex().items_center().rounded(px(radius::S))
                                .text_size(px(ts::SMALL)).font_weight(FontWeight::BLACK).text_color(rgb(p.grape.text)).cursor_pointer()
                                .hover(move |s| s.bg(rgb(p.grape.soft)))
                                .on_mouse_down(MouseButton::Left, |_, _, _| haptic(Haptic::Tick))
                                .on_click(cx.listener(move |this, _, _, cx| this.tick_all(&paths, !all_on, cx)))
                                .child(if all_on { "Untick all".to_string() } else { format!("Tick all {n}") }),
                        )
                    }),
            )
            .child(label_small(k.blurb))
    }

    fn tick_all(&mut self, paths: &[PathBuf], on: bool, cx: &mut Context<Self>) {
        for p in paths {
            if on {
                self.tools.explore_sel.insert(p.clone());
            } else {
                self.tools.explore_sel.remove(p);
            }
        }
        self.bubble = if on { "All the ones that grow back. Good pick.".into() } else { "Okay, none of those.".into() };
        self.dusty(cx, |d| d.st.poke(0.));
        cx.notify();
    }

    pub(super) fn explore_screen(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let p = pal();
        if self.tools.explore_job.is_some() {
            return self.explore_busy_view(cx).into_any_element();
        }
        let Some(items) = self.tools.explore.clone() else {
            return self.explore_intro(cx).into_any_element();
        };
        if items.is_empty() && self.tools.explore_result.is_none() {
            return self.explore_intro(cx).into_any_element();
        }
        let n_sel = self.tools.explore_sel.len();
        let sel_bytes = self.explore_sel_bytes();
        let nav = self.nav;
        let result = self.tools.explore_result.clone();
        let can_remember = !self.tools.explore_tossed.is_empty() && !self.tools.explore_remembered && result.as_ref().is_some_and(|r| !r.entry.practice);
        let banner = div()
            .px(px(GUTTER))
            .pb(px(sp::S))
            .child(self.tool_header("Look further"))
            .when_some(result, |d, r| {
                let practice = r.entry.practice;
                d.child(pop_in(
                    div().mt(px(sp::M)).flex().flex_col().gap(px(sp::S))
                        .when(r.entry.bytes > 0, |d| d.child(self.trash_note(r.entry.bytes, practice, &self.tools.explore_put_back, (!practice).then(|| r.entry.id.clone()), PutBackFor::Explore, cx)))
                        .when(can_remember, |d| {
                            d.child(card().p(px(CARD_PAD)).flex().items_center().gap(px(sp::M))
                                .child(icon_tile(Ico::Sparkle, p.grape, 40.))
                                .child(div().flex_1().min_w(px(0.)).flex().flex_col().gap(px(sp::XXS))
                                    .child(div().text_size(px(ts::LEAD)).font_weight(FontWeight::BLACK).child("Find these myself next time?"))
                                    .child(label_small("Checkups will list them, unticked, without asking anyone.")))
                                .child(mini_button("explore-remember", "Remember", p.grape, true).on_click(cx.listener(|this, _, _, cx| this.remember_tossed(cx)))))
                        })
                        .when(!r.errors.is_empty(), |d| d.child(notice(p.sun, p.warn).child(div().font_weight(FontWeight::BLACK).child(format!("{} stayed put", n_items(r.errors.len())))).children(r.errors.iter().take(3).map(|e| div().one_line().child(e.clone()))))),
                    ElementId::NamedInteger("explore-result".into(), nav),
                    motion::STAGGER,
                ))
            })
            .when_some(self.tools.explore_warn.clone(), |d, w| d.child(notice(p.sun, p.warn).mt(px(sp::S)).child(icon_label(Ico::Warning, ts::BODY, w).items_start())))
            .when(self.tools.explore_used.is_some(), |d| {
                // who was asked, on which subscription, and what it said
                // one quiet line: who was asked and what it cost; the sizes below are the story
                d.child(
                    div().mt(px(sp::XS)).flex().items_center().gap(px(sp::S))
                        .child(agent_tile(self.agent(), 24.))
                        .child(fill_line(label_small(self.tools.explore_used.clone().unwrap_or_default())))
                        .child(
                            div().id("explore-another").flex_none().h(px(HIT)).px(px(sp::S)).flex().items_center().rounded(px(radius::S))
                                .text_size(px(ts::SMALL)).font_weight(FontWeight::BLACK).text_color(rgb(p.grape.text))
                                .when(!self.tools.explore_busy, |d| d.cursor_pointer().hover(move |s| s.bg(rgb(p.grape.soft))).on_click(cx.listener(|this, _, _, cx| this.ask_another(cx))))
                                .child("Ask someone else"),
                        ),
                )
            });
        // wide windows: the whole story of the row under the pointer (or the focused one)
        let hov = self.tools.hovered.clone();
        let sel_i = items.iter().position(|s| hov.as_deref() == Some(format!("explore-{}", s.path.to_string_lossy()).as_str())).or(self.tools.focus.filter(|i| *i < items.len())).or((!items.is_empty()).then_some(0));
        let wide = self.is_wide();
        let list = div()
            .id("explore-list")
            .flex_1()
            .min_h(px(0.))
            .overflow_y_scroll()
            .track_scroll(&self.tools.scroll)
            .px(px(GUTTER))
            .when(items.is_empty(), |d| {
                d.child(pop_in(
                    card().mt(px(sp::M)).px(px(sp::XL)).py(px(sp::XL)).flex().flex_col().items_center().gap(px(sp::XS))
                        .child(hero_icon(Ico::Sparkle))
                        .child(div().text_size(px(ts::TITLE)).font_weight(FontWeight::BLACK).child("All looked at"))
                        .child(subtitle("Nothing else Claude thinks is safe to toss. ⌘R looks again.").text_center()),
                    ElementId::NamedInteger("explore-empty".into(), nav),
                    motion::STAGGER,
                ))
            })
            .children(items.iter().enumerate().map(|(i, s)| {
                let on = self.tools.explore_sel.contains(&s.path);
                let t = tone(s.group);
                let hid = format!("explore-{}", s.path.to_string_lossy());
                let hid2 = hid.clone();
                let hovered = self.tools.hovered.as_deref() == Some(hid.as_str());
                let ringed = self.tool_ring(i);
                let (path, rpath) = (s.path.clone(), s.path.clone());
                let row = div()
                    .id(ElementId::Name(hid.clone().into()))
                    .flex()
                    .items_start()
                    .gap(px(sp::M))
                    .px(px(sp::M + sp::XXS))
                    .py(px(sp::S + sp::XXS))
                    .rounded(px(radius::L))
                    .border_2()
                    .border_b_4()
                    // the row the inspector is telling about wears its group's color, lightly
                    .border_color(rgb(if wide && sel_i == Some(i) { p.grape.base } else if on { t.base } else { p.line }))
                    .bg(rgb(if on { t.soft } else { p.card }))
                    .when(ringed, |d| d.shadow(focus_ring(p.bg)))
                    .cursor_pointer()
                    // hovering shows it in the inspector, so it gets the inspector's outline
                    .hover(move |st| st.border_color(rgb(if wide { p.grape.base } else { t.base })))
                    .active(|st| st.border_b_2().mt(px(2.)))
                    .on_mouse_down(MouseButton::Left, |_, _, _| haptic(Haptic::Tick))
                    .on_hover(cx.listener(move |this, on: &bool, _, cx| this.set_tool_hover(&hid2, *on, cx)))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.tools.focus = Some(i);
                        this.tools.ring = false;
                        this.toggle_suggestion(&path, cx);
                    }))
                    .child(div().pt(px(sp::XXS)).child(checkbox(format!("eck-{i}").into(), on, t, 24., false)))
                    .child(glyph_tile(s.group, 32.))
                    .child(
                        // name, chips and size on the first line; Claude's reason and the place under them
                        div().flex_1().min_w(px(0.)).flex().flex_col().gap(px(sp::XXS))
                            .child(
                                div().flex().items_start().gap(px(sp::S))
                                    .child(div().flex_1().min_w(px(0.)).text_size(px(ts::BODY)).line_height(px(ts::BODY * 1.3)).font_weight(FontWeight::BLACK).child(s.what.clone()))
                                    .when(hovered, |d| d.child(div().flex_none().my(px(-(HIT - ts::BODY * 1.25) / 2.)).child(row_action(format!("ereveal-{i}"), Ico::Reveal, "Show in Finder", p.grape).on_click(cx.listener(move |this, _, _, cx| this.reveal_path(rpath.clone(), cx))))))
                                    .child(number(format_bytes(s.bytes), ts::BODY, p.ink)),
                            )
                            // wide: the inspector has the whole story, so the row keeps to two lines
                            .child(div().text_size(px(ts::SMALL)).line_height(px(ts::SMALL * 1.4)).font_weight(FontWeight::SEMIBOLD).text_color(rgb(p.ink)).when(wide, |d| d.line_clamp(2)).child(s.why.clone()))
                            .when(!wide && !s.comes_back.is_empty(), |d| d.child(label_small(format!("Once it's gone: {}", lower_first(&s.comes_back)))))
                            // the section above says how risky it is; the place and its age close the row
                            .child(div().flex().items_center().gap(px(sp::S)).child(fill_line(label_small(short_path(&tildify(&s.path), if wide { 64 } else { 26 })))).when_some(s.untouched_days.map(age), |d, a| d.child(label_small(a).flex_none()))),
                    );
                let k = kind(s);
                let first = i == 0 || kind(&items[i - 1]).rank != k.rank;
                div()
                    .when(first, |d| d.child(self.section_head(&items, i, &k, cx)))
                    .child(div().mt(px(sp::S - 2. * RING_PAD)).py(px(RING_PAD)).child(row))
            }))
            // what the agent chose not to suggest, in full: it's part of why you can trust the list
            .when_some(self.tools.explore_note.clone(), |d, note| {
                d.child(
                    div().mt(px(sp::L)).flex().flex_col().gap(px(sp::XS))
                        .child(div().flex().items_center().min_h(px(HIT)).child(small_chip("LEFT ALONE", p.grape)))
                        .child(card().p(px(CARD_PAD)).flex().items_start().gap(px(sp::M))
                            .child(agent_tile(self.agent(), 32.))
                            .child(div().flex_1().min_w(px(0.)).text_size(px(ts::SMALL)).line_height(px(ts::SMALL * 1.45)).font_weight(FontWeight::SEMIBOLD).text_color(rgb(p.ink)).child(note))),
                )
            })
            .child(div().h(px(sp::XL)));
        let go = if self.tools.explore_busy {
            "Moving…".to_string()
        } else if n_sel == 0 {
            "Tick what goes to the Trash".to_string()
        } else if self.dry_run {
            format!("Practice with {n_sel} · {}", format_bytes(sel_bytes))
        } else {
            format!("Move {n_sel} to Trash · {}", format_bytes(sel_bytes))
        };
        let inspector = (self.is_wide() && !items.is_empty()).then(|| {
            // the row already says what it is and what happens after; this is the decision, then the proof
            let body = sel_i.map(|i| &items[i]).map(|s| {
                let k = kind(s);
                let (rpath, tpath) = (s.path.clone(), s.path.clone());
                let on = self.tools.explore_sel.contains(&s.path);
                div().flex().flex_col().gap(px(sp::M))
                    .child(div().flex().items_center().gap(px(sp::M)).child(glyph_tile(s.group, 40.)).child(small_chip(k.chip, k.tone)))
                    .child(div().text_size(px(ts::TITLE)).font_weight(FontWeight::BLACK).child(s.what.clone()))
                    .child(div().flex().items_baseline().gap(px(sp::S)).child(number(format_bytes(s.bytes), ts::H2, p.ink)).when_some(s.untouched_days.map(age), |d, a| d.child(label_small(lower_first(&a)))))
                    // ticking only picks it: the coral button is the one that moves things
                    .child(div().flex().items_center().gap(px(sp::S))
                        .child(div().flex_1().child(button("insp-toggle", if on { "Untick it" } else { "Tick it" }, Btn::Ghost, true).on_click(cx.listener(move |this, _, _, cx| this.toggle_suggestion(&tpath, cx)))))
                        .child(row_action(format!("insp-reveal-{}", s.path.to_string_lossy()), Ico::Reveal, "Show in Finder", p.grape).on_click(cx.listener(move |this, _, _, cx| this.reveal_path(rpath.clone(), cx)))))
                    .when(!s.comes_back.is_empty(), |d| d.child(div().flex().flex_col().gap(px(sp::XXS)).child(label_small("Once it's gone")).child(div().text_size(px(ts::BODY)).line_height(px(ts::BODY * 1.35)).font_weight(FontWeight::SEMIBOLD).child(upper_first(&s.comes_back)))))
                    .when(s.evidence.iter().any(|e| !is_just_a_size(e)), |d| d.child(div().flex().flex_col().gap(px(sp::XXS)).child(label_small(format!("What {} went by", self.agent().name))).children(s.evidence.iter().filter(|e| !is_just_a_size(e)).map(|e| div().text_size(px(ts::SMALL)).line_height(px(ts::SMALL * 1.4)).font_weight(FontWeight::SEMIBOLD).child(format!("· {e}"))))))
                    .child(div().flex().flex_col().gap(px(sp::XXS)).child(label_small("Where")).child(div().text_size(px(ts::SMALL)).line_height(px(ts::SMALL * 1.4)).font_weight(FontWeight::SEMIBOLD).text_color(rgb(p.muted)).child(tildify(&s.path).replace('/', "/\u{200B}").replace('.', ".\u{200B}"))))
            });
            // as tall as what it says, up to the list's height
            card().w(px(300.)).flex_none().max_h_full().mr(px(GUTTER)).mt(px(sp::S)).mb(px(sp::S)).p(px(CARD_PAD)).flex().flex_col().child(div().id("explore-insp").min_h(px(0.)).overflow_y_scroll().children(body))
        });
        let list_pane = div().flex_1().min_w(px(0.)).min_h(px(0.)).flex().flex_col().child(fade_top(list, p.bg));
        div()
            .flex_1()
            .min_h(px(0.))
            .flex()
            .flex_col()
            .child(banner)
            .child(div().flex_1().min_h(px(0.)).flex().items_start().child(list_pane.h_full()).when_some(inspector, |d, i| d.child(i)))
            .child(footer(p.bg).child(button("explore-trash", go, Btn::Primary(p.coral), n_sel > 0 && !self.tools.explore_busy).on_click(cx.listener(|this, _, _, cx| {
                if !this.tools.explore_sel.is_empty() && !this.tools.explore_busy {
                    this.open_sheet(ConfirmKind::TrashSuggestions, cx)
                }
            }))))
            .into_any_element()
    }
}

/// "Asked Codex on your ChatGPT Team plan · about 91k tokens · peeked at 2 things itself"
fn used_line(agent: &Agent, r: &dusty_ai::ExploreReport) -> String {
    let plan = r.account.as_ref().map(|a| format!("{a} plan")).unwrap_or_else(|| agent.plan.to_string());
    let mut parts = vec![if r.tokens > 0 { format!("{} used about {}k tokens of your {plan}", agent.name, r.tokens.div_ceil(1000)) } else { format!("Asked {} on your {plan}", agent.name) }];
    if r.own_looks > 0 {
        parts.push(format!("peeked at {} thing{} itself", r.own_looks, plural_s(r.own_looks)));
    }
    parts.join(" · ")
}

/// The agent's logo (from the ACP registry) on a white tile, like an app icon: the same in light and dark,
/// so single-color marks stay visible, and away from the mess colors.
fn agent_tile(a: &Agent, side: f32) -> Div {
    div().size(px(side)).flex_none().rounded(px(if side >= 40. { radius::M + 2. } else { radius::M - 2. })).border_2().border_color(rgb(pal().line)).bg(rgb(if is_dark() { 0xE9E3FA } else { 0xFFFFFF })).flex().items_center().justify_center().child(icon(agent_logo(a), (side * 0.72).round()))
}

fn agent_logo(a: &Agent) -> Ico {
    match a.id {
        "claude" => Ico::LogoClaude,
        "codex" => Ico::LogoCodex,
        "gemini" => Ico::LogoGemini,
        "copilot" => Ico::LogoCopilot,
        "cursor" => Ico::LogoCursor,
        _ => Ico::LogoOpencode,
    }
}

/// "2.1 GB", "386 MB": evidence that only repeats the size shown above it.
fn is_just_a_size(e: &str) -> bool {
    let e = e.trim().trim_start_matches('~').trim();
    let unit = e.trim_start_matches(|c: char| c.is_ascii_digit() || c == '.' || c == ',' || c == ' ');
    unit.len() < e.len() && ["B", "KB", "MB", "GB", "TB"].contains(&unit.to_uppercase().as_str())
}

/// The section a suggestion sits in, with its badge: safe ones in mint, re-downloads in grape (the opt-in
/// color), anything that might hold something of yours in sun.
struct Kind {
    rank: u8,
    chip: &'static str,
    tone: Tone,
    blurb: &'static str,
}

fn kind(s: &Suggestion) -> Kind {
    match (s.risk, s.group) {
        (_, dusty_core::Group::Redownload) => Kind { rank: 1, chip: "RE-DOWNLOAD", tone: pal().grape, blurb: "They come back, but cost a download." },
        (Risk::Medium, _) => Kind { rank: 2, chip: "CHECK FIRST", tone: pal().sun, blurb: "Might hold something of yours: have a look before you tick." },
        (Risk::Low, _) => Kind { rank: 0, chip: "REBUILDS ITSELF", tone: pal().mint, blurb: "Caches and logs: made again when needed." },
    }
}

/// "Downloaded again…" → "downloaded again…", to sit inside a sentence.
fn lower_first(s: &str) -> String {
    let mut c = s.chars();
    c.next().map(|f| f.to_lowercase().chain(c).collect()).unwrap_or_default()
}

#[derive(Clone, Copy, PartialEq)]
enum Step {
    Done,
    Now,
    Later,
}

/// One step of a trip: a mint check when done, a pulsing grape dot while it runs, an empty ring to come.
fn step_row(i: usize, st: Step, title: String, detail: String) -> Div {
    let p = pal();
    let side = 26.;
    let mark = div().size(px(side)).flex_none().rounded_full().flex().items_center().justify_center().text_size(px(ts::SMALL)).font_weight(FontWeight::BLACK);
    let mark = match st {
        Step::Done => mark.bg(rgb(p.mint.base)).text_color(rgb(0x2B2340)).child("✓").into_any_element(),
        Step::Later => mark.border_2().border_color(rgb(p.line)).into_any_element(),
        Step::Now => {
            let dot = div().size(px(10.)).rounded_full().bg(rgb(p.grape.base));
            let dot = if reduced() {
                dot.into_any_element()
            } else {
                dot.with_animation(ElementId::NamedInteger("step-pulse".into(), i as u64), Animation::new(Duration::from_millis(1100)).repeat().with_easing(ease_in_out), |el, t| {
                    let k = 1. - (2. * t - 1.).abs();
                    el.opacity(0.45 + 0.55 * k).size(px(8. + 6. * k))
                })
                .into_any_element()
            };
            mark.border_2().border_color(rgb(p.grape.base)).bg(rgb(p.grape.soft)).child(dot).into_any_element()
        }
    };
    let later = st == Step::Later;
    div().flex().items_start().gap(px(sp::M))
        .child(mark)
        .child(
            div().flex_1().min_w(px(0.)).flex().flex_col().gap(px(1.))
                .child(div().text_size(px(ts::BODY)).line_height(px(side)).font_weight(FontWeight::BLACK).text_color(rgb(if later { p.muted } else { p.ink })).child(title))
                .child(label_small(detail).when(st == Step::Now, |d| d.text_color(rgb(p.grape.text)))),
        )
}

/// "get it at…" → "Get it at…", to start a line.
fn upper_first(s: &str) -> String {
    let mut c = s.chars();
    c.next().map(|f| f.to_uppercase().chain(c).collect()).unwrap_or_default()
}

/// "Untouched for 8 months": how long since anything in it changed.
fn age(days: u64) -> String {
    match days {
        0 => "Changed today".into(),
        1 => "Changed yesterday".into(),
        2..=13 => format!("Changed {days} days ago"),
        14..=59 => format!("Untouched for {} weeks", days / 7),
        60..=729 => format!("Untouched for {} months", days / 30),
        _ => format!("Untouched for {} years", days / 365),
    }
}

/// A long `~/…` path cut in the middle, so where it starts and the folder's own name both stay:
/// "~/Library/…/Caches/AppInstallationBinaryDeltas".
fn short_path(p: &str, max: usize) -> String {
    if p.chars().count() <= max {
        return p.to_string();
    }
    let parts: Vec<&str> = p.split('/').collect();
    let head = parts.iter().take(2).copied().collect::<Vec<_>>().join("/");
    let mut tail: Vec<&str> = vec![];
    for part in parts.iter().skip(2).rev() {
        let len: usize = tail.iter().map(|t| t.chars().count() + 1).sum::<usize>() + part.chars().count();
        if !tail.is_empty() && head.chars().count() + 3 + len > max {
            break;
        }
        tail.insert(0, part);
    }
    if tail.len() + 2 >= parts.len() {
        return p.to_string();
    }
    format!("{head}/…/{}", tail.join("/"))
}
