// The bottom panel: IDA's output, the live cross references of the item
// under the caret, and the IDAPython console.
// Tabs use the underline style of the editor tab strip.
mod debug;

use crate::focus::{Focus, FocusEvent};
use crate::py_console::PyConsole;
use crate::scrollbar::{self, Axes, Source};
use crate::session::Session;
use crate::table::InlineEdit;
use crate::theme::{Theme, code_font_name, hsla};
use crate::ui::{self, BadgeKind};
use gpui::{
    Context, Entity, EventEmitter, IntoElement, Render, SharedString, Subscription, Window, canvas,
    deferred, div, prelude::*, px,
};
use std::cell::Cell;
use std::rc::Rc;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum BottomTab {
    Output,
    Xrefs,
    Python,
    // The debugger's panels.
    Registers,
    Stack,
    Memory,
    CallStack,
    Threads,
    Modules,
    Breakpoints,
    Events,
    Exceptions,
}

impl BottomTab {
    pub fn key(self) -> &'static str {
        match self {
            BottomTab::Output => "output",
            BottomTab::Xrefs => "xrefs",
            BottomTab::Python => "python",
            BottomTab::Registers => "registers",
            BottomTab::Stack => "stack",
            BottomTab::Memory => "memory",
            BottomTab::CallStack => "callstack",
            BottomTab::Threads => "threads",
            BottomTab::Modules => "modules",
            BottomTab::Breakpoints => "breakpoints",
            BottomTab::Events => "events",
            BottomTab::Exceptions => "exceptions",
        }
    }

    pub fn from_key(key: &str) -> Self {
        match key {
            "output" => BottomTab::Output,
            "python" => BottomTab::Python,
            "registers" => BottomTab::Registers,
            "stack" => BottomTab::Stack,
            "memory" => BottomTab::Memory,
            "callstack" => BottomTab::CallStack,
            "threads" => BottomTab::Threads,
            "modules" => BottomTab::Modules,
            "breakpoints" => BottomTab::Breakpoints,
            "events" => BottomTab::Events,
            "exceptions" => BottomTab::Exceptions,
            _ => BottomTab::Xrefs,
        }
    }

    fn label(self) -> &'static str {
        match self {
            BottomTab::Output => "Output",
            BottomTab::Xrefs => "Xrefs",
            BottomTab::Python => "Python",
            BottomTab::Registers => "Registers",
            BottomTab::Stack => "Stack",
            BottomTab::Memory => "Memory",
            BottomTab::CallStack => "Call stack",
            BottomTab::Threads => "Threads",
            BottomTab::Modules => "Modules",
            BottomTab::Breakpoints => "Breakpoints",
            BottomTab::Events => "Events",
            BottomTab::Exceptions => "Exceptions",
        }
    }

    /// One of the debugger panels.
    pub fn is_debug(self) -> bool {
        !matches!(
            self,
            BottomTab::Output | BottomTab::Xrefs | BottomTab::Python
        )
    }
}

/// What the inline cell editor of the debugger panels is editing.
#[derive(Clone, PartialEq)]
pub enum EditKind {
    Reg(String),
    /// The condition of the breakpoint at an address.
    Cond(ida_proto::Ea),
    MemExpr,
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
    /// The open cell editor of a debugger panel.
    edit: Option<(EditKind, InlineEdit)>,
    /// The debugger revision the events log last scrolled for.
    events_seen: std::cell::Cell<u64>,
    /// Width of the tab strip as last painted; tabs that do not fit fold into
    /// a menu.
    strip_w: Rc<Cell<f32>>,
    tab_menu: bool,
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
    /// The chevron behind which the tabs that do not fit wait.
    fn tab_overflow(
        &self,
        hidden: &[BottomTab],
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let t = *theme;
        let open = self.tab_menu;
        let menu = open.then(|| {
            deferred(
                div().absolute().top(px(30.)).left(px(0.)).child(
                    div()
                        .occlude()
                        .min_w(px(150.))
                        .p(px(4.))
                        .rounded(px(8.))
                        .bg(hsla(t.popover))
                        .border_1()
                        .border_color(hsla(t.border_strong))
                        .shadow(ui::drop_shadow(&t, 8., 24.))
                        .on_mouse_down_out(cx.listener(|v, _, _, cx| {
                            v.tab_menu = false;
                            cx.notify();
                        }))
                        .children(hidden.iter().map(|tab| {
                            let tab = *tab;
                            div()
                                .id(SharedString::from(format!("more-tab-{}", tab.label())))
                                .h(px(28.))
                                .px(px(10.))
                                .flex()
                                .items_center()
                                .rounded(px(4.))
                                .text_size(px(12.))
                                .cursor_pointer()
                                .text_color(hsla(t.text))
                                .hover(move |d| d.bg(hsla(t.hover)))
                                .on_click(cx.listener(move |v, _, _, cx| {
                                    v.tab_menu = false;
                                    v.tab = tab;
                                    v.selected = None;
                                    v.edit = None;
                                    cx.emit(BottomEvent::TabChanged);
                                    cx.notify();
                                }))
                                .child(tab.label())
                        })),
                ),
            )
        });
        div()
            .relative()
            .child(
                ui::icon_button("tabs-more", "chevron-down", 24., open, true, &t)
                    .tooltip(ui::tip("More tabs", None))
                    .on_click(cx.listener(|v, _, _, cx| {
                        v.tab_menu = !v.tab_menu;
                        cx.notify();
                    })),
            )
            .children(menu)
    }

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
            edit: None,
            events_seen: std::cell::Cell::new(0),
            strip_w: Rc::new(Cell::new(0.)),
            tab_menu: false,
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
        let (dbg_active, bpt_count) = {
            let s = self.session.read(cx);
            (s.dbg.active(), s.dbg.bpts.len())
        };
        // The debugger tabs show while a process lives (Breakpoints always).
        let mut tabs: Vec<Option<BottomTab>> = vec![
            Some(BottomTab::Output),
            Some(BottomTab::Xrefs),
            Some(BottomTab::Python),
            None,
        ];
        let debug_tabs: &[BottomTab] = if dbg_active || self.tab.is_debug() {
            &[
                BottomTab::Registers,
                BottomTab::Stack,
                BottomTab::Memory,
                BottomTab::CallStack,
                BottomTab::Threads,
                BottomTab::Modules,
                BottomTab::Breakpoints,
                BottomTab::Events,
                BottomTab::Exceptions,
            ]
        } else {
            &[BottomTab::Breakpoints]
        };
        tabs.extend(debug_tabs.iter().map(|t| Some(*t)));
        // Tabs that do not fit fold into a menu behind a chevron.
        let py_busy = self.console.read(cx).is_running();
        let real: Vec<BottomTab> = tabs.iter().flatten().copied().collect();
        let widths: Vec<f32> = real
            .iter()
            .map(|tab| {
                tab_width(
                    tab.label(),
                    (*tab == BottomTab::Xrefs && count > 0)
                        || (*tab == BottomTab::Breakpoints && bpt_count > 0)
                        || (*tab == BottomTab::Python && py_busy),
                )
            })
            .collect();
        let active_ix = real.iter().position(|t| *t == self.tab).unwrap_or(0);
        let strip_w = self.strip_w.get();
        let reserve = STRIP_RESERVE
            + if self.tab == BottomTab::Python {
                56.
            } else {
                0.
            }
            + if self.tab == BottomTab::Xrefs && !target.is_empty() {
                260.
            } else {
                0.
            };
        let mut show = vec![true; real.len()];
        if strip_w > 0. {
            let avail = (strip_w - reserve).max(0.);
            show = fit_tabs(&widths, active_ix, avail);
            if show.iter().any(|s| !s) {
                show = fit_tabs(&widths, active_ix, (avail - MORE_BUTTON).max(0.));
            }
        }
        let hidden: Vec<BottomTab> = real
            .iter()
            .zip(&show)
            .filter(|(_, s)| !**s)
            .map(|(t, _)| *t)
            .collect();
        {
            let mut it = show.iter();
            tabs.retain(|e| e.is_none() || *it.next().unwrap_or(&true));
            // A divider only between two groups that both still show.
            let mut cleaned: Vec<Option<BottomTab>> = Vec::new();
            for e in tabs.drain(..) {
                if e.is_none()
                    && (cleaned.is_empty() || cleaned.last().is_some_and(|l| l.is_none()))
                {
                    continue;
                }
                cleaned.push(e);
            }
            if cleaned.last().is_some_and(|l| l.is_none()) {
                cleaned.pop();
            }
            tabs = cleaned;
        }
        let body = match self.tab {
            BottomTab::Output => self.render_output(&t, window, cx),
            BottomTab::Xrefs => self.render_xrefs(&t, window, cx),
            BottomTab::Python => self.render_python(),
            BottomTab::Registers => self.render_registers(&t, window, cx),
            BottomTab::Stack => self.render_stack(&t, window, cx),
            BottomTab::Memory => self.render_memory(&t, window, cx),
            BottomTab::CallStack => self.render_call_stack(&t, window, cx),
            BottomTab::Threads => self.render_threads(&t, window, cx),
            BottomTab::Modules => self.render_modules(&t, window, cx),
            BottomTab::Breakpoints => self.render_breakpoints(&t, window, cx),
            BottomTab::Events => self.render_events(&t, window, cx),
            BottomTab::Exceptions => self.render_exceptions(&t, window, cx),
        };
        self.events_seen.set(self.session.read(cx).dbg.revision);
        let py_running = self.console.read(cx).is_running();
        self.wire_edit(div().id("bottom-panel"), cx)
            .size_full()
            .flex()
            .flex_col()
            .bg(hsla(t.surface))
            .text_color(hsla(t.text))
            .child(
                div()
                    .h(px(32.))
                    .flex_none()
                    .relative()
                    .flex()
                    .items_center()
                    .bg(hsla(t.bg))
                    .border_b_1()
                    .border_color(hsla(t.border))
                    .children(tabs.into_iter().map(|entry| {
                        let Some(tab) = entry else {
                            return div()
                                .w(px(1.))
                                .h(px(14.))
                                .mx(px(6.))
                                .bg(hsla(t.border))
                                .into_any_element();
                        };
                        let label = tab.label();
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
                                v.selected = None;
                                v.edit = None;
                                cx.emit(BottomEvent::TabChanged);
                                cx.notify();
                            }))
                            .child(label)
                            .when(tab == BottomTab::Xrefs && count > 0, |d| {
                                d.child(ui::badge(count.to_string(), BadgeKind::Count, &t))
                            })
                            .when(tab == BottomTab::Breakpoints && bpt_count > 0, |d| {
                                d.child(ui::badge(bpt_count.to_string(), BadgeKind::Danger, &t))
                            })
                            .when(tab == BottomTab::Python && py_running, |d| {
                                d.child(ui::spinner("py-tab-spin", 12., hsla(t.accent)))
                            })
                            .into_any_element()
                    }))
                    .when(!hidden.is_empty(), |d| {
                        d.child(self.tab_overflow(&hidden, &t, cx))
                    })
                    .child(div().flex_1())
                    .when(self.tab == BottomTab::Python, |d| {
                        let lang = if self.console.read(cx).idc {
                            "IDC"
                        } else {
                            "Python"
                        };
                        d.child(
                            ui::button("py-lang", lang, ui::ButtonKind::Ghost, true, &t)
                                .tooltip(ui::tip(
                                    "Switch the console language (Python / IDC)",
                                    None,
                                ))
                                .on_click(cx.listener(|v, _, _, cx| {
                                    v.console.update(cx, |c, cx| c.toggle_language(cx))
                                })),
                        )
                        .child(
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
                                .max_w(px(260.))
                                .overflow_hidden()
                                .whitespace_nowrap()
                                .text_ellipsis()
                                .text_size(px(12.))
                                .text_color(hsla(t.muted))
                                .child(format!("Xrefs to {target}")),
                        )
                    })
                    .child({
                        // Measures the strip so tabs can fold into the menu.
                        let width = self.strip_w.clone();
                        let view = cx.entity();
                        canvas(
                            move |bounds, _, cx| {
                                let w = f32::from(bounds.size.width);
                                if (width.get() - w).abs() > 2. {
                                    width.set(w);
                                    view.update(cx, |_, cx| cx.notify());
                                }
                            },
                            |_, _, _, _| {},
                        )
                        .absolute()
                        .size_full()
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

/// Room taken by the strip's right-hand controls (close button, margins).
const STRIP_RESERVE: f32 = 44.;
/// Width of the "more tabs" button.
const MORE_BUTTON: f32 = 32.;

/// Which of `widths` (tab widths, in order) stay on the strip in `avail`
/// pixels: the leading ones that fit, with the `active` tab always shown (the
/// tabs after the last one that fits give way to it).
fn fit_tabs(widths: &[f32], active: usize, avail: f32) -> Vec<bool> {
    let mut show = vec![false; widths.len()];
    let mut used = 0.;
    for (i, w) in widths.iter().enumerate() {
        if used + w > avail {
            break;
        }
        used += w;
        show[i] = true;
    }
    if active < widths.len() && !show[active] {
        // Make room for the active tab by dropping from the end.
        let need = widths[active];
        let mut i = show.iter().rposition(|&s| s);
        while let Some(last) = i {
            if used + need <= avail {
                break;
            }
            show[last] = false;
            used -= widths[last];
            i = show.iter().rposition(|&s| s);
        }
        show[active] = true;
    }
    show
}

/// A rough width for a tab: its label, padding and the badge it may carry.
fn tab_width(label: &str, badge: bool) -> f32 {
    label.chars().count() as f32 * 7. + 24. + if badge { 24. } else { 0. }
}

#[cfg(test)]
mod tests {
    use super::fit_tabs;

    #[test]
    fn everything_shows_when_it_fits() {
        assert_eq!(fit_tabs(&[50., 50., 50.], 0, 200.), vec![true; 3]);
        assert!(fit_tabs(&[], 0, 100.).is_empty());
    }

    #[test]
    fn the_leading_tabs_stay_and_the_rest_fold() {
        assert_eq!(
            fit_tabs(&[50., 50., 50., 50.], 0, 120.),
            vec![true, true, false, false]
        );
    }

    #[test]
    fn the_active_tab_is_always_shown_at_the_cost_of_the_last_visible_ones() {
        // Exceptions (last) is active: it takes the place of the tabs before it.
        assert_eq!(
            fit_tabs(&[50., 50., 50., 50.], 3, 120.),
            vec![true, false, false, true]
        );
        // A wide active tab pushes out more than one.
        assert_eq!(
            fit_tabs(&[40., 40., 40., 90.], 3, 130.),
            vec![true, false, false, true]
        );
    }

    #[test]
    fn an_active_tab_wider_than_the_strip_is_still_shown() {
        assert_eq!(
            fit_tabs(&[50., 50., 300.], 2, 100.),
            vec![false, false, true]
        );
    }
}
