//! The update pill in the title band and the sheet that shows release notes and installs updates.

use super::*;

/// The "Update" pill: sits after the traffic lights, grape-tinted so it reads as news, not chrome.
pub(super) fn update_pill(label: SharedString) -> Stateful<Div> {
    let g = pal().grape;
    div()
        .id("update-pill")
        .absolute()
        .left(px(92.))
        .top(px((TOP - (HIT + 4.)) / 2.))
        .h(px(HIT + 4.))
        .px(px(sp::M))
        .flex()
        .items_center()
        .gap(px(sp::S - 2.))
        .rounded_full()
        .border_2()
        .border_b_4()
        .border_color(rgb(g.base))
        .bg(rgb(g.soft))
        .text_size(px(ts::SMALL))
        .font(tabular(FontWeight::BLACK))
        .text_color(rgb(g.text))
        .cursor_pointer()
        .hover(|s| s.bg(rgb(pal().card)))
        .active(|s| s.border_b_2().mt(px(2.)))
        .on_mouse_down(MouseButton::Left, |_, _, cx| {
            haptic(Haptic::Click);
            cx.stop_propagation()
        })
        .child(div().size(px(7.)).rounded_full().bg(rgb(g.base)))
        .child(label)
}

/// Small text button under the main buttons ("Skip this version").
pub(super) fn text_link(id: &'static str, label: &'static str) -> Stateful<Div> {
    div()
        .id(id)
        .h(px(HIT))
        .px(px(sp::S))
        .flex()
        .items_center()
        .rounded(px(radius::S))
        .text_size(px(ts::SMALL))
        .font_weight(FontWeight::BOLD)
        .text_color(rgb(pal().muted))
        .cursor_pointer()
        .hover(|s| s.text_color(rgb(pal().grape.text)))
        .on_mouse_down(MouseButton::Left, |_, _, _| haptic(Haptic::Tick))
        .child(label)
}

pub(super) fn small_chip(label: &'static str, t: Tone) -> Div {
    div().px(px(sp::S)).py(px(sp::XXS)).rounded_full().bg(rgb(t.soft)).text_color(rgb(t.text)).text_size(px(ts::MICRO)).font_weight(FontWeight::BLACK).child(label)
}

impl DustyApp {
    /// Follow the updater: open / close the sheet and ease its progress bar. True while something moves.
    pub(super) fn sync_update(&mut self, dt: f32) -> bool {
        let snap = updater::snapshot();
        let show = snap.open && !matches!(snap.phase, UpdPhase::Idle);
        if let UpdPhase::Found(f) = &snap.phase {
            self.upd_version = f.version.clone();
        }
        if show {
            if !self.upd_visible || self.upd_closing.is_some() {
                self.upd_visible = true;
                self.upd_closing = None;
                self.upd_t = Instant::now();
            }
            if std::mem::discriminant(&snap.phase) != std::mem::discriminant(&self.upd_phase) {
                self.upd_bar = 0.;
            }
            self.upd_phase = snap.phase.clone();
        } else if self.upd_visible && self.upd_closing.is_none() {
            self.upd_closing = Some(Instant::now());
        }
        if self.upd_closing.is_some_and(|t| since(t, motion::EXIT) >= 1.) {
            self.upd_visible = false;
            self.upd_closing = None;
        }
        // keep the pill's percentage moving even while the sheet is away
        if !show {
            if !matches!(snap.phase, UpdPhase::Idle) {
                self.upd_phase = snap.phase.clone();
            } else if !self.upd_visible {
                self.upd_phase = UpdPhase::Idle;
            }
        }
        let target = match &self.upd_phase {
            UpdPhase::Downloading { received, expected } if *expected > 0 => *received as f32 / *expected as f32,
            UpdPhase::Extracting(p) => *p as f32,
            _ => 0.,
        };
        let k = if reduced() { 1. } else { 1. - (-dt * 10.).exp() };
        self.upd_bar += (target - self.upd_bar) * k;
        let bar_moving = (target - self.upd_bar).abs() > 0.002;
        if !bar_moving {
            self.upd_bar = target;
        }
        let sheet_moving = self.upd_visible && (self.upd_t.elapsed() < Duration::from_millis(motion::SHEET + 200) || self.upd_closing.is_some());
        let spinning = self.upd_visible && matches!(self.upd_phase, UpdPhase::Checking | UpdPhase::Installing);
        bar_moving || sheet_moving || spinning
    }

    /// What the pill says when an update is under way but its sheet is put away.
    pub(super) fn update_pill_label(&self) -> Option<SharedString> {
        if self.upd_visible || self.intro.is_some() || self.screen == Screen::Cleaning {
            return None;
        }
        Some(match &self.upd_phase {
            UpdPhase::Found(_) => "Update".into(),
            UpdPhase::Downloading { received, expected } if *expected > 0 => format!("↓ {}%", (*received * 100 / *expected).min(100)).into(),
            UpdPhase::Downloading { .. } => "↓ Update".into(),
            UpdPhase::Extracting(_) => "Unpacking".into(),
            UpdPhase::Ready => "Restart".into(),
            UpdPhase::Installing => "Installing".into(),
            _ => return None,
        })
    }

    pub(super) fn update_sheet(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let phase = self.upd_phase.clone();
        let cur = updater::pretty(updater::current_version());
        let k_in = since(self.upd_t, motion::SHEET);
        let k_out = self.upd_closing.map(|t| since(t, motion::EXIT)).unwrap_or(0.);
        let backdrop = since(self.upd_t, motion::BASE) * (1. - k_out);
        let drop = 380. * (1. - back_out(k_in)) + 420. * k_out * k_out;
        let version = if self.upd_version.is_empty() { "the update".to_string() } else { updater::pretty(&self.upd_version) };

        let (emoji, title, sub): (Ico, String, String) = match &phase {
            UpdPhase::Idle | UpdPhase::Checking => (Ico::Telescope, "Looking for updates…".into(), "Asking GitHub if there's a newer Dusty.".into()),
            UpdPhase::Found(f) => (
                Ico::Gift,
                format!("Dusty {} is here", updater::pretty(&f.version)),
                if f.critical {
                    "This one fixes something important. Worth installing now.".into()
                } else if f.downloaded {
                    "It's already downloaded. Installing takes a few seconds.".into()
                } else if f.size > 0 {
                    format!("You have {cur}. It's a {} download.", format_bytes(f.size))
                } else {
                    format!("You have {cur}.")
                },
            ),
            UpdPhase::Downloading { .. } => (Ico::Redownload, format!("Downloading {version}"), "Keep cleaning if you like, it carries on in the background.".into()),
            UpdPhase::Extracting(_) => (Ico::Lock, "Checking the package…".into(), "Making sure it really comes from Dusty's maker, then unpacking.".into()),
            UpdPhase::Ready => (Ico::Sparkle, "Ready to install".into(), "Dusty closes for a second and comes right back.".into()),
            UpdPhase::Installing => (Ico::Wrench, "Installing…".into(), "Dusty will be right back.".into()),
            UpdPhase::UpToDate => (
                Ico::Relief,
                "You're up to date".into(),
                if updater::beta_enabled() { format!("Dusty {cur} is the newest version, betas included.") } else { format!("Dusty {cur} is the newest version.") },
            ),
            UpdPhase::Failed(msg) => (Ico::Dizzy, "Couldn't update".into(), msg.clone()),
        };

        // what tapping outside the sheet means in each state
        let outside = match &phase {
            UpdPhase::Checking => Some(UpdAction::Cancel),
            UpdPhase::UpToDate | UpdPhase::Failed(_) => Some(UpdAction::Acknowledge),
            UpdPhase::Installing | UpdPhase::Idle => None,
            _ => Some(UpdAction::Hide),
        };
        let act = |a: UpdAction| {
            cx.listener(move |_, _, _, cx| {
                updater::act(a);
                cx.notify();
            })
        };

        let body: Option<AnyElement> = match &phase {
            UpdPhase::Checking | UpdPhase::Installing => Some(div().w_full().px(px(sp::XL)).py(px(sp::S)).child(busy_bar()).into_any_element()),
            UpdPhase::Downloading { received, expected } => Some(
                div()
                    .w_full()
                    .flex()
                    .flex_col()
                    .gap(px(sp::S))
                    .py(px(sp::XS))
                    .child(progress_bar(self.upd_bar, pal().grape))
                    .child(
                        div()
                            .flex()
                            .justify_between()
                            .child(label_small(if *expected > 0 { format!("{} of {}", format_bytes(*received), format_bytes(*expected)) } else { format_bytes(*received) }).font(tabular(FontWeight::BOLD)))
                            .child(label_small(format!("{}%", (self.upd_bar * 100.).round() as u32)).font(tabular(FontWeight::BLACK)).text_color(rgb(pal().grape.text))),
                    )
                    .into_any_element(),
            ),
            UpdPhase::Extracting(_) => Some(div().w_full().py(px(sp::XS)).child(progress_bar(self.upd_bar, pal().mint)).into_any_element()),
            UpdPhase::Found(f) if !f.notes.is_empty() || f.beta => Some(
                card()
                    .w_full()
                    .p(px(CARD_PAD))
                    .flex()
                    .flex_col()
                    .gap(px(sp::S))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(sp::S))
                            .child(label_small("What's new"))
                            .when(f.beta, |d| d.child(small_chip("BETA", pal().sun)))
                            .when(f.critical, |d| d.child(small_chip("IMPORTANT", pal().coral))),
                    )
                    .children(f.notes.iter().enumerate().map(|(i, n)| {
                        pop_in(
                            div()
                                .flex()
                                .gap(px(sp::S))
                                .child(div().mt(px(6.)).size(px(6.)).flex_none().rounded_full().bg(rgb(pal().grape.base)))
                                .child(div().flex_1().min_w(px(0.)).text_size(px(ts::BODY)).line_height(px(ts::BODY * 1.4)).font_weight(FontWeight::SEMIBOLD).child(n.clone())),
                            ElementId::NamedInteger("upd-note".into(), i as u64),
                            120 + i as u64 * motion::STAGGER,
                        )
                    }))
                    .when(f.notes.is_empty(), |d| d.child(subtitle("A preview of what's coming. It may have rough edges.")))
                    .into_any_element(),
            ),
            _ => None,
        };

        let mut buttons: Vec<AnyElement> = Vec::new();
        match &phase {
            UpdPhase::Checking => buttons.push(button("upd-cancel", "Cancel", Btn::Ghost, true).on_click(act(UpdAction::Cancel)).into_any_element()),
            UpdPhase::Found(f) => {
                let label = if f.downloaded { "Install and relaunch" } else { "Update now" };
                buttons.push(button("upd-install", label, Btn::Primary(pal().grape), true).on_click(act(UpdAction::Install)).into_any_element());
                buttons.push(button("upd-later", "Later", Btn::Ghost, true).on_click(act(UpdAction::Later)).into_any_element());
                if !f.critical {
                    buttons.push(div().flex().justify_center().child(text_link("upd-skip", "Skip this version").on_click(act(UpdAction::Skip))).into_any_element());
                }
            }
            UpdPhase::Downloading { .. } => {
                buttons.push(button("upd-hide", "Keep using Dusty", Btn::Ghost, true).on_click(act(UpdAction::Hide)).into_any_element());
                buttons.push(div().flex().justify_center().child(text_link("upd-stop", "Cancel download").on_click(act(UpdAction::Cancel))).into_any_element());
            }
            UpdPhase::Extracting(_) => buttons.push(button("upd-hide", "Keep using Dusty", Btn::Ghost, true).on_click(act(UpdAction::Hide)).into_any_element()),
            UpdPhase::Ready => {
                buttons.push(button("upd-relaunch", "Install and relaunch", Btn::Primary(pal().mint), true).on_click(act(UpdAction::Install)).into_any_element());
                buttons.push(button("upd-onquit", "When I quit", Btn::Ghost, true).on_click(act(UpdAction::Later)).into_any_element());
            }
            UpdPhase::UpToDate => buttons.push(button("upd-ok", "Nice", Btn::Primary(pal().mint), true).on_click(act(UpdAction::Acknowledge)).into_any_element()),
            UpdPhase::Failed(_) => buttons.push(button("upd-ok", "OK", Btn::Ghost, true).on_click(act(UpdAction::Acknowledge)).into_any_element()),
            UpdPhase::Installing | UpdPhase::Idle => {}
        }

        // the emoji pops each time the state changes, not on every redraw
        let stage_key = match &phase {
            UpdPhase::Idle => 0u64,
            UpdPhase::Checking => 1,
            UpdPhase::Found(_) => 2,
            UpdPhase::Downloading { .. } => 3,
            UpdPhase::Extracting(_) => 4,
            UpdPhase::Ready => 5,
            UpdPhase::Installing => 6,
            UpdPhase::UpToDate => 7,
            UpdPhase::Failed(_) => 8,
        };
        div()
            .id("upd-wrap")
            .absolute()
            .inset_0()
            .occlude()
            .flex()
            .flex_col()
            .justify_end()
            .child({
                let bg = div().id("upd-bg").absolute().inset_0().bg(alpha(pal().scrim, pal().scrim_a)).opacity(backdrop);
                match outside {
                    Some(a) => bg.on_click(act(a)),
                    None => bg,
                }
            })
            .child(
                div()
                    .id("upd-sheet")
                    .relative()
                    .bg(rgb(pal().card))
                    .rounded_t(px(radius::SHEET))
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
                    .child(sheet_icon(emoji, ElementId::NamedInteger("upd-emoji".into(), stage_key)))
                    .child(div().text_size(px(ts::STAT)).line_height(px(ts::STAT * 1.25)).font(tabular(FontWeight::BLACK)).text_center().child(title))
                    .child(subtitle(sub).text_center())
                    .when_some(body, |d, b| d.child(div().w_full().mt(px(sp::S)).child(b)))
                    .when(!buttons.is_empty(), |d| d.child(div().h(px(sp::S))))
                    .children(buttons),
            )
    }
}
