// The right-hand inspector: facts about the function or block
// under the caret on the Info tab, its declaration, its references and the
// comments on the item. Everything shown is data from `Focus` or the graph.
use crate::focus::{Focus, FocusEvent};
use crate::graph::{BlockInfo, GraphView};
use crate::scrollbar::{self, Axes, Source};
use crate::session::Session;
use crate::theme::{Theme, code_font_name, hsla};
use crate::ui::{self, BadgeKind};
use gpui::{
    Context, Entity, EventEmitter, HighlightStyle, IntoElement, ParentElement, Render,
    SharedString, StyledText, Subscription, Window, div, prelude::*, px,
};
use ida_proto::{Ea, EdgeKind, XrefRow};
use std::ops::Range;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum InspectorTab {
    Info,
    Type,
    Xrefs,
    Comments,
}

const TABS: [(InspectorTab, &str, &str); 4] = [
    (InspectorTab::Info, "info", "Info"),
    (InspectorTab::Type, "type", "Type"),
    (InspectorTab::Xrefs, "xrefs", "Xrefs"),
    (InspectorTab::Comments, "comments", "Comments"),
];

impl InspectorTab {
    pub fn key(self) -> &'static str {
        TABS.iter().find(|t| t.0 == self).map_or("info", |t| t.1)
    }

    pub fn from_key(key: &str) -> Self {
        TABS.iter()
            .find(|t| t.1 == key)
            .map_or(InspectorTab::Info, |t| t.0)
    }
}

pub enum InspectorEvent {
    /// The user picked another tab (so the workspace can remember it).
    TabChanged,
}

pub struct Inspector {
    session: Entity<Session>,
    focus: Entity<Focus>,
    graph: Entity<GraphView>,
    pub tab: InspectorTab,
    /// Show the basic block in the Info tab (graph view) instead of the function.
    pub block_mode: bool,
    _subs: Vec<Subscription>,
}

impl EventEmitter<InspectorEvent> for Inspector {}

fn hex(ea: Ea) -> String {
    format!("0x{ea:x}")
}

/// `A::f(int)` without its argument list, as the prototype prints the name.
pub fn strip_args(name: &str) -> &str {
    if !name.ends_with(')') {
        return name;
    }
    let mut depth = 0;
    for (i, c) in name.char_indices().rev() {
        match c {
            ')' => depth += 1,
            '(' => {
                depth -= 1;
                if depth == 0 {
                    return if i > 0 { &name[..i] } else { name };
                }
            }
            _ => {}
        }
    }
    name
}

/// Ranges of a one-line prototype to colour: calling convention, types, the
/// function name and the parameter names.
pub fn prototype_runs(
    text: &str,
    name: &str,
    theme: &Theme,
) -> Vec<(Range<usize>, HighlightStyle)> {
    let color = |c: u32| HighlightStyle {
        color: Some(hsla(c)),
        ..Default::default()
    };
    let s = &theme.syntax;
    let mut out = Vec::new();
    let bytes = text.as_bytes();
    let is_word = |b: u8| {
        b.is_ascii_alphanumeric()
            || b == b'_'
            || b == b':'
            || b == b'~'
            || b == b'?'
            || b == b'@'
            || b == b'$'
    };
    let mut i = 0;
    let mut depth = 0;
    while i < bytes.len() {
        let b = bytes[i];
        if b == b'(' {
            depth += 1;
        } else if b == b')' {
            depth -= 1;
        }
        if is_word(b) {
            let start = i;
            while i < bytes.len() && is_word(bytes[i]) {
                i += 1;
            }
            let word = &text[start..i];
            let followed_by_paren = text[i..].trim_start().starts_with('(');
            let c = if word.starts_with("__") && !word.starts_with("__int") {
                Some(s.kw)
            } else if crate::theme::is_type_word(word)
                || matches!(word, "struct" | "union" | "enum" | "const")
            {
                Some(if matches!(word, "struct" | "union" | "enum" | "const") {
                    s.kw
                } else {
                    s.ty
                })
            } else if word == name || (followed_by_paren && depth == 0) {
                Some(s.code_name)
            } else if depth > 0 && !word.starts_with(|c: char| c.is_ascii_digit()) {
                // A parameter: a bare name after the type, or a type name before `*`.
                let rest = text[i..].trim_start();
                let prev = text[..start].trim_end();
                if rest.starts_with(',') || rest.starts_with(')') || rest.starts_with('[') {
                    if prev.ends_with('*')
                        || prev.ends_with(' ')
                        || prev.ends_with(char::is_alphanumeric)
                    {
                        Some(s.loc_name)
                    } else {
                        Some(s.ty)
                    }
                } else {
                    Some(s.ty)
                }
            } else {
                None
            };
            if let Some(c) = c {
                out.push((start..i, color(c)));
            }
            continue;
        }
        i += 1;
    }
    out
}

impl Inspector {
    pub fn new(
        session: Entity<Session>,
        focus: Entity<Focus>,
        graph: Entity<GraphView>,
        tab: InspectorTab,
        cx: &mut Context<Self>,
    ) -> Self {
        let subs = vec![
            cx.subscribe(&focus, |_, _, _: &FocusEvent, cx| cx.notify()),
            cx.observe(&graph, |_, _, cx| cx.notify()),
        ];
        Self {
            session,
            focus,
            graph,
            tab,
            block_mode: false,
            _subs: subs,
        }
    }

    fn go(&mut self, ea: Ea, cx: &mut Context<Self>) {
        self.session.update(cx, |s, cx| s.navigate(ea, cx));
    }

    /// Right click on a row that stands for an address.
    fn row_menu(
        &mut self,
        ea: Ea,
        name: Option<String>,
        pos: gpui::Point<gpui::Pixels>,
        cx: &mut Context<Self>,
    ) {
        let entries = crate::context_menu::address_row(&self.session, ea, name.clone(), cx);
        let title = name.unwrap_or_else(|| format!("{ea:X}"));
        crate::context_menu::show(&self.session, pos, title, entries, false, cx);
    }

    /// A key/value row: the key in a 96 px column, the value beside it.
    fn kv(key: &str, value: impl IntoElement, theme: &Theme) -> gpui::Div {
        div()
            .h(px(24.))
            .px(px(16.))
            .flex()
            .items_center()
            .gap(px(8.))
            .text_size(px(12.))
            .child(
                div()
                    .w(px(96.))
                    .flex_none()
                    .text_color(hsla(theme.muted))
                    .child(key.to_owned()),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .child(value),
            )
    }

    fn mono(text: impl Into<SharedString>) -> gpui::Div {
        div().font_family(code_font_name()).child(text.into())
    }

    /// A section header: 28 px, uppercase caption, optional count.
    fn section(title: &str, count: Option<usize>, theme: &Theme) -> gpui::Div {
        div()
            .h(px(28.))
            .px(px(16.))
            .flex()
            .items_center()
            .border_t_1()
            .border_color(hsla(theme.border))
            .child(ui::section_label(title, theme))
            .child(div().flex_1())
            .when_some(count, |d, n| {
                d.child(
                    div()
                        .text_size(px(11.))
                        .text_color(hsla(theme.faint))
                        .child(n.to_string()),
                )
            })
    }

    fn proto_block(&self, text: &str, name: &str, theme: &Theme) -> gpui::Div {
        let t = *theme;
        let runs = prototype_runs(text, name, theme);
        // Break before the function name, as the design does.
        let (first, second) = match text.find(name).filter(|&at| at > 0 && !name.is_empty()) {
            Some(at) => (
                Some(text[..at].trim_end().to_owned()),
                text[at..].to_owned(),
            ),
            None => (None, text.to_owned()),
        };
        let styled = |piece: &str, offset: usize| {
            let len = piece.len();
            let hl: Vec<_> = runs
                .iter()
                .filter(|(r, _)| r.start >= offset && r.end <= offset + len)
                .map(|(r, h)| (r.start - offset..r.end - offset, *h))
                .collect();
            StyledText::new(piece.to_owned()).with_highlights(hl)
        };
        let second_offset = text.len() - second.len();
        div()
            .mx(px(16.))
            .mb(px(12.))
            .px(px(12.))
            .py(px(8.))
            .rounded(px(6.))
            .border_1()
            .border_color(hsla(t.border))
            .bg(hsla(t.surface))
            .font_family(code_font_name())
            .text_size(px(12.))
            .line_height(px(20.))
            .text_color(hsla(t.syntax.sym))
            .flex()
            .flex_col()
            .when_some(first.as_ref(), |d, f| d.child(styled(f, 0)))
            .child(styled(&second, second_offset))
    }

    fn xref_rows(
        &self,
        rows: &[XrefRow],
        limit: usize,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> gpui::Div {
        let t = *theme;
        div().px(px(8.)).pb(px(8.)).flex().flex_col().children(
            rows.iter().take(limit).enumerate().map(|(i, x)| {
                let from = x.from;
                div()
                    .id(("xref", i))
                    .h(px(24.))
                    .px(px(8.))
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .rounded(px(4.))
                    .cursor_pointer()
                    .text_size(px(12.))
                    .hover(move |d| d.bg(hsla(t.hover)))
                    .on_click(cx.listener(move |v, _, _, cx| v.go(from, cx)))
                    .on_mouse_down(
                        gpui::MouseButton::Right,
                        cx.listener(move |v, ev: &gpui::MouseDownEvent, _, cx| {
                            v.row_menu(from, None, ev.position, cx)
                        }),
                    )
                    .child(ui::xref_badge(&x.kind, &t))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_ellipsis()
                            .child(x.func.clone().unwrap_or_else(|| format!("{:X}", x.from))),
                    )
                    .child(
                        div()
                            .flex_none()
                            .font_family(code_font_name())
                            .text_size(px(11.))
                            .text_color(hsla(t.muted))
                            .child(format!("{:x}", x.from)),
                    )
            }),
        )
    }

    fn render_function(&self, theme: &Theme, cx: &mut Context<Self>) -> gpui::AnyElement {
        let t = *theme;
        let focus = self.focus.read(cx);
        let Some(f) = focus.func.clone() else {
            return self.render_item(theme, cx);
        };
        let callers = focus.func_xrefs.clone();
        let size = f.end.saturating_sub(f.start);
        let user = |on: bool, label: &str| {
            ui::badge(label.to_owned(), BadgeKind::Neutral, &t).when(!on, |d| d.opacity(0.9))
        };
        let mut flags: Vec<gpui::AnyElement> = Vec::new();
        for (flag, off) in [("lib", "not lib"), ("thunk", "not thunk")] {
            let on = f.flags.iter().any(|x| x == flag);
            flags.push(user(on, if on { flag } else { off }).into_any_element());
        }
        for flag in f
            .flags
            .iter()
            .filter(|x| !matches!(x.as_str(), "lib" | "thunk"))
        {
            flags.push(user(true, flag).into_any_element());
        }
        let kind = if f.flags.iter().any(|x| x == "lib") {
            "library code"
        } else {
            "user code"
        };
        let name = f.name.clone();
        div()
            .flex()
            .flex_col()
            .child(
                div()
                    .px(px(16.))
                    .pt(px(16.))
                    .pb(px(12.))
                    .child(
                        div()
                            .text_size(px(15.))
                            .line_height(px(20.))
                            .font_weight(gpui::FontWeight::MEDIUM)
                            .child(name.clone()),
                    )
                    .child(
                        div()
                            .text_size(px(12.))
                            .text_color(hsla(t.muted))
                            .child(format!("Function in {}, {kind}", f.segment)),
                    ),
            )
            .child(Self::section("Function", None, &t))
            .child(Self::kv("Start", Self::mono(hex(f.start)), &t))
            .child(Self::kv("End", Self::mono(hex(f.end)), &t))
            .child(Self::kv(
                "Size",
                Self::mono(format!("0x{size:x} ({size} bytes)")),
                &t,
            ))
            .child(Self::kv(
                "Segment",
                Self::mono(format!("{} ({}, {})", f.segment, f.seg_class, f.seg_perm)),
                &t,
            ))
            .child(Self::kv(
                "Convention",
                Self::mono(f.convention.clone())
                    .text_color(hsla(convention_color(&f.convention, &t))),
                &t,
            ))
            .child(Self::kv(
                "Basic blocks",
                Self::mono(f.blocks.to_string()),
                &t,
            ))
            .child(Self::kv("Locals", Self::mono(f.locals.to_string()), &t))
            .child(Self::kv("Flags", div().flex().gap(px(4.)).children(flags), &t).pb(px(4.)))
            .when(!f.prototype.is_empty(), |d| {
                d.child(Self::section("Prototype", None, &t)).child(
                    div()
                        .pt(px(4.))
                        .child(self.proto_block(&f.prototype, strip_args(&name), &t)),
                )
            })
            .child(Self::section("Xrefs to", Some(callers.len()), &t))
            .child(self.xref_rows(&callers, 12, &t, cx))
            .child(Self::section("Callees", Some(f.callees.len()), &t))
            .child(div().px(px(8.)).pb(px(8.)).flex().flex_col().children(
                f.callees.iter().take(40).enumerate().map(|(i, c)| {
                    let ea = c.ea;
                    let callee = c.name.clone();
                    div()
                        .id(("callee", i))
                        .h(px(24.))
                        .px(px(8.))
                        .flex()
                        .items_center()
                        .gap(px(8.))
                        .rounded(px(4.))
                        .cursor_pointer()
                        .text_size(px(12.))
                        .hover(move |d| d.bg(hsla(t.hover)))
                        .on_click(cx.listener(move |v, _, _, cx| v.go(ea, cx)))
                        .on_mouse_down(
                            gpui::MouseButton::Right,
                            cx.listener(move |v, ev: &gpui::MouseDownEvent, _, cx| {
                                v.row_menu(ea, Some(callee.clone()), ev.position, cx)
                            }),
                        )
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .overflow_hidden()
                                .whitespace_nowrap()
                                .text_ellipsis()
                                .text_color(hsla(if c.imported {
                                    t.syntax.imp_name
                                } else {
                                    t.syntax.code_name
                                }))
                                .child(c.name.clone()),
                        )
                        .child(
                            div()
                                .flex_none()
                                .font_family(code_font_name())
                                .text_size(px(11.))
                                .text_color(hsla(t.muted))
                                .child(format!("{:x}", c.ea)),
                        )
                }),
            ))
            .into_any_element()
    }

    /// An address that is not in a function: a name, a data item.
    fn render_item(&self, theme: &Theme, cx: &mut Context<Self>) -> gpui::AnyElement {
        let t = *theme;
        let focus = self.focus.read(cx);
        let Some(a) = focus.addr.clone() else {
            return div()
                .p(px(16.))
                .text_color(hsla(t.faint))
                .child("Nothing selected")
                .into_any_element();
        };
        div()
            .flex()
            .flex_col()
            .child(
                div()
                    .px(px(16.))
                    .pt(px(16.))
                    .pb(px(12.))
                    .child(
                        div()
                            .text_size(px(15.))
                            .line_height(px(20.))
                            .font_weight(gpui::FontWeight::MEDIUM)
                            .child(a.name.clone().unwrap_or_else(|| hex(a.head))),
                    )
                    .child(
                        div()
                            .text_size(px(12.))
                            .text_color(hsla(t.muted))
                            .child(format!(
                                "{} item in {}",
                                a.kind,
                                a.segment.clone().unwrap_or_default()
                            )),
                    ),
            )
            .child(Self::section("Item", None, &t))
            .child(Self::kv("Address", Self::mono(hex(a.head)), &t))
            .child(Self::kv(
                "Segment",
                Self::mono(a.segment.clone().unwrap_or_default()),
                &t,
            ))
            .child(Self::kv("Kind", Self::mono(a.kind.clone()), &t))
            .when_some(a.file_offset, |d, o| {
                d.child(Self::kv("File offset", Self::mono(hex(o)), &t))
            })
            .into_any_element()
    }

    fn render_block(
        &self,
        b: BlockInfo,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let t = *theme;
        let focus = self.focus.read(cx);
        let func_name = focus
            .func
            .as_ref()
            .map(|f| f.name.clone())
            .unwrap_or_default();
        let segment = focus
            .func
            .as_ref()
            .map(|f| f.segment.clone())
            .unwrap_or_default();
        let size = b.end.saturating_sub(b.start);
        let flow_badge = |k: EdgeKind| match k {
            EdgeKind::Taken => ui::badge("taken", BadgeKind::Success, &t),
            EdgeKind::NotTaken => ui::badge("not taken", BadgeKind::Danger, &t),
            EdgeKind::Always => ui::badge("always", BadgeKind::Count, &t),
        };
        let edge_rows =
            |prefix: &'static str, rows: &[(EdgeKind, usize, Ea)], cx: &mut Context<Self>| {
                div()
                    .px(px(8.))
                    .pb(px(8.))
                    .flex()
                    .flex_col()
                    .children(rows.iter().enumerate().map(|(i, (k, idx, ea))| {
                        let ea = *ea;
                        div()
                            .id((prefix, i))
                            .h(px(24.))
                            .px(px(8.))
                            .flex()
                            .items_center()
                            .gap(px(8.))
                            .rounded(px(4.))
                            .cursor_pointer()
                            .text_size(px(12.))
                            .hover(move |d| d.bg(hsla(t.hover)))
                            .on_click(cx.listener(move |v, _, _, cx| v.go(ea, cx)))
                            .on_mouse_down(
                                gpui::MouseButton::Right,
                                cx.listener(move |v, ev: &gpui::MouseDownEvent, _, cx| {
                                    v.row_menu(ea, None, ev.position, cx)
                                }),
                            )
                            .child(flow_badge(*k))
                            .child(div().flex_1().child(format!("Block #{idx}")))
                            .child(
                                div()
                                    .font_family(code_font_name())
                                    .text_size(px(11.))
                                    .text_color(hsla(t.muted))
                                    .child(format!("{ea:x}")),
                            )
                    }))
            };
        let conditional = b.succs.len() > 1;
        let exit = b.succs.is_empty();
        div()
            .flex()
            .flex_col()
            .child(
                div()
                    .px(px(16.))
                    .pt(px(16.))
                    .pb(px(12.))
                    .child(
                        div()
                            .text_size(px(15.))
                            .line_height(px(20.))
                            .font_weight(gpui::FontWeight::MEDIUM)
                            .child(format!("Basic block #{}", b.index)),
                    )
                    .child(
                        div()
                            .text_size(px(12.))
                            .text_color(hsla(t.muted))
                            .child(format!("In {func_name}, {segment}")),
                    ),
            )
            .child(Self::section("Block", None, &t))
            .child(Self::kv("Start", Self::mono(hex(b.start)), &t))
            .child(Self::kv("End", Self::mono(hex(b.end)), &t))
            .child(Self::kv(
                "Size",
                Self::mono(format!("0x{size:x} ({size} bytes)")),
                &t,
            ))
            .child(Self::kv(
                "Instructions",
                Self::mono(b.insns.to_string()),
                &t,
            ))
            .child(Self::kv(
                "Terminator",
                Self::mono(b.terminator.clone()).text_color(hsla(t.syntax.insn)),
                &t,
            ))
            .child(
                Self::kv(
                    "Flags",
                    div()
                        .flex()
                        .gap(px(4.))
                        .child(ui::badge(
                            if conditional {
                                "conditional"
                            } else {
                                "unconditional"
                            },
                            BadgeKind::Neutral,
                            &t,
                        ))
                        .child(ui::badge(
                            if exit { "exit" } else { "not exit" },
                            BadgeKind::Neutral,
                            &t,
                        )),
                    &t,
                )
                .pb(px(4.)),
            )
            .child(Self::section("Successors", Some(b.succs.len()), &t))
            .child(edge_rows("succ", &b.succs, cx))
            .child(Self::section("Predecessors", Some(b.preds.len()), &t))
            .child(edge_rows("pred", &b.preds, cx))
            .child(Self::section("Calls in block", Some(b.calls.len()), &t))
            .child(div().px(px(8.)).pb(px(8.)).flex().flex_col().children(
                b.calls.iter().enumerate().map(|(i, (ea, target))| {
                    let ea = *ea;
                    div()
                        .id(("call", i))
                        .h(px(24.))
                        .px(px(8.))
                        .flex()
                        .items_center()
                        .gap(px(8.))
                        .rounded(px(4.))
                        .cursor_pointer()
                        .text_size(px(12.))
                        .text_color(hsla(t.syntax.code_name))
                        .hover(move |d| d.bg(hsla(t.hover)))
                        .on_click(cx.listener(move |v, _, _, cx| v.go(ea, cx)))
                        .on_mouse_down(
                            gpui::MouseButton::Right,
                            cx.listener(move |v, ev: &gpui::MouseDownEvent, _, cx| {
                                v.row_menu(ea, None, ev.position, cx)
                            }),
                        )
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .overflow_hidden()
                                .whitespace_nowrap()
                                .text_ellipsis()
                                .child(target.clone()),
                        )
                        .child(
                            div()
                                .font_family(code_font_name())
                                .text_size(px(11.))
                                .text_color(hsla(t.muted))
                                .child(format!("{ea:x}")),
                        )
                }),
            ))
            .child(Self::section("Function", None, &t))
            .child(Self::kv(
                "Name",
                div().text_color(hsla(t.syntax.code_name)).child(func_name),
                &t,
            ))
            .child(Self::kv("Blocks", Self::mono(b.blocks.to_string()), &t))
            .child(Self::kv("Edges", Self::mono(b.edges.to_string()), &t))
            .child(Self::kv("Back edges", Self::mono(b.back_edges.to_string()), &t).pb(px(8.)))
            .into_any_element()
    }

    fn render_type(&self, theme: &Theme, cx: &mut Context<Self>) -> gpui::AnyElement {
        let t = *theme;
        let focus = self.focus.read(cx);
        let name = focus
            .func
            .as_ref()
            .map(|f| f.name.clone())
            .or_else(|| focus.addr.as_ref().and_then(|a| a.name.clone()))
            .unwrap_or_default();
        let text = focus.type_text.clone();
        let frame = focus
            .func
            .as_ref()
            .map(|f| (f.locals, f.frame_size, f.convention.clone()));
        div()
            .flex()
            .flex_col()
            .child(
                div()
                    .px(px(16.))
                    .pt(px(16.))
                    .pb(px(12.))
                    .child(
                        div()
                            .text_size(px(15.))
                            .line_height(px(20.))
                            .font_weight(gpui::FontWeight::MEDIUM)
                            .child(name.clone()),
                    )
                    .child(
                        div()
                            .text_size(px(12.))
                            .text_color(hsla(t.muted))
                            .child("Declared type"),
                    ),
            )
            .child(Self::section("Declaration", None, &t))
            .child(if text.is_empty() {
                div()
                    .px(px(16.))
                    .py(px(8.))
                    .text_size(px(12.))
                    .text_color(hsla(t.faint))
                    .child("No type is set for this item.")
                    .into_any_element()
            } else {
                div()
                    .pt(px(4.))
                    .child(self.proto_block(&text, &name, &t))
                    .into_any_element()
            })
            .when_some(frame, |d, (locals, size, cc)| {
                d.child(Self::section("Frame", None, &t))
                    .child(Self::kv(
                        "Convention",
                        Self::mono(cc.clone()).text_color(hsla(convention_color(&cc, &t))),
                        &t,
                    ))
                    .child(Self::kv("Locals", Self::mono(locals.to_string()), &t))
                    .child(Self::kv(
                        "Frame size",
                        Self::mono(format!("0x{size:x}")),
                        &t,
                    ))
            })
            .child(
                div()
                    .px(px(16.))
                    .py(px(12.))
                    .text_size(px(11.))
                    .text_color(hsla(t.muted))
                    .child("Press Y in a code view to change the type."),
            )
            .into_any_element()
    }

    fn render_xrefs(&self, theme: &Theme, cx: &mut Context<Self>) -> gpui::AnyElement {
        let t = *theme;
        let focus = self.focus.read(cx);
        let rows = focus.target_xrefs.clone();
        let name = focus.target_name.clone();
        div()
            .flex()
            .flex_col()
            .child(
                div()
                    .px(px(16.))
                    .pt(px(16.))
                    .pb(px(12.))
                    .child(
                        div()
                            .text_size(px(15.))
                            .line_height(px(20.))
                            .font_weight(gpui::FontWeight::MEDIUM)
                            .child(name),
                    )
                    .child(
                        div()
                            .text_size(px(12.))
                            .text_color(hsla(t.muted))
                            .child("Cross references to the item under the caret"),
                    ),
            )
            .child(Self::section("Xrefs to", Some(rows.len()), &t))
            .child(self.xref_rows(&rows, 200, &t, cx))
            .into_any_element()
    }

    fn render_comments(&self, theme: &Theme, cx: &mut Context<Self>) -> gpui::AnyElement {
        let t = *theme;
        let focus = self.focus.read(cx);
        let head = focus.addr.as_ref().map(|a| a.head).unwrap_or(0);
        let (c, r) = (focus.comment.clone(), focus.repeatable_comment.clone());
        let block = |text: String, empty: &str| {
            div()
                .mx(px(16.))
                .mb(px(12.))
                .px(px(12.))
                .py(px(8.))
                .rounded(px(6.))
                .border_1()
                .border_color(hsla(t.border))
                .bg(hsla(t.surface))
                .text_size(px(12.))
                .line_height(px(18.))
                .when(text.is_empty(), |d| d.text_color(hsla(t.faint)))
                .when(!text.is_empty(), |d| {
                    d.font_family(code_font_name())
                        .text_color(hsla(t.syntax.cmt))
                })
                .child(if text.is_empty() {
                    empty.to_owned()
                } else {
                    text
                })
        };
        div()
            .flex()
            .flex_col()
            .child(
                div()
                    .px(px(16.))
                    .pt(px(16.))
                    .pb(px(12.))
                    .child(
                        div()
                            .text_size(px(15.))
                            .line_height(px(20.))
                            .font_weight(gpui::FontWeight::MEDIUM)
                            .child("Comments"),
                    )
                    .child(
                        div()
                            .text_size(px(12.))
                            .text_color(hsla(t.muted))
                            .child(format!("At {}", hex(head))),
                    ),
            )
            .child(Self::section("Comment", None, &t))
            .child(
                div()
                    .pt(px(4.))
                    .child(block(c, "No comment. Press ; to add one.")),
            )
            .child(Self::section("Repeatable comment", None, &t))
            .child(
                div()
                    .pt(px(4.))
                    .child(block(r, "No repeatable comment. Press : to add one.")),
            )
            .into_any_element()
    }
}

impl Render for Inspector {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::get(cx);
        let t = theme;
        let ih = scrollbar::handle("inspector-h", window, cx);
        let ibar = scrollbar::scrollbar(
            "inspector-scroll",
            Source::handle(&ih),
            Axes::V,
            &theme,
            window,
            cx,
        );
        let body = match self.tab {
            InspectorTab::Info => match self
                .block_mode
                .then(|| self.graph.read(cx).block_info())
                .flatten()
            {
                Some(b) => self.render_block(b, &t, cx),
                None => self.render_function(&t, cx),
            },
            InspectorTab::Type => self.render_type(&t, cx),
            InspectorTab::Xrefs => self.render_xrefs(&t, cx),
            InspectorTab::Comments => self.render_comments(&t, cx),
        };
        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(hsla(t.surface2))
            .text_color(hsla(t.text))
            .child(
                div()
                    .h(px(32.))
                    .flex_none()
                    .flex()
                    .items_center()
                    .gap(px(2.))
                    .px(px(8.))
                    .border_b_1()
                    .border_color(hsla(t.border))
                    .children(TABS.iter().map(|&(tab, _, label)| {
                        let on = tab == self.tab;
                        div()
                            .id(label)
                            .h(px(24.))
                            .px(px(12.))
                            .flex()
                            .items_center()
                            .rounded(px(4.))
                            .text_size(px(12.))
                            .font_weight(gpui::FontWeight::MEDIUM)
                            .cursor_pointer()
                            .text_color(hsla(if on { t.accent } else { t.muted }))
                            .when(on, |d| d.bg(hsla(t.accent_soft)))
                            .when(!on, |d| d.hover(move |d| d.bg(hsla(t.hover))))
                            .on_click(cx.listener(move |v, _, _, cx| {
                                v.tab = tab;
                                cx.emit(InspectorEvent::TabChanged);
                                cx.notify();
                            }))
                            .child(label)
                    })),
            )
            .child(
                div()
                    .relative()
                    .flex_1()
                    .min_h_0()
                    .child(
                        div()
                            .id("inspector-body")
                            .size_full()
                            .overflow_y_scroll()
                            .track_scroll(&ih)
                            .child(body),
                    )
                    .child(ibar),
            )
    }
}

/// A known calling convention reads as a keyword; "unknown" is a plain fact,
/// not an error, so it stays muted.
fn convention_color(cc: &str, t: &Theme) -> u32 {
    if cc.is_empty() || cc.eq_ignore_ascii_case("unknown") {
        t.muted
    } else {
        t.syntax.kw
    }
}
