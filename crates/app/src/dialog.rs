// Shared pieces of the in-app dialogs (gpui-rendered, so they look the same
// everywhere and automation can drive them), plus the question dialog that
// replaces native message boxes.
use crate::actions::{Cancel, Confirm};
use crate::open_dialog::OpenRequest;
use crate::theme::{Theme, hsla};
use crate::ui::{self, icon, icon_button};
use gpui::{
    App, ClickEvent, Context, EventEmitter, FocusHandle, Focusable, SharedString, Stateful, Window,
    div, prelude::*, px,
};

pub use crate::ui::ButtonKind;

/// What a dialog tells the workspace.
pub enum DialogEvent {
    Close,
    /// A question was answered with this button (0-based).
    Answer(usize),
    Open(OpenRequest),
    /// Run this Python file in the console.
    RunScript(std::path::PathBuf),
}

/// The dialog card: `popover`, 1 px `border_strong`, radius 12, drop shadow.
pub fn frame(theme: &Theme, width: f32) -> gpui::Div {
    div()
        .w(px(width))
        .max_h_full()
        .flex()
        .flex_col()
        .overflow_hidden()
        .rounded(px(12.))
        .bg(hsla(theme.popover))
        .border_1()
        .border_color(hsla(theme.border_strong))
        .shadow(ui::drop_shadow(theme, 16., 48.))
        .text_color(hsla(theme.text))
        .text_size(px(13.))
        .line_height(px(20.))
}

/// The 48 px header: title (15 / 600) and a close button.
pub fn header(
    theme: &Theme,
    title: impl Into<SharedString>,
    on_close: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> gpui::Div {
    div()
        .h(px(48.))
        .flex_none()
        .flex()
        .items_center()
        .pl(px(16.))
        .pr(px(12.))
        .border_b_1()
        .border_color(hsla(theme.border))
        .child(
            div()
                .flex_1()
                .text_size(px(15.))
                .line_height(px(20.))
                .font_weight(gpui::FontWeight::SEMIBOLD)
                .child(title.into()),
        )
        .child(icon_button("dialog-close", "close", 28., false, true, theme).on_click(on_close))
}

/// The body: padded, scrolls when the card is height-limited.
pub fn body() -> gpui::Div {
    div()
        .flex_1()
        .min_h_0()
        .flex()
        .flex_col()
        .gap(px(12.))
        .p(px(16.))
        .px(px(24.))
}

/// The 56 px footer: hairline above, buttons right-aligned, primary last.
pub fn footer(theme: &Theme) -> gpui::Div {
    div()
        .h(px(56.))
        .flex_none()
        .flex()
        .items_center()
        .justify_end()
        .gap(px(8.))
        .px(px(24.))
        .border_t_1()
        .border_color(hsla(theme.border))
}

pub fn button(
    id: impl Into<gpui::ElementId>,
    label: impl Into<SharedString>,
    kind: ButtonKind,
    theme: &Theme,
) -> Stateful<gpui::Div> {
    ui::button(id, label, kind, false, theme)
}

/// A 16 px checkbox with a label; `accent` fill with a check when on.
pub fn checkbox(
    id: impl Into<gpui::ElementId>,
    label: impl Into<SharedString>,
    checked: bool,
    theme: &Theme,
) -> Stateful<gpui::Div> {
    div()
        .id(id)
        .flex()
        .items_center()
        .gap(px(8.))
        .cursor_pointer()
        .child(
            div()
                .size(px(16.))
                .flex_none()
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(4.))
                .border_1()
                .border_color(hsla(if checked {
                    theme.accent
                } else {
                    theme.border_strong
                }))
                .bg(hsla(if checked { theme.accent } else { theme.bg }))
                .when(checked, |d| {
                    d.child(icon("check", 12., hsla(theme.on_accent)))
                }),
        )
        .child(label.into())
}

pub fn caption(theme: &Theme, text: impl Into<SharedString>) -> gpui::Div {
    div()
        .text_size(px(12.))
        .line_height(px(16.))
        .text_color(hsla(theme.muted))
        .child(text.into())
}

/// A question with buttons. Number keys, Enter (the default button) and Esc
/// (the cancel button) answer it, as do clicks.
pub struct ConfirmDialog {
    title: SharedString,
    detail: SharedString,
    buttons: Vec<SharedString>,
    default: usize,
    cancel: usize,
    focus: FocusHandle,
}

impl EventEmitter<DialogEvent> for ConfirmDialog {}

impl ConfirmDialog {
    pub fn new(
        title: impl Into<SharedString>,
        detail: impl Into<SharedString>,
        buttons: &[&str],
        default: usize,
        cancel: usize,
        cx: &mut Context<Self>,
    ) -> Self {
        Self {
            title: title.into(),
            detail: detail.into(),
            buttons: buttons
                .iter()
                .map(|b| SharedString::from(b.to_string()))
                .collect(),
            default,
            cancel,
            focus: cx.focus_handle(),
        }
    }
}

impl Focusable for ConfirmDialog {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for ConfirmDialog {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::get(cx);
        let n = self.buttons.len();
        let cancel = self.cancel;
        frame(&theme, 460.)
            .key_context("Modal")
            .track_focus(&self.focus)
            .on_action(cx.listener(|d, _: &Confirm, _, cx| cx.emit(DialogEvent::Answer(d.default))))
            .on_action(cx.listener(|d, _: &Cancel, _, cx| cx.emit(DialogEvent::Answer(d.cancel))))
            .on_key_down(cx.listener(move |_, ev: &gpui::KeyDownEvent, _, cx| {
                if let Ok(i) = ev.keystroke.key.parse::<usize>()
                    && (1..=n).contains(&i)
                {
                    cx.emit(DialogEvent::Answer(i - 1));
                }
            }))
            .child(header(
                &theme,
                self.title.clone(),
                cx.listener(move |_, _, _, cx| cx.emit(DialogEvent::Answer(cancel))),
            ))
            .child(
                body().child(
                    div()
                        .text_color(hsla(theme.muted))
                        .child(self.detail.clone()),
                ),
            )
            .child(
                footer(&theme).children(self.buttons.iter().enumerate().map(|(i, label)| {
                    let kind = if i == self.default {
                        ButtonKind::Primary
                    } else {
                        ButtonKind::Secondary
                    };
                    button(("answer", i), label.clone(), kind, &theme)
                        .on_click(cx.listener(move |_, _, _, cx| cx.emit(DialogEvent::Answer(i))))
                })),
            )
    }
}
