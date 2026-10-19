// The debugger's panels in the bottom dock: registers, stack, memory, call
// stack, threads, modules, breakpoints, events and exceptions. They read the
// session's `DebugModel`; edits (a register, a breakpoint condition, the
// memory expression) use the table's inline cell editor.
use super::{BottomPanel, EditKind};
use crate::actions::{Cancel, Confirm};
use crate::debugger::{LogKind, flag_states};
use crate::scrollbar::{self, Axes, Source};
use crate::session::debug::mem_rows;
use crate::table::{self, Col, InlineEdit};
use crate::theme::{Theme, code_font_name, hsla};
use crate::ui::{self, BadgeKind};
use gpui::{AnyElement, Context, Window, div, prelude::*, px};
use ida_proto::{DbgReg, Ea};

/// The message that stands in for a panel without a stopped process.
fn placeholder(text: &str, theme: &Theme) -> AnyElement {
    div()
        .flex_1()
        .min_h_0()
        .flex()
        .items_center()
        .px(px(12.))
        .text_size(px(12.))
        .text_color(hsla(theme.faint))
        .child(text.to_owned())
        .into_any_element()
}

/// Header, scrolling rows and a scrollbar.
fn table_body(
    id: &'static str,
    cols: &[Col],
    rows: Vec<gpui::AnyElement>,
    theme: &Theme,
    window: &mut Window,
    cx: &mut gpui::App,
) -> AnyElement {
    let h = scrollbar::handle(id, window, cx);
    let bar = scrollbar::scrollbar(id, Source::handle(&h), Axes::V, theme, window, cx);
    div()
        .flex_1()
        .min_h_0()
        .flex()
        .flex_col()
        .child(table::header(cols, theme))
        .child(
            div()
                .relative()
                .flex_1()
                .min_h_0()
                .child(
                    div()
                        .id(id)
                        .size_full()
                        .overflow_y_scroll()
                        .track_scroll(&h)
                        .children(rows),
                )
                .child(bar),
        )
        .into_any_element()
}

fn hex(ea: Ea) -> String {
    format!("0x{ea:X}")
}

impl BottomPanel {
    // ---- inline editing ---------------------------------------------------

    pub(super) fn begin_edit(
        &mut self,
        kind: EditKind,
        text: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let edit = InlineEdit::start(
            0,
            0,
            text,
            |v: &mut Self, _, cx| v.cancel_edit(cx),
            window,
            cx,
        );
        self.edit = Some((kind, edit));
        cx.notify();
    }

    pub(super) fn cancel_edit(&mut self, cx: &mut Context<Self>) {
        if self.edit.take().is_some() {
            cx.notify();
        }
    }

    fn commit_edit(&mut self, cx: &mut Context<Self>) {
        let Some((kind, edit)) = self.edit.take() else {
            return;
        };
        let text = edit.text(cx);
        self.session.update(cx, |s, cx| match kind {
            EditKind::Reg(name) => s.dbg_set_reg(name, text, cx),
            EditKind::Cond(ea) => {
                // "py:" in front selects a Python condition.
                let (lang, cond) = match text.strip_prefix("py:") {
                    Some(rest) => ("Python", rest.trim().to_owned()),
                    None => ("", text),
                };
                s.dbg_bpt_cond(ea, cond, lang.to_owned(), cx)
            }
            EditKind::MemExpr => s.dbg_set_mem_expr(text, cx),
        });
        cx.notify();
    }

    /// Key handling shared by every panel: Enter and Escape reach the open
    /// cell editor through the `CellEdit` context.
    pub(super) fn wire_edit(
        &self,
        d: gpui::Stateful<gpui::Div>,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        d.when(self.edit.is_some(), |d| d.key_context("CellEdit"))
            .on_action(cx.listener(|v, _: &Confirm, window, cx| {
                v.commit_edit(cx);
                window.blur();
            }))
            .on_action(cx.listener(|v, _: &Cancel, window, cx| {
                v.cancel_edit(cx);
                window.blur();
            }))
    }

    fn editing(&self, kind: &EditKind) -> Option<gpui::Entity<crate::input::TextInput>> {
        self.edit
            .as_ref()
            .filter(|(k, _)| k == kind)
            .map(|(_, e)| e.input.clone())
    }

    // ---- registers --------------------------------------------------------

    pub(super) fn render_registers(
        &self,
        t: &Theme,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let s = self.session.read(cx);
        if !s.dbg.suspended() {
            return placeholder(
                if s.dbg.active() {
                    "The process is running. Pause it to see the registers"
                } else {
                    "No process. Start the debugger (F9)"
                },
                t,
            );
        }
        let regs: Vec<DbgReg> = s.dbg.regs.clone();
        let vregs: Vec<DbgReg> = s.dbg.vregs.clone();
        let changed = s.dbg.changed.clone();
        let sym: Vec<Option<String>> = regs
            .iter()
            .map(|r| {
                (r.address || r.ip || r.sp || r.fp)
                    .then(|| r.value())
                    .flatten()
                    .filter(|v| s.function_at(*v).is_some())
                    .map(|v| s.symbol_at(v))
            })
            .collect();
        let t = *t;
        let h = scrollbar::handle("regs-h", window, cx);
        let bar = scrollbar::scrollbar("regs-scroll", Source::handle(&h), Axes::V, &t, window, cx);
        let cells = regs.into_iter().zip(sym).enumerate().map(|(ix, (r, sym))| {
            let name = r.name.clone();
            let is_changed = changed.contains(&r.name);
            let digits = r.hex.trim_start_matches('0');
            let digits = if digits.is_empty() { "0" } else { digits };
            let shown = if r.bit_names.is_empty() {
                format!(
                    "{:0>width$}",
                    r.hex.to_uppercase(),
                    width = (r.bits / 4) as usize
                )
            } else {
                format!("{:X}h", u64::from_str_radix(digits, 16).unwrap_or(0))
            };
            let kind = EditKind::Reg(name.clone());
            let editing = self.editing(&kind);
            let value_text = r.hex.clone();
            let flags = flag_states(&r.bit_names, r.value().unwrap_or(0));
            let value_cell = div()
                .id(("reg", ix))
                .h(px(20.))
                .px(px(4.))
                .rounded(px(4.))
                .flex()
                .items_center()
                .font_family(code_font_name())
                .text_color(hsla(if is_changed { t.warning } else { t.text }))
                .when(is_changed, |d| d.bg(hsla(t.warning_soft)))
                .when(!r.readonly, |d| {
                    d.cursor_pointer()
                        .hover(move |d| d.bg(hsla(t.hover)))
                        .on_click(cx.listener({
                            let name = name.clone();
                            move |v, _, window, cx| {
                                let text = format!("0x{}", value_text.trim_start_matches('0'));
                                let text = if text == "0x" { "0".to_owned() } else { text };
                                v.begin_edit(EditKind::Reg(name.clone()), &text, window, cx)
                            }
                        }))
                })
                .map(|d| match editing {
                    Some(input) => d.w(px(190.)).child(input),
                    None => d.child(shown),
                });
            div()
                .when(r.bit_names.is_empty(), |d| d.w(px(250.)))
                .when(!r.bit_names.is_empty(), |d| d.w_full())
                .h(px(22.))
                .flex_none()
                .flex()
                .items_center()
                .gap(px(8.))
                .child(
                    div()
                        .w(px(44.))
                        .flex_none()
                        .font_family(code_font_name())
                        .text_size(px(12.))
                        .text_color(hsla(t.syntax.reg))
                        .child(name.clone()),
                )
                .child(value_cell)
                .when_some(sym, |d, text| {
                    d.child(
                        div()
                            .min_w_0()
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_ellipsis()
                            .text_size(px(11.))
                            .text_color(hsla(t.syntax.code_name))
                            .child(text),
                    )
                })
                .when(!flags.is_empty(), |d| {
                    d.child(div().flex().items_center().gap(px(2.)).children(
                        flags.into_iter().map(|(fname, on)| {
                            let reg = name.clone();
                            let bit = r
                                .bit_names
                                .iter()
                                .find(|(_, n)| *n == fname)
                                .map(|(b, _)| *b)
                                .unwrap_or(0);
                            let current = r.value().unwrap_or(0);
                            div()
                                .id(("flag", ix * 64 + bit as usize))
                                .px(px(4.))
                                .h(px(18.))
                                .flex()
                                .items_center()
                                .rounded(px(4.))
                                .cursor_pointer()
                                .text_size(px(10.))
                                .font_family(code_font_name())
                                .text_color(hsla(if on { t.accent } else { t.faint }))
                                .when(on, |d| d.bg(hsla(t.accent_soft)))
                                .hover(move |d| d.bg(hsla(t.hover)))
                                .on_click(cx.listener(move |v, _, _, cx| {
                                    let value = current ^ (1u64 << bit);
                                    let reg = reg.clone();
                                    v.session.update(cx, |s, cx| {
                                        s.dbg_set_reg(reg, format!("{value}"), cx)
                                    });
                                }))
                                .child(fname)
                        }),
                    ))
                })
        });
        div()
            .relative()
            .flex_1()
            .min_h_0()
            .child(
                div()
                    .id("regs-body")
                    .size_full()
                    .overflow_y_scroll()
                    .track_scroll(&h)
                    .child(
                        div()
                            .flex()
                            .flex_wrap()
                            .gap_x(px(16.))
                            .px(px(12.))
                            .py(px(6.))
                            .children(cells)
                            .when(!vregs.is_empty(), |d| {
                                d.child(
                                    div()
                                        .w_full()
                                        .pt(px(8.))
                                        .pb(px(2.))
                                        .child(crate::ui::section_label("Vector registers", &t)),
                                )
                                .children(vregs.into_iter().map(|r| {
                                    div()
                                        .w_full()
                                        .h(px(20.))
                                        .flex()
                                        .items_center()
                                        .gap(px(8.))
                                        .child(
                                            div()
                                                .w(px(44.))
                                                .flex_none()
                                                .font_family(code_font_name())
                                                .text_size(px(12.))
                                                .text_color(hsla(t.syntax.reg))
                                                .child(r.name.clone()),
                                        )
                                        .child(
                                            div()
                                                .min_w_0()
                                                .overflow_hidden()
                                                .font_family(code_font_name())
                                                .text_size(px(12.))
                                                .text_color(hsla(t.text))
                                                .child(r.hex.to_uppercase()),
                                        )
                                }))
                            }),
                    ),
            )
            .child(bar)
            .into_any_element()
    }

    // ---- stack ------------------------------------------------------------

    pub(super) fn render_stack(
        &self,
        t: &Theme,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        const COLS: [Col; 3] = [
            Col::fixed("Address", 190.),
            Col::fixed("Value", 190.),
            Col::flex("Info"),
        ];
        let s = self.session.read(cx);
        if !s.dbg.suspended() {
            return placeholder("The stack is shown while the process is suspended", t);
        }
        let sp = s.dbg.sp;
        let (min, max) = s
            .info
            .as_ref()
            .map(|i| (i.min_ea, i.max_ea))
            .unwrap_or((0, 0));
        let t = *t;
        let rows: Vec<AnyElement> = s
            .dbg
            .stack
            .iter()
            .enumerate()
            .map(|(i, w)| {
                let off = sp.map(|sp| w.addr.wrapping_sub(sp)).unwrap_or(0);
                let label = if off == 0 {
                    "RSP".to_owned()
                } else {
                    format!("RSP+{off:X}")
                };
                let target = w.value.filter(|v| *v >= min && *v < max);
                let addr = format!("{:016X}  {label}", w.addr);
                table::row_shell(("stack", i), self.selected == Some(i), false, &t)
                    .on_click(cx.listener(move |v, _, _, cx| {
                        v.selected = Some(i);
                        if let Some(ea) = target {
                            v.session.update(cx, |s, cx| s.navigate(ea, cx));
                        }
                        cx.notify();
                    }))
                    .child(table::cell(&COLS[0]).text_color(hsla(t.muted)).child(addr))
                    .child(table::cell(&COLS[1]).child(match w.value {
                        Some(v) => format!("{v:016X}"),
                        None => "????????????????".to_owned(),
                    }))
                    .child(
                        table::cell(&COLS[2])
                            .text_color(hsla(t.syntax.code_name))
                            .child(w.sym.clone().unwrap_or_default()),
                    )
                    .into_any_element()
            })
            .collect();
        table_body("stack-rows", &COLS, rows, &t, window, cx)
    }

    // ---- memory -----------------------------------------------------------

    pub(super) fn render_memory(
        &self,
        t: &Theme,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let s = self.session.read(cx);
        let t = *t;
        let expr = s.dbg.mem_expr.clone();
        let editing = self.editing(&EditKind::MemExpr);
        let head = div()
            .h(px(30.))
            .flex_none()
            .flex()
            .items_center()
            .gap(px(8.))
            .px(px(12.))
            .border_b_1()
            .border_color(hsla(t.border))
            .bg(hsla(t.surface2))
            .text_size(px(12.))
            .text_color(hsla(t.muted))
            .child("Follow")
            .child(match editing {
                Some(input) => div().w(px(260.)).child(input).into_any_element(),
                None => div()
                    .id("mem-expr")
                    .h(px(22.))
                    .px(px(8.))
                    .flex()
                    .items_center()
                    .rounded(px(4.))
                    .border_1()
                    .border_color(hsla(t.border))
                    .bg(hsla(t.bg))
                    .cursor_pointer()
                    .font_family(code_font_name())
                    .text_color(hsla(t.text))
                    .hover(move |d| d.border_color(hsla(t.border_strong)))
                    .on_click(cx.listener({
                        let expr = expr.clone();
                        move |v, _, window, cx| v.begin_edit(EditKind::MemExpr, &expr, window, cx)
                    }))
                    .child(expr.clone())
                    .into_any_element(),
            })
            .when_some(s.dbg.mem_addr, |d, a| {
                d.child(
                    div()
                        .font_family(code_font_name())
                        .text_color(hsla(t.syntax.num))
                        .child(hex(a)),
                )
            })
            .child(div().flex_1())
            .child(
                div()
                    .text_size(px(11.))
                    .text_color(hsla(t.faint))
                    .child("an expression: RSP, RIP, a name, 0x7FF…"),
            );
        if !s.dbg.suspended() {
            return div()
                .flex_1()
                .min_h_0()
                .flex()
                .flex_col()
                .child(head)
                .child(placeholder(
                    "Memory is read while the process is suspended",
                    &t,
                ))
                .into_any_element();
        }
        let rows: Vec<AnyElement> = match &s.dbg.mem {
            Some(mem) => mem_rows(mem)
                .into_iter()
                .enumerate()
                .map(|(i, r)| {
                    let group = |range: std::ops::Range<usize>| {
                        r.bytes[range.start.min(r.bytes.len())..range.end.min(r.bytes.len())]
                            .iter()
                            .map(|b| match b {
                                Some(b) => format!("{b:02X}"),
                                None => "??".to_owned(),
                            })
                            .collect::<Vec<_>>()
                            .join(" ")
                    };
                    div()
                        .id(("mem-row", i))
                        .h(px(20.))
                        .flex_none()
                        .flex()
                        .items_center()
                        .gap(px(16.))
                        .px(px(12.))
                        .font_family(code_font_name())
                        .text_size(px(12.))
                        .child(
                            div()
                                .w(px(140.))
                                .flex_none()
                                .text_color(hsla(t.syntax.num))
                                .child(format!("{:016X}", r.addr)),
                        )
                        .child(div().w(px(180.)).flex_none().child(group(0..8)))
                        .child(div().w(px(180.)).flex_none().child(group(8..16)))
                        .child(div().text_color(hsla(t.muted)).child(r.ascii.clone()))
                        .into_any_element()
                })
                .collect(),
            None => Vec::new(),
        };
        let h = scrollbar::handle("mem-h", window, cx);
        let bar = scrollbar::scrollbar("mem-scroll", Source::handle(&h), Axes::V, &t, window, cx);
        div()
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .child(head)
            .child(
                div()
                    .relative()
                    .flex_1()
                    .min_h_0()
                    .child(
                        div()
                            .id("mem-body")
                            .size_full()
                            .overflow_y_scroll()
                            .track_scroll(&h)
                            .py(px(4.))
                            .children(rows),
                    )
                    .child(bar),
            )
            .into_any_element()
    }

    // ---- call stack -------------------------------------------------------

    pub(super) fn render_call_stack(
        &self,
        t: &Theme,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        const COLS: [Col; 4] = [
            Col::fixed("#", 40.),
            Col::fixed("Address", 190.),
            Col::fixed("Frame", 190.),
            Col::flex("Function"),
        ];
        let s = self.session.read(cx);
        if !s.dbg.suspended() {
            return placeholder("The call stack is shown while the process is suspended", t);
        }
        let t = *t;
        let rows: Vec<AnyElement> = s
            .dbg
            .frames
            .iter()
            .enumerate()
            .map(|(i, f)| {
                let ea = f.ea;
                table::row_shell(("frame", i), self.selected == Some(i), false, &t)
                    .on_click(cx.listener(move |v, _, _, cx| {
                        v.selected = Some(i);
                        v.session.update(cx, |s, cx| s.navigate(ea, cx));
                        cx.notify();
                    }))
                    .child(
                        table::cell(&COLS[0])
                            .text_color(hsla(t.muted))
                            .child(i.to_string()),
                    )
                    .child(table::cell(&COLS[1]).child(format!("{:016X}", f.ea)))
                    .child(
                        table::cell(&COLS[2])
                            .text_color(hsla(t.muted))
                            .child(format!("{:016X}", f.fp)),
                    )
                    .child(
                        table::cell(&COLS[3])
                            .text_color(hsla(t.syntax.code_name))
                            .child(f.symbol.clone()),
                    )
                    .into_any_element()
            })
            .collect();
        table_body("frame-rows", &COLS, rows, &t, window, cx)
    }

    // ---- threads ----------------------------------------------------------

    pub(super) fn render_threads(
        &self,
        t: &Theme,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        const COLS: [Col; 4] = [
            Col::fixed("", 28.),
            Col::fixed("Thread", 110.),
            Col::fixed("Name", 150.),
            Col::flex("Instruction pointer"),
        ];
        let s = self.session.read(cx);
        if !s.dbg.active() {
            return placeholder("No process", t);
        }
        let t = *t;
        let rows: Vec<AnyElement> = s
            .dbg
            .threads
            .iter()
            .enumerate()
            .map(|(i, th)| {
                let tid = th.tid;
                // IDA names some threads by their start address; that is no name.
                let name = if th.name.is_empty()
                    || th
                        .name
                        .trim_start_matches("0x")
                        .chars()
                        .all(|c| c.is_ascii_hexdigit())
                {
                    format!("thread {}", th.tid)
                } else {
                    th.name.clone()
                };
                let ip = if th.ip == 0 {
                    String::new()
                } else {
                    {
                        let sym = s.symbol_at(th.ip);
                        if sym.starts_with("0x") {
                            format!("{:X}", th.ip)
                        } else {
                            format!("{:X}  {sym}", th.ip)
                        }
                    }
                };
                table::row_shell(("thread", i), self.selected == Some(i), false, &t)
                    .on_click(cx.listener(move |v, ev: &gpui::ClickEvent, _, cx| {
                        v.selected = Some(i);
                        if ev.click_count() >= 2 {
                            v.session.update(cx, |s, cx| s.dbg_select_thread(tid, cx));
                        }
                        cx.notify();
                    }))
                    .child(table::cell(&COLS[0]).child(if th.current {
                        ui::icon("chevron-right", 14., hsla(t.accent)).into_any_element()
                    } else {
                        div().into_any_element()
                    }))
                    .child(table::cell(&COLS[1]).child(th.tid.to_string()))
                    .child(table::cell(&COLS[2]).child(name))
                    .child(table::cell(&COLS[3]).child(ip))
                    .into_any_element()
            })
            .collect();
        if rows.is_empty() {
            return placeholder("Threads are listed while the process is suspended", &t);
        }
        table_body("thread-rows", &COLS, rows, &t, window, cx)
    }

    // ---- modules ----------------------------------------------------------

    pub(super) fn render_modules(
        &self,
        t: &Theme,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        const COLS: [Col; 3] = [
            Col::fixed("Base", 190.),
            Col::fixed("Size", 110.),
            Col::flex("Path"),
        ];
        let s = self.session.read(cx);
        if !s.dbg.active() {
            return placeholder("No process", t);
        }
        let (min, max) = s
            .info
            .as_ref()
            .map(|i| (i.min_ea, i.max_ea))
            .unwrap_or((0, 0));
        let t = *t;
        let rows: Vec<AnyElement> = s
            .dbg
            .modules
            .iter()
            .enumerate()
            .map(|(i, m)| {
                let base = m.base;
                table::row_shell(("module", i), self.selected == Some(i), false, &t)
                    .on_click(cx.listener(move |v, ev: &gpui::ClickEvent, _, cx| {
                        v.selected = Some(i);
                        if ev.click_count() >= 2 && base >= min && base < max {
                            v.session.update(cx, |s, cx| s.navigate(base, cx));
                        }
                        cx.notify();
                    }))
                    .child(
                        table::cell(&COLS[0])
                            .text_color(hsla(t.syntax.num))
                            .child(format!("{:016X}", m.base)),
                    )
                    .child(table::cell(&COLS[1]).child(format!("0x{:X}", m.size)))
                    .child(table::cell(&COLS[2]).child(m.name.clone()))
                    .into_any_element()
            })
            .collect();
        if rows.is_empty() {
            return placeholder("Modules are listed while the process is suspended", &t);
        }
        table_body("module-rows", &COLS, rows, &t, window, cx)
    }

    // ---- breakpoints ------------------------------------------------------

    pub(super) fn render_breakpoints(
        &self,
        t: &Theme,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        const COLS: [Col; 7] = [
            Col::fixed("", 32.),
            Col::fixed("Address", 190.),
            Col::fixed("Location", 220.),
            Col::fixed("Type", 70.),
            Col::fixed("Condition", 260.),
            Col::fixed("Hits", 60.),
            Col::flex(""),
        ];
        let s = self.session.read(cx);
        let t = *t;
        let rows: Vec<AnyElement> = s
            .dbg
            .bpts
            .iter()
            .enumerate()
            .map(|(i, b)| {
                let (ea, enabled) = (b.ea, b.enabled);
                let kind = EditKind::Cond(ea);
                let editing = self.editing(&kind);
                let cond_text = if b.lang.eq_ignore_ascii_case("python") && !b.cond.is_empty() {
                    format!("py: {}", b.cond)
                } else {
                    b.cond.clone()
                };
                let cond_edit = cond_text.clone();
                table::row_shell(("bpt", i), self.selected == Some(i), !enabled, &t)
                    .on_click(cx.listener(move |v, ev: &gpui::ClickEvent, _, cx| {
                        v.selected = Some(i);
                        if ev.click_count() >= 2 {
                            v.session.update(cx, |s, cx| s.navigate(ea, cx));
                        }
                        cx.notify();
                    }))
                    .child(
                        table::cell(&COLS[0]).child(
                            div()
                                .id(("bpt-on", i))
                                .size(px(16.))
                                .flex()
                                .items_center()
                                .justify_center()
                                .rounded(px(8.))
                                .border_1()
                                .border_color(hsla(t.danger))
                                .when(enabled, |d| d.bg(hsla(t.danger)))
                                .on_click(cx.listener(move |v, _, _, cx| {
                                    cx.stop_propagation();
                                    v.session
                                        .update(cx, |s, cx| s.dbg_bpt_enable(ea, !enabled, cx));
                                })),
                        ),
                    )
                    .child(table::cell(&COLS[1]).child(format!("{:016X}", b.ea)))
                    .child(
                        table::cell(&COLS[2])
                            .text_color(hsla(t.syntax.code_name))
                            .child(b.symbol.clone()),
                    )
                    .child(
                        table::cell(&COLS[3])
                            .text_color(hsla(t.muted))
                            .child(b.kind.clone()),
                    )
                    .child({
                        let c = table::cell(&COLS[4]);
                        match editing {
                            Some(input) => c.px(px(2.)).child(input).into_any_element(),
                            None => c
                                .id(("bpt-cond", i))
                                .on_click(cx.listener(
                                    move |v, ev: &gpui::ClickEvent, window, cx| {
                                        if ev.click_count() >= 2 {
                                            cx.stop_propagation();
                                            v.begin_edit(
                                                EditKind::Cond(ea),
                                                &cond_edit,
                                                window,
                                                cx,
                                            );
                                        }
                                    },
                                ))
                                .text_color(hsla(t.syntax.sym))
                                .child(if cond_text.is_empty() {
                                    "double-click to set".to_owned()
                                } else {
                                    cond_text
                                })
                                .when(b.cond.is_empty(), |d| d.text_color(hsla(t.faint)))
                                .into_any_element(),
                        }
                    })
                    .child(table::cell(&COLS[5]).child(b.hits.to_string()))
                    .child(table::cell(&COLS[6]).child(
                        ui::icon_button(("bpt-del", i), "trash", 20., false, true, &t).on_click(
                            cx.listener(move |v, _, _, cx| {
                                cx.stop_propagation();
                                v.session.update(cx, |s, cx| s.dbg_remove_bpt(ea, cx));
                            }),
                        ),
                    ))
                    .into_any_element()
            })
            .collect();
        if rows.is_empty() {
            return placeholder("No breakpoints. Press F2 on a line to set one", &t);
        }
        table_body("bpt-rows", &COLS, rows, &t, window, cx)
    }

    // ---- events -----------------------------------------------------------

    pub(super) fn render_events(
        &self,
        t: &Theme,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let s = self.session.read(cx);
        let t = *t;
        let lines: Vec<AnyElement> = s
            .dbg
            .log
            .iter()
            .map(|l| {
                let color = match l.kind {
                    LogKind::Info => t.muted,
                    LogKind::Output => t.text,
                    LogKind::Warn => t.warning,
                    LogKind::Error => t.danger,
                };
                div()
                    .whitespace_nowrap()
                    .text_color(hsla(color))
                    .when(l.kind == LogKind::Output, |d| {
                        d.child(
                            div()
                                .flex()
                                .gap(px(8.))
                                .child(ui::badge("out", BadgeKind::Neutral, &t))
                                .child(l.text.clone()),
                        )
                    })
                    .when(l.kind != LogKind::Output, |d| d.child(l.text.clone()))
                    .into_any_element()
            })
            .collect();
        if lines.is_empty() {
            return placeholder(
                "Process events and the program's debug output appear here",
                &t,
            );
        }
        let revision = s.dbg.revision;
        let h = scrollbar::handle("events-h", window, cx);
        let bar =
            scrollbar::scrollbar("events-scroll", Source::handle(&h), Axes::V, &t, window, cx);
        if self.events_seen.get() != revision {
            h.scroll_to_bottom();
        }
        div()
            .relative()
            .flex_1()
            .min_h_0()
            .child(
                div()
                    .id("events-lines")
                    .size_full()
                    .overflow_y_scroll()
                    .track_scroll(&h)
                    .px(px(12.))
                    .py(px(4.))
                    .font_family(code_font_name())
                    .text_size(px(12.))
                    .line_height(px(18.))
                    .children(lines),
            )
            .child(bar)
            .into_any_element()
    }

    // ---- exceptions -------------------------------------------------------

    pub(super) fn render_exceptions(
        &self,
        t: &Theme,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        const COLS: [Col; 5] = [
            Col::fixed("Code", 110.),
            Col::fixed("Name", 280.),
            Col::fixed("Stop", 60.),
            Col::fixed("Pass to program", 130.),
            Col::flex("Description").text(),
        ];
        let s = self.session.read(cx);
        let t = *t;
        let rows: Vec<AnyElement> = s
            .dbg
            .exceptions
            .iter()
            .enumerate()
            .map(|(i, e)| {
                let (code, stop, pass) = (e.code, e.stop, e.pass);
                let tick = |id: &'static str, on: bool| {
                    div()
                        .id((id, i))
                        .size(px(16.))
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded(px(4.))
                        .border_1()
                        .border_color(hsla(if on { t.accent } else { t.border_strong }))
                        .bg(hsla(if on { t.accent } else { t.bg }))
                        .cursor_pointer()
                        .when(on, |d| d.child(ui::icon("check", 12., hsla(t.on_accent))))
                };
                table::row_shell(("exc", i), self.selected == Some(i), false, &t)
                    .on_click(cx.listener(move |v, _, _, cx| {
                        v.selected = Some(i);
                        cx.notify();
                    }))
                    .child(table::cell(&COLS[0]).child(format!("{:08X}", e.code)))
                    .child(table::cell(&COLS[1]).child(e.name.clone()))
                    .child(table::cell(&COLS[2]).child(tick("exc-stop", stop).on_click(
                        cx.listener(move |v, _, _, cx| {
                            cx.stop_propagation();
                            v.session
                                .update(cx, |s, cx| s.dbg_set_exception(code, !stop, pass, cx));
                        }),
                    )))
                    .child(table::cell(&COLS[3]).child(tick("exc-pass", pass).on_click(
                        cx.listener(move |v, _, _, cx| {
                            cx.stop_propagation();
                            v.session
                                .update(cx, |s, cx| s.dbg_set_exception(code, stop, !pass, cx));
                        }),
                    )))
                    .child(
                        table::cell(&COLS[4])
                            .text_color(hsla(t.muted))
                            .child(e.desc.clone()),
                    )
                    .into_any_element()
            })
            .collect();
        if rows.is_empty() {
            return placeholder("Loading the exception table…", &t);
        }
        table_body("exc-rows", &COLS, rows, &t, window, cx)
    }
}
