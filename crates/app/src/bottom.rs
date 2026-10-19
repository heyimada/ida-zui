// The bottom panel: IDA's output, the live cross references of the item
// under the caret, and the IDAPython console.
// Tabs use the underline style of the editor tab strip.
use crate::focus::{Focus, FocusEvent};
use crate::py_console::PyConsole;
use crate::scrollbar::{self, Axes, Source};
use crate::session::Session;
use crate::theme::{Theme, code_font_name, hsla};
use crate::ui::{self, BadgeKind};
use gpui::{
    Context, Entity, EventEmitter, IntoElement, Render, SharedString, Subscription, Window, div,
    prelude::*, px,
};

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum BottomTab {
    Output,
    Xrefs,
    Python,
}

impl BottomTab {
    pub fn key(self) -> &'static str {
        match self {
            BottomTab::Output => "output",
            BottomTab::Xrefs => "xrefs",
            BottomTab::Python => "python",
        }
    }

    pub fn from_key(key: &str) -> Self {
        match key {
            "output" => BottomTab::Output,
            "python" => BottomTab::Python,
            _ => BottomTab::Xrefs,
        }
    }
}

pub enum BottomEvent {
    TabChanged,
    Close,
    /// The header's "Run script file" button.
    RunScript,
}

pub struct BottomPanel {
    session: Entity<Session>,
    focus: Entity<Focus>,
    pub console: Entity<PyConsole>,
    pub tab: BottomTab,
    selected: Option<usize>,
    _subs: Vec<Subscription>,
}

impl EventEmitter<BottomEvent> for BottomPanel {}

const COLS: [(&str, Option<f32>); 4] = [
    ("Type", Some(84.)),
    ("Address", Some(136.)),
    ("Location", Some(232.)),
    ("Text", None),
];

impl BottomPanel {
    pub fn new(
        session: Entity<Session>,
        focus: Entity<Focus>,
        tab: BottomTab,
        cx: &mut Context<Self>,
    ) -> Self {
        let console = cx.new(|cx| PyConsole::new(session.clone(), cx));
        let subs = vec![
            cx.subscribe(&focus, |this, _, _: &FocusEvent, cx| {
                this.selected = None;
                cx.notify();
            }),
            cx.observe(&session, |_, _, cx| cx.notify()),
            // The tab shows the busy state of the console.
            cx.observe(&console, |_, _, cx| cx.notify()),
        ];
        Self {
            session,
            focus,
            console,
            tab,
            selected: None,
            _subs: subs,
        }
    }

    fn render_xrefs(
        &self,
        theme: &Theme,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let t = *theme;
        let focus = self.focus.read(cx);
        let rows = focus.target_xrefs.clone();
        let xh = scrollbar::handle("xrefs-h", window, cx);
        let xbar = scrollbar::scrollbar(
            "xrefs-scroll",
            Source::handle(&xh),
            Axes::V,
            theme,
            window,
            cx,
        );
        let cell = |w: Option<f32>| {
            let d = div()
                .h_full()
                .flex()
                .items_center()
                .px(px(12.))
                .overflow_hidden()
                .whitespace_nowrap()
                .text_ellipsis();
            match w {
                Some(w) => d.w(px(w)).flex_none(),
                None => d.flex_1().min_w_0(),
            }
        };
        div()
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .child(
                div()
                    .h(px(24.))
                    .flex_none()
                    .flex()
                    .items_center()
                    .bg(hsla(t.surface2))
                    .border_b_1()
                    .border_color(hsla(t.border))
                    .text_size(px(11.))
                    .font_weight(gpui::FontWeight::MEDIUM)
                    .text_color(hsla(t.muted))
                    .children(COLS.iter().map(|(title, w)| cell(*w).child(*title))),
            )
            .child(
                div()
                    .relative()
                    .flex_1()
                    .min_h_0()
                    .child(
                        div()
                            .id("xrefs-rows")
                            .size_full()
                            .overflow_y_scroll()
                            .track_scroll(&xh)
                            .children(rows.iter().enumerate().map(|(i, x)| {
                                let from = x.from;
                                let on = self.selected == Some(i);
                                div()
                                    .id(("live-xref", i))
                                    .h(px(24.))
                                    .flex_none()
                                    .flex()
                                    .items_center()
                                    .cursor_pointer()
                                    .text_size(px(12.))
                                    .when(on, |d| d.bg(hsla(t.accent_soft)))
                                    .when(!on, |d| d.hover(move |d| d.bg(hsla(t.hover))))
                                    .on_click(cx.listener(move |v, _, _, cx| {
                                        v.selected = Some(i);
                                        v.session.update(cx, |s, cx| s.navigate(from, cx));
                                        cx.notify();
                                    }))
                                    .child(cell(COLS[0].1).child(ui::xref_badge(&x.kind, &t)))
                                    .child(
                                        cell(COLS[1].1)
                                            .font_family(code_font_name())
                                            .child(format!("0x{:x}", x.from)),
                                    )
                                    .child(
                                        cell(COLS[2].1)
                                            .font_family(code_font_name())
                                            .child(x.func.clone().unwrap_or_default()),
                                    )
                                    .child(
                                        cell(COLS[3].1)
                                            .font_family(code_font_name())
                                            .text_color(hsla(t.syntax.sym))
                                            .child(
                                                x.text
                                                    .split_whitespace()
                                                    .collect::<Vec<_>>()
                                                    .join(" "),
                                            ),
                                    )
                            }))
                            .when(rows.is_empty(), |d| {
                                d.child(
                                    div()
                                        .h(px(32.))
                                        .px(px(12.))
                                        .flex()
                                        .items_center()
                                        .text_size(px(12.))
                                        .text_color(hsla(t.faint))
                                        .child("No cross references to this item"),
                                )
                            }),
                    )
                    .child(xbar),
            )
            .child(
                div()
                    .h(px(24.))
                    .flex_none()
                    .flex()
                    .items_center()
                    .px(px(12.))
                    .border_t_1()
                    .border_color(hsla(t.border))
                    .bg(hsla(t.bg))
                    .text_size(px(11.))
                    .text_color(hsla(t.muted))
                    .child(format!(
                        "{} reference{}  ·  sorted by address",
                        rows.len(),
                        if rows.len() == 1 { "" } else { "s" }
                    )),
            )
            .into_any_element()
    }

    fn render_output(
        &self,
        theme: &Theme,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let t = *theme;
        let lines: Vec<SharedString> = self
            .session
            .read(cx)
            .log
            .iter()
            .rev()
            .take(300)
            .rev()
            .cloned()
            .collect();
        let oh = scrollbar::handle("output-h", window, cx);
        let obar = scrollbar::scrollbar(
            "output-scroll",
            Source::handle(&oh),
            Axes::V,
            theme,
            window,
            cx,
        );
        div()
            .relative()
            .flex_1()
            .min_h_0()
            .child(
                div()
                    .id("output-lines")
                    .size_full()
                    .overflow_y_scroll()
                    .track_scroll(&oh)
                    .px(px(12.))
                    .py(px(4.))
                    .font_family(code_font_name())
                    .text_size(px(12.))
                    .line_height(px(18.))
                    .text_color(hsla(t.muted))
                    .children(
                        lines
                            .into_iter()
                            .map(|l| div().whitespace_nowrap().child(l)),
                    ),
            )
            .child(obar)
            .into_any_element()
    }

    fn render_python(&self) -> gpui::AnyElement {
        div()
            .flex_1()
            .min_h_0()
            .child(self.console.clone())
            .into_any_element()
    }
}

impl Render for BottomPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::get(cx);
        let t = theme;
        let focus = self.focus.read(cx);
        let count = focus.target_xrefs.len();
        let target = focus.target_name.clone();
        let tabs = [
            (BottomTab::Output, "Output"),
            (BottomTab::Xrefs, "Xrefs"),
            (BottomTab::Python, "Python"),
        ];
        let body = match self.tab {
            BottomTab::Output => self.render_output(&t, window, cx),
            BottomTab::Xrefs => self.render_xrefs(&t, window, cx),
            BottomTab::Python => self.render_python(),
        };
        let py_running = self.console.read(cx).is_running();
        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(hsla(t.surface))
            .text_color(hsla(t.text))
            .child(
                div()
                    .h(px(32.))
                    .flex_none()
                    .flex()
                    .items_center()
                    .bg(hsla(t.bg))
                    .border_b_1()
                    .border_color(hsla(t.border))
                    .children(tabs.into_iter().map(|(tab, label)| {
                        let on = tab == self.tab;
                        div()
                            .id(label)
                            .h_full()
                            .px(px(12.))
                            .flex()
                            .items_center()
                            .gap(px(6.))
                            .text_size(px(12.))
                            .cursor_pointer()
                            .text_color(hsla(if on { t.text } else { t.muted }))
                            .when(on, |d| {
                                d.font_weight(gpui::FontWeight::MEDIUM)
                                    .border_b_2()
                                    .border_color(hsla(t.accent))
                            })
                            .when(!on, |d| d.hover(move |d| d.text_color(hsla(t.text))))
                            .on_click(cx.listener(move |v, _, _, cx| {
                                v.tab = tab;
                                cx.emit(BottomEvent::TabChanged);
                                cx.notify();
                            }))
                            .child(label)
                            .when(tab == BottomTab::Xrefs && count > 0, |d| {
                                d.child(ui::badge(count.to_string(), BadgeKind::Count, &t))
                            })
                            .when(tab == BottomTab::Python && py_running, |d| {
                                d.child(ui::spinner("py-tab-spin", 12., hsla(t.accent)))
                            })
                    }))
                    .child(div().flex_1())
                    .when(self.tab == BottomTab::Python, |d| {
                        d.child(
                            ui::icon_button("py-run-file", "play", 24., false, true, &t)
                                .tooltip(ui::tip(
                                    "Run script file…",
                                    Some(crate::actions::shortcut_text(
                                        &crate::actions::RunScriptFile,
                                        cx,
                                    )),
                                ))
                                .on_click(
                                    cx.listener(|_, _, _, cx| cx.emit(BottomEvent::RunScript)),
                                ),
                        )
                        .child(
                            ui::icon_button("py-clear", "trash", 24., false, true, &t)
                                .tooltip(ui::tip("Clear output", None))
                                .on_click(cx.listener(|v, _, _, cx| {
                                    v.console.update(cx, |c, cx| c.clear_output(cx))
                                })),
                        )
                    })
                    .when(self.tab == BottomTab::Xrefs && !target.is_empty(), |d| {
                        d.child(
                            div()
                                .max_w(px(480.))
                                .overflow_hidden()
                                .whitespace_nowrap()
                                .text_ellipsis()
                                .text_size(px(12.))
                                .text_color(hsla(t.muted))
                                .child(format!("Xrefs to {target}")),
                        )
                    })
                    .child(div().w(px(4.)))
                    .child(
                        ui::icon_button("bottom-close", "close", 24., false, true, &t)
                            .on_click(cx.listener(|_, _, _, cx| cx.emit(BottomEvent::Close))),
                    )
                    .child(div().w(px(8.))),
            )
            .child(body)
    }
}
