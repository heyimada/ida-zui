// Pieces shared by the table-shaped editors (structures, enums, stack
// frames): column headers, cells, rows and the inline cell editor.
use crate::input::TextInput;
use crate::theme::{Theme, code_font_name, hsla};
use gpui::{
    App, Context, Div, Entity, FocusHandle, Focusable, SharedString, Stateful, Subscription,
    Window, div, prelude::*, px,
};

pub const ROW_HEIGHT: f32 = 22.;

/// A column: a fixed width, or `None` to share what is left.
#[derive(Clone, Copy)]
pub struct Col {
    pub title: &'static str,
    pub width: Option<f32>,
    pub mono: bool,
}

impl Col {
    pub const fn fixed(title: &'static str, width: f32) -> Self {
        Self {
            title,
            width: Some(width),
            mono: true,
        }
    }

    pub const fn flex(title: &'static str) -> Self {
        Self {
            title,
            width: None,
            mono: true,
        }
    }

    pub const fn text(mut self) -> Self {
        self.mono = false;
        self
    }
}

/// An empty cell box of the column's width.
pub fn cell(col: &Col) -> Div {
    let d = div()
        .h_full()
        .flex()
        .items_center()
        .px(px(8.))
        .overflow_hidden()
        .whitespace_nowrap()
        .text_ellipsis();
    let d = match col.width {
        Some(w) => d.w(px(w)).flex_none(),
        None => d.flex_1().min_w_0(),
    };
    if col.mono {
        d.font_family(code_font_name())
    } else {
        d
    }
}

pub fn header(cols: &[Col], theme: &Theme) -> Div {
    div()
        .h(px(24.))
        .flex_none()
        .flex()
        .items_center()
        .bg(hsla(theme.surface2))
        .border_b_1()
        .border_color(hsla(theme.border))
        .text_size(px(11.))
        .font_weight(gpui::FontWeight::MEDIUM)
        .text_color(hsla(theme.muted))
        .children(
            cols.iter()
                .map(|c| cell(c).font_family(crate::theme::UI_FONT).child(c.title)),
        )
}

/// The frame of one row: height, selection and hover colours.
pub fn row_shell(
    id: impl Into<gpui::ElementId>,
    selected: bool,
    muted: bool,
    theme: &Theme,
) -> Stateful<Div> {
    div()
        .id(id)
        .w_full()
        .h(px(ROW_HEIGHT))
        .flex()
        .items_center()
        .cursor_pointer()
        .text_size(px(12.))
        .text_color(if muted {
            hsla(theme.faint)
        } else {
            hsla(theme.text)
        })
        .when(selected, |d| d.bg(hsla(theme.selection)))
        .hover(|d| d.bg(hsla(theme.hover)))
}

/// A cell being edited in place: a compact text input that takes the focus.
/// Enter and Escape reach the view through the `CellEdit` key context.
pub struct InlineEdit {
    pub row: usize,
    pub col: usize,
    pub input: Entity<TextInput>,
    _blur: Subscription,
}

impl InlineEdit {
    /// Starts editing `text`. Losing the focus calls `on_blur` (which should
    /// drop the edit).
    pub fn start<V: 'static>(
        row: usize,
        col: usize,
        text: &str,
        on_blur: fn(&mut V, &mut Window, &mut Context<V>),
        window: &mut Window,
        cx: &mut Context<V>,
    ) -> Self {
        let input = cx.new(|cx| TextInput::new(text, "", cx).compact());
        let focus: FocusHandle = input.read(cx).focus_handle(cx);
        window.focus(&focus, cx);
        let blur = cx.on_focus_out(&focus, window, move |this, _, window, cx| {
            on_blur(this, window, cx)
        });
        Self {
            row,
            col,
            input,
            _blur: blur,
        }
    }

    pub fn text(&self, cx: &App) -> String {
        self.input.read(cx).value().trim().to_owned()
    }
}

/// A compact secondary button for editor toolbars.
pub fn button(
    id: impl Into<gpui::ElementId>,
    label: impl Into<SharedString>,
    theme: &Theme,
) -> Stateful<Div> {
    crate::ui::button(id, label, crate::ui::ButtonKind::Secondary, true, theme)
}

/// The primary toolbar button (accent coloured).
pub fn primary_button(
    id: impl Into<gpui::ElementId>,
    label: impl Into<SharedString>,
    theme: &Theme,
) -> Stateful<Div> {
    crate::ui::button(id, label, crate::ui::ButtonKind::Primary, true, theme)
}
