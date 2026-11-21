//! History: past cleans, with Put back for whatever is still in the Trash.

use super::*;

impl DustyApp {
    pub(super) fn load_history(&mut self, cx: &mut Context<Self>) {
        self.bubble = "Flipping through my diary…".into();
        let task = cx.background_spawn(async move {
            load_history().into_iter().map(|e| HistRow { restorable: if e.practice { 0 } else { e.restorable().len() }, entry: e }).collect::<Vec<_>>()
        });
        cx.spawn(async move |this, cx| {
            let rows = task.await;
            let _ = this.update(cx, |app, cx| {
                if rows.is_empty() {
                    app.bubble = "Nothing in my diary yet. Clean something!".into();
                    app.dusty(cx, |d| d.st.set(Mood::Sleepy, Some(2.)));
                } else {
                    let real: Vec<&HistRow> = rows.iter().filter(|r| !r.entry.practice).collect();
                    let total: u64 = real.iter().map(|r| r.entry.bytes).sum();
                    app.bubble = if real.is_empty() {
                        format!("{} practice run{} so far. Nothing eaten for real yet.", rows.len(), plural_s(rows.len())).into()
                    } else {
                        format!("{} clean{} so far, {} eaten for real. Yum.", real.len(), plural_s(real.len()), format_bytes(total)).into()
                    };
                    app.dusty(cx, |d| d.st.set(Mood::Happy, Some(1.)));
                }
                app.tools.history = Some(rows);
                cx.notify();
            });
        })
        .detach();
    }

    pub(super) fn history_screen(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let p = pal();
        let nav = self.nav;
        let mut list = div().id("history-list").flex_1().min_h(px(0.)).overflow_y_scroll().px(px(GUTTER)).child(self.tool_header("History"));
        match &self.tools.history {
            None => list = list.child(card().mt(px(sp::M)).p(px(CARD_PAD)).child(busy_bar())),
            Some(rows) if rows.is_empty() => {
                list = list.child(pop_in(
                    card().mt(px(sp::M)).px(px(sp::XL)).py(px(sp::XL)).flex().flex_col().items_center().gap(px(sp::XS))
                        .child(hero_icon(Ico::History))
                        .child(div().text_size(px(ts::TITLE)).font_weight(FontWeight::BLACK).child("No cleans yet"))
                        .child(subtitle("Every clean, uninstall and big-file trip shows up here, with Put back for whatever is still in the Trash.").text_center()),
                    ElementId::NamedInteger("hist-empty".into(), nav),
                    motion::STAGGER,
                ))
            }
            Some(rows) => {
                list = list.children(rows.iter().take(200).enumerate().map(|(i, r)| {
                    let e = &r.entry;
                    use dusty_core::history::HistorySource as S;
                    let (glyph, t, title) = match e.source {
                        S::App => (Ico::Broom, p.mint, "Disk checkup".to_string()),
                        S::Scheduled => (Ico::Schedule, p.teal, "Weekly safe clean".to_string()),
                        S::Uninstall => (Ico::Uninstall, p.sky, format!("Uninstalled {}", e.items.first().map(|i| i.label.clone()).unwrap_or_else(|| "an app".into()))),
                        S::LargeFiles => (Ico::BigFiles, p.sun, format!("{} big file{}", e.items.len(), plural_s(e.items.len()))),
                        S::Cli => (Ico::Wrench, p.grape, "From the terminal".to_string()),
                    };
                    let (del, tr) = (e.bytes_deleted(), e.bytes_trashed());
                    let split = match (del > 0, tr > 0) {
                        (true, true) => format!("{} deleted · {} to the Trash", format_bytes(del), format_bytes(tr)),
                        (true, false) => format!("{} deleted", format_bytes(del)),
                        (false, true) => format!("{} to the Trash", format_bytes(tr)),
                        _ => n_items(e.items.len()),
                    };
                    let busy = self.tools.history_busy.as_deref() == Some(e.id.as_str());
                    let id = e.id.clone();
                    pop_in(
                        card().mt(px(sp::S)).p(px(CARD_PAD)).flex().items_center().gap(px(sp::M))
                            .child(icon_tile(glyph, t, 40.))
                            .child(
                                div().flex_1().min_w(px(0.)).flex().flex_col().gap(px(sp::XXS))
                                    .child(div().flex().items_center().gap(px(sp::S)).child(div().text_size(px(ts::LEAD)).font_weight(FontWeight::BLACK).one_line().child(title)).when(e.practice, |d| d.child(small_chip("PRACTICE", p.sky))))
                                    .child(label_small(format!("{} · {}", local_date(e.ts), ago(now_secs().saturating_sub(e.ts)))).one_line())
                                    .child(label_small(split).one_line())
                                    .when(!e.errors.is_empty(), |d| d.child(label_small(format!("{} left alone", e.errors.len())).text_color(rgb(p.warn)))),
                            )
                            .child(
                                div().flex_none().flex().flex_col().items_end().gap(px(sp::XS))
                                    .child(number(format_bytes(e.bytes), ts::LEAD, if e.practice { p.muted } else { t.text }))
                                    .when(r.restorable > 0, |d| d.child(mini_button_icon(("hist-pb", i), Some(Ico::PutBack), if busy { "Putting back…".to_string() } else { format!("Put back {}", r.restorable) }, p.grape, !busy && self.tools.history_busy.is_none()).on_click(cx.listener(move |this, _, _, cx| this.put_back_entry(id.clone(), PutBackFor::History, cx))))),
                            ),
                        ElementId::NamedInteger("hist".into(), nav * 1000 + i as u64),
                        (i.min(8) as u64) * motion::STAGGER / 2,
                    )
                }));
            }
        }
        list.child(div().h(px(sp::XL)))
    }
}
