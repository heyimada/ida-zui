// One overlay type for every quick interaction: a text prompt (rename,
// comment, ...) or the filterable pick list behind jump-anywhere, xrefs and
// the command palette.
use crate::actions::{Cancel, Confirm, SelectNext, SelectPrev};
use crate::input::{InputEvent, TextInput};
use crate::scrollbar::{self, Axes, Source};
use crate::theme::{Theme, code_font_name, hsla};
use crate::ui::{self, BadgeKind, ButtonKind, icon, icon_button};
use gpui::{
    Action, App, Context, Entity, EventEmitter, FocusHandle, Focusable, HighlightStyle,
    ScrollHandle, SharedString, StyledText, Subscription, Window, div, prelude::*, px,
};
use ida_proto::Ea;
use std::ops::Range;
use std::rc::Rc;

pub enum ChoiceValue {
    Ea(Ea),
    Action(Box<dyn Action>),
    /// Open an editor tab.
    Doc(crate::docs::DocKey),
    /// Run this with whatever it captured (a chooser whose rows do something
    /// of their own, such as picking a structure for an operand).
    Run(Rc<dyn Fn(&mut App)>),
}

impl Clone for ChoiceValue {
    fn clone(&self) -> Self {
        match self {
            Self::Ea(e) => Self::Ea(*e),
            Self::Action(a) => Self::Action(a.boxed_clone()),
            Self::Doc(k) => Self::Doc(k.clone()),
            Self::Run(f) => Self::Run(f.clone()),
        }
    }
}

#[derive(Clone)]
pub struct Choice {
    pub label: SharedString,
    /// Right-aligned mono note: an address or a shortcut.
    pub detail: SharedString,
    /// Kind badge: `fn`, `name`, `str`, `seg`, or an xref kind.
    pub kind: SharedString,
    /// Group header this choice sits under; empty for ungrouped lists.
    pub group: SharedString,
    /// Extra search terms that are not displayed.
    pub keywords: SharedString,
    pub value: ChoiceValue,
}

pub enum ModalEvent {
    Dismiss,
    Pick(ChoiceValue),
    /// Free text submitted (no choice picked).
    Submit(String),
}

/// What the overlay looks like.
#[derive(Clone)]
pub enum Style {
    /// Search palette with a result list.
    Palette {
        /// What Enter does, for the footer: "jump", "run", "open".
        enter: &'static str,
        /// A shortcut shown beside the result count, e.g. `Ctrl+G`.
        key_hint: Option<SharedString>,
    },
    /// A text prompt with a title and a confirm button.
    Prompt {
        confirm: SharedString,
        validate_name: bool,
    },
}

pub struct Modal {
    title: SharedString,
    input: Entity<TextInput>,
    choices: Rc<Vec<Choice>>,
    filtered: Vec<usize>,
    selected: usize,
    /// Text prompts submit the input; pickers pick the selection.
    prefer_text: bool,
    initial: String,
    style: Style,
    scroll: ScrollHandle,
    focus: FocusHandle,
    _sub: Subscription,
}

impl EventEmitter<ModalEvent> for Modal {}

/// Results shown at most; the list is rebuilt on every keystroke.
const MAX_RESULTS: usize = 80;

/// Byte range of the first case-insensitive occurrence of `q` in `text`.
pub fn match_range(text: &str, q: &str) -> Option<Range<usize>> {
    if q.is_empty() {
        return None;
    }
    let (lt, lq) = (text.to_lowercase(), q.to_lowercase());
    // Lowercasing can change byte lengths for exotic characters; only trust
    // the offsets when it did not.
    if lt.len() != text.len() {
        return None;
    }
    let at = lt.find(&lq)?;
    Some(at..at + lq.len())
}

/// Why `name` cannot be a name in the database, if it cannot.
pub fn name_problem(name: &str) -> Option<&'static str> {
    if name.is_empty() {
        None // an empty name removes the custom name
    } else if name.chars().any(|c| c.is_whitespace() || c.is_control()) {
        Some("Names cannot contain spaces")
    } else if name.chars().next().is_some_and(|c| c.is_ascii_digit()) {
        Some("A name cannot start with a digit")
    } else {
        None
    }
}

impl Modal {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        title: impl Into<SharedString>,
        initial: &str,
        placeholder: &str,
        choices: Rc<Vec<Choice>>,
        prefer_text: bool,
        style: Style,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let prompt = matches!(style, Style::Prompt { .. });
        let input = cx.new(|cx| {
            let i = TextInput::new(initial, placeholder, cx);
            if prompt { i.tall() } else { i.bare(15.) }
        });
        let sub = cx.subscribe(&input, |this, _, ev: &InputEvent, cx| match ev {
            InputEvent::Changed => {
                this.refilter(cx);
            }
        });
        input.update(cx, |i, cx| window.focus(&i.focus_handle(cx), cx));
        let mut me = Self {
            title: title.into(),
            input,
            choices,
            filtered: Vec::new(),
            selected: 0,
            prefer_text,
            initial: initial.to_owned(),
            style,
            scroll: ScrollHandle::new(),
            focus: cx.focus_handle(),
            _sub: sub,
        };
        if prefer_text {
            me.filtered = (0..me.choices.len()).collect();
        } else {
            me.refilter(cx);
        }
        me
    }

    fn refilter(&mut self, cx: &mut Context<Self>) {
        let q = self.input.read(cx).value().trim().to_lowercase();
        self.filtered.clear();
        if q.is_empty() {
            self.filtered.extend(0..self.choices.len().min(MAX_RESULTS));
        } else {
            // An exact match first (typing `main` must land on `main`), then
            // prefix matches, then substring matches.
            let mut exact = Vec::new();
            let mut prefix = Vec::new();
            let mut sub = Vec::new();
            for (i, c) in self.choices.iter().enumerate() {
                let l = c.label.to_lowercase();
                if l == q {
                    exact.push(i);
                } else if l.starts_with(&q) {
                    prefix.push(i);
                } else if l.contains(&q)
                    || c.detail.to_lowercase().contains(&q)
                    || c.keywords.contains(q.as_str())
                {
                    sub.push(i);
                }
            }
            self.filtered.extend(exact);
            self.filtered.extend(prefix);
            self.filtered.extend(sub);
            self.filtered.truncate(MAX_RESULTS * 4);
        }
        // Group the results under their headers, keeping the order inside a group.
        if self.choices.iter().any(|c| !c.group.is_empty()) {
            let mut order: Vec<&SharedString> = Vec::new();
            for &i in &self.filtered {
                if !order.contains(&&self.choices[i].group) {
                    order.push(&self.choices[i].group);
                }
            }
            let mut grouped = Vec::with_capacity(self.filtered.len());
            for g in order {
                grouped.extend(
                    self.filtered
                        .iter()
                        .copied()
                        .filter(|&i| &self.choices[i].group == g)
                        .take(MAX_RESULTS),
                );
            }
            self.filtered = grouped;
        } else {
            self.filtered.truncate(MAX_RESULTS);
        }
        self.selected = 0;
        self.scroll = ScrollHandle::new();
        cx.notify();
    }

    fn confirm(&mut self, _: &Confirm, _: &mut Window, cx: &mut Context<Self>) {
        let text = self.input.read(cx).value().to_owned();
        let pick = self
            .filtered
            .get(self.selected)
            .map(|&i| self.choices[i].value.clone());
        let t = text.trim().trim_start_matches("0x");
        let looks_like_address =
            !t.is_empty() && t.len() >= 4 && t.chars().all(|c| c.is_ascii_hexdigit());
        let typed_choice = !self.prefer_text && !self.filtered.is_empty() && !looks_like_address;
        if let Style::Prompt {
            validate_name: true,
            ..
        } = self.style
            && text.trim() != self.initial.trim()
            && name_problem(text.trim()).is_some()
        {
            return;
        }
        match pick {
            Some(v) if typed_choice => cx.emit(ModalEvent::Pick(v)),
            _ => cx.emit(ModalEvent::Submit(text)),
        }
    }

    fn cancel(&mut self, _: &Cancel, _: &mut Window, cx: &mut Context<Self>) {
        cx.emit(ModalEvent::Dismiss);
    }

    fn step(&mut self, delta: isize, cx: &mut Context<Self>) {
        if self.filtered.is_empty() {
            return;
        }
        let n = self.filtered.len() as isize;
        self.selected = ((self.selected as isize + delta).rem_euclid(n)) as usize;
        self.prefer_text = false;
        // Child index in the list: one header per group before the row.
        let mut child = self.selected;
        let mut last: Option<&SharedString> = None;
        for &i in self.filtered.iter().take(self.selected + 1) {
            let g = &self.choices[i].group;
            if !g.is_empty() && last != Some(g) {
                child += 1;
            }
            last = Some(g);
        }
        self.scroll.scroll_to_item(child);
        cx.notify();
    }

    fn render_prompt(
        &mut self,
        theme: &Theme,
        confirm: &SharedString,
        validate: bool,
        cx: &mut Context<Self>,
    ) -> gpui::Div {
        let text = self.input.read(cx).value().trim().to_owned();
        let problem = if validate && text != self.initial.trim() {
            name_problem(&text)
        } else {
            None
        };
        let t = *theme;
        div()
            .w(px(480.))
            .flex()
            .flex_col()
            .child(
                div()
                    .h(px(48.))
                    .flex_none()
                    .flex()
                    .items_center()
                    .pl(px(16.))
                    .pr(px(12.))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_size(px(15.))
                            .font_weight(gpui::FontWeight::MEDIUM)
                            .child(self.title.clone()),
                    )
                    .child(
                        icon_button("prompt-close", "close", 28., false, true, &t)
                            .on_click(cx.listener(|_, _, _, cx| cx.emit(ModalEvent::Dismiss))),
                    ),
            )
            .child(div().px(px(16.)).child(self.input.clone()))
            .when(validate, |d| {
                d.child(
                    div()
                        .h(px(32.))
                        .px(px(16.))
                        .flex()
                        .items_center()
                        .gap(px(8.))
                        .text_size(px(12.))
                        .text_color(hsla(if problem.is_some() {
                            t.danger
                        } else {
                            t.success
                        }))
                        .child(icon(
                            if problem.is_some() {
                                "warning"
                            } else {
                                "check"
                            },
                            16.,
                            hsla(if problem.is_some() {
                                t.danger
                            } else {
                                t.success
                            }),
                        ))
                        .child(problem.unwrap_or("Valid name")),
                )
            })
            .child(
                div()
                    .h(px(56.))
                    .flex_none()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .px(px(16.))
                    .child(
                        div()
                            .flex_1()
                            .text_size(px(12.))
                            .text_color(hsla(t.muted))
                            .child("Enter to apply · Esc to cancel"),
                    )
                    .child(
                        ui::button("prompt-cancel", "Cancel", ButtonKind::Ghost, false, &t)
                            .on_click(cx.listener(|_, _, _, cx| cx.emit(ModalEvent::Dismiss))),
                    )
                    .child(
                        ui::button("prompt-ok", confirm.clone(), ButtonKind::Primary, false, &t)
                            .on_click(cx.listener(|m, _, window, cx| {
                                m.confirm(&Confirm, window, cx);
                            })),
                    ),
            )
    }

    fn render_palette(
        &mut self,
        theme: &Theme,
        enter: &'static str,
        key_hint: Option<SharedString>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> gpui::Div {
        let mbar = scrollbar::scrollbar(
            "modal-scroll",
            Source::handle(&self.scroll),
            Axes::V,
            theme,
            window,
            cx,
        );
        let t = *theme;
        let q = self.input.read(cx).value().trim().to_owned();
        let count = self.filtered.len();
        let mut rows: Vec<gpui::AnyElement> = Vec::new();
        let mut last: Option<SharedString> = None;
        for (ix, &ci) in self.filtered.iter().enumerate() {
            let c = &self.choices[ci];
            if !c.group.is_empty() && last.as_ref() != Some(&c.group) {
                let n = self
                    .filtered
                    .iter()
                    .filter(|&&j| self.choices[j].group == c.group)
                    .count();
                rows.push(
                    div()
                        .h(px(28.))
                        .px(px(16.))
                        .flex()
                        .items_center()
                        .text_size(px(11.))
                        .font_weight(gpui::FontWeight::MEDIUM)
                        .text_color(hsla(t.muted))
                        .child(div().flex_1().child(c.group.to_uppercase()))
                        .child(
                            div()
                                .font_weight(gpui::FontWeight::NORMAL)
                                .child(n.to_string()),
                        )
                        .into_any_element(),
                );
            }
            last = Some(c.group.clone());
            let selected = ix == self.selected;
            let value = c.value.clone();
            let label = match match_range(&c.label, &q) {
                Some(r) => StyledText::new(c.label.clone()).with_highlights(vec![(
                    r,
                    HighlightStyle {
                        color: Some(hsla(t.accent)),
                        font_weight: Some(gpui::FontWeight::MEDIUM),
                        ..Default::default()
                    },
                )]),
                None => StyledText::new(c.label.clone()),
            };
            rows.push(
                div()
                    .id(("choice", ix))
                    .mx(px(4.))
                    .h(px(32.))
                    .px(px(12.))
                    .flex()
                    .items_center()
                    .gap(px(12.))
                    .rounded(px(4.))
                    .cursor_pointer()
                    .when(selected, |d| d.bg(hsla(t.accent_soft)))
                    .when(!selected, |d| d.hover(move |d| d.bg(hsla(t.hover))))
                    .on_click(
                        cx.listener(move |_, _, _, cx| cx.emit(ModalEvent::Pick(value.clone()))),
                    )
                    .when(!c.kind.is_empty(), |d| {
                        let b = if c.kind.len() <= 4
                            && c.kind.chars().all(|ch| ch.is_ascii_lowercase())
                            && matches!(
                                &*c.kind,
                                "call" | "jmp" | "read" | "write" | "offs" | "info" | "flow"
                            ) {
                            ui::badge(c.kind.clone(), BadgeKind::Xref, &t)
                        } else {
                            ui::badge(c.kind.clone(), BadgeKind::Neutral, &t).min_w(px(28.))
                        };
                        d.child(b)
                    })
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_ellipsis()
                            .text_size(px(13.))
                            .text_color(hsla(t.text))
                            .child(label),
                    )
                    .child(
                        div()
                            .flex_none()
                            .font_family(code_font_name())
                            .text_size(px(11.))
                            .text_color(hsla(t.muted))
                            .whitespace_nowrap()
                            .child(c.detail.clone()),
                    )
                    .into_any_element(),
            );
        }
        let results = format!("{count} result{}", if count == 1 { "" } else { "s" });
        let hint = |keys: &str, what: &str| {
            div()
                .flex()
                .items_center()
                .gap(px(6.))
                .child(ui::key_cap(keys.to_owned(), &t))
                .child(what.to_owned())
        };
        div()
            .w(px(640.))
            .flex()
            .flex_col()
            // The noun: what this list is a list of (the footer has the verb).
            .child(
                div()
                    .h(px(32.))
                    .flex_none()
                    .flex()
                    .items_center()
                    .px(px(16.))
                    .text_size(px(12.))
                    .font_weight(gpui::FontWeight::MEDIUM)
                    .text_color(hsla(t.muted))
                    .whitespace_nowrap()
                    .overflow_hidden()
                    .text_ellipsis()
                    .child(self.title.clone()),
            )
            .child(
                div()
                    .h(px(44.))
                    .flex_none()
                    .flex()
                    .items_center()
                    .gap(px(12.))
                    .px(px(16.))
                    .border_b_1()
                    .border_color(hsla(t.border))
                    .child(icon("search", 16., hsla(t.muted)))
                    .child(div().flex_1().min_w_0().child(self.input.clone()))
                    .child(
                        div()
                            .flex_none()
                            .flex()
                            .items_center()
                            .gap(px(8.))
                            .text_size(px(11.))
                            .text_color(hsla(t.muted))
                            .child(results)
                            .when_some(key_hint, |d, k| d.child(ui::key_cap(k, &t))),
                    ),
            )
            .child(
                div()
                    .relative()
                    .max_h(px(32. * 9. + 56.))
                    .child(
                        div()
                            .id("modal-results")
                            .max_h(px(32. * 9. + 56.))
                            .overflow_y_scroll()
                            .track_scroll(&self.scroll)
                            .py(px(4.))
                            .flex()
                            .flex_col()
                            .children(rows)
                            .when(count == 0, |d| {
                                d.child(
                                    div()
                                        .h(px(40.))
                                        .px(px(16.))
                                        .flex()
                                        .items_center()
                                        .text_color(hsla(t.faint))
                                        .child("No matches"),
                                )
                            }),
                    )
                    .child(mbar),
            )
            .child(
                div()
                    .h(px(28.))
                    .flex_none()
                    .flex()
                    .items_center()
                    .gap(px(16.))
                    .px(px(12.))
                    .border_t_1()
                    .border_color(hsla(t.border))
                    .text_size(px(11.))
                    .text_color(hsla(t.muted))
                    .child(hint("↑↓", "select"))
                    .child(hint("Enter", enter))
                    .child(hint("Esc", "close")),
            )
    }
}

impl Focusable for Modal {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.input.read(cx).focus_handle(cx)
    }
}

impl Render for Modal {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::get(cx);
        let inner = match self.style.clone() {
            Style::Palette { enter, key_hint } => {
                self.render_palette(&theme, enter, key_hint, window, cx)
            }
            Style::Prompt {
                confirm,
                validate_name,
            } => self.render_prompt(&theme, &confirm, validate_name, cx),
        };
        div()
            .key_context("Modal")
            .track_focus(&self.focus)
            .on_action(cx.listener(Self::confirm))
            .on_action(cx.listener(Self::cancel))
            .on_action(cx.listener(|m, _: &SelectPrev, _, cx| m.step(-1, cx)))
            .on_action(cx.listener(|m, _: &SelectNext, _, cx| m.step(1, cx)))
            .rounded(px(8.))
            .overflow_hidden()
            .bg(hsla(theme.popover))
            .border_1()
            .border_color(hsla(theme.border_strong))
            .shadow(ui::drop_shadow(&theme, 16., 48.))
            .text_color(hsla(theme.text))
            .text_size(px(13.))
            .child(inner)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn match_range_is_case_insensitive_and_first_occurrence() {
        assert_eq!(
            match_range("FIND::SearchFiles(void)", "search"),
            Some(6..12)
        );
        assert_eq!(match_range("aXa", "a"), Some(0..1));
        assert_eq!(match_range("abc", ""), None);
        assert_eq!(match_range("abc", "zz"), None);
    }

    #[test]
    fn names_with_spaces_or_a_leading_digit_are_flagged() {
        assert_eq!(name_problem("FIND::SearchFiles"), None);
        assert_eq!(name_problem(""), None);
        assert!(name_problem("two words").is_some());
        assert!(name_problem("1abc").is_some());
        assert!(name_problem("tab\there").is_some());
    }
}
