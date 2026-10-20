// Building blocks of the design system: icons,
// icon buttons, badges, key caps, buttons, panel headers and tooltips. Views
// compose these so every control looks and behaves alike.
use crate::theme::{Theme, code_font_name, hsla, hsla_a};
use gpui::{
    Animation, AnimationExt, AnyView, App, BoxShadow, Context, ElementId, Hsla, IntoElement,
    Render, SharedString, Stateful, Styled, Svg, Transformation, Window, div, percentage,
    prelude::*, px, rgb, svg,
};
use std::time::Duration;

/// A 16 px stroke icon from `assets/icons`, tinted `color`.
pub fn icon(name: &str, size: f32, color: Hsla) -> Svg {
    svg()
        .path(SharedString::from(format!("icons/{name}.svg")))
        .size(px(size))
        .flex_none()
        .text_color(color)
}

/// A turning arc for work in progress. `id` must be unique among siblings.
pub fn spinner(id: impl Into<ElementId>, size: f32, color: Hsla) -> impl IntoElement {
    icon("spinner", size, color).with_animation(
        id,
        Animation::new(Duration::from_millis(900)).repeat(),
        |svg, delta| svg.with_transformation(Transformation::rotate(percentage(delta))),
    )
}

/// `a` moved `t` of the way towards `b`.
pub fn mix(a: u32, b: u32, t: f32) -> Hsla {
    let ch = |shift: u32| {
        let (x, y) = (((a >> shift) & 0xff) as f32, ((b >> shift) & 0xff) as f32);
        (x + (y - x) * t).round() as u32
    };
    rgb((ch(16) << 16) | (ch(8) << 8) | ch(0)).into()
}

/// The 2 px focus ring.
pub fn ring(theme: &Theme) -> Vec<BoxShadow> {
    vec![BoxShadow::new(px(0.), px(0.), hsla(theme.ring)).spread_radius(px(2.))]
}

/// The drop shadow under menus, palettes and dialogs.
pub fn drop_shadow(theme: &Theme, y: f32, blur: f32) -> Vec<BoxShadow> {
    vec![BoxShadow::new(px(0.), px(y), hsla_a(theme.shadow)).blur_radius(px(blur))]
}

/// A toolbar or strip icon button: `muted`, `accent` on `accent_soft` when
/// `active`, `faint` and inert when not `enabled`. Callers attach `on_click`.
pub fn icon_button(
    id: impl Into<ElementId>,
    name: &str,
    size: f32,
    active: bool,
    enabled: bool,
    theme: &Theme,
) -> Stateful<gpui::Div> {
    let color = if !enabled {
        theme.faint
    } else if active {
        theme.accent
    } else {
        theme.muted
    };
    div()
        .id(id)
        .size(px(size))
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(if size > 24. { 6. } else { 4. }))
        .when(active, |d| d.bg(hsla(theme.accent_soft)))
        .when(enabled, |d| d.cursor_pointer())
        .when(enabled && !active, |d| {
            d.hover(|d| d.bg(hsla(theme.hover)).text_color(hsla(theme.text)))
        })
        .child(icon(name, 16., hsla(color)))
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum BadgeKind {
    Neutral,
    Count,
    Danger,
    Success,
    /// call, jmp, read, offs
    Xref,
    /// A write xref.
    XrefWrite,
}

pub fn badge(text: impl Into<SharedString>, kind: BadgeKind, theme: &Theme) -> gpui::Div {
    let (bg, fg) = match kind {
        BadgeKind::Neutral => (theme.raised, theme.muted),
        BadgeKind::Count | BadgeKind::Xref => (theme.accent_soft, theme.accent),
        BadgeKind::Danger => (theme.danger_soft, theme.danger),
        BadgeKind::Success => (theme.success_soft, theme.success),
        BadgeKind::XrefWrite => (theme.warning_soft, theme.warning),
    };
    div()
        .h(px(16.))
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .px(px(5.))
        .rounded(px(if kind == BadgeKind::Count { 8. } else { 3. }))
        .when(kind == BadgeKind::Count, |d| d.min_w(px(16.)))
        .when(
            matches!(kind, BadgeKind::Xref | BadgeKind::XrefWrite),
            |d| d.w(px(36.)).px(px(0.)),
        )
        .bg(hsla(bg))
        .text_color(hsla(fg))
        .text_size(px(11.))
        .line_height(px(16.))
        .font_weight(gpui::FontWeight::MEDIUM)
        .whitespace_nowrap()
        .child(text.into())
}

/// A badge for an xref kind letter as IDA reports it (`p`, `j`, `r`, `w`, `o`, ...).
pub fn xref_badge(kind: &str, theme: &Theme) -> gpui::Div {
    let (label, write) = match kind {
        "p" => ("call", false),
        "j" => ("jmp", false),
        "r" => ("read", false),
        "w" => ("write", true),
        "o" => ("offs", false),
        "i" => ("info", false),
        "^" => ("flow", false),
        other => (other, false),
    };
    badge(
        label.to_owned(),
        if write {
            BadgeKind::XrefWrite
        } else {
            BadgeKind::Xref
        },
        theme,
    )
}

/// The product mark ("iz" from ida-zui) on an accent tile; the title strip and
/// the welcome page share it.
pub fn logo(size: f32, theme: &Theme) -> gpui::Div {
    div()
        .size(px(size))
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(size / 4.))
        .bg(hsla(theme.accent))
        .text_color(hsla(theme.on_accent))
        .text_size(px(size * 0.56))
        .font_weight(gpui::FontWeight::SEMIBOLD)
        .child("iz")
}

/// A keyboard key cap: 11 px mono on `raised`.
pub fn key_cap(text: impl Into<SharedString>, theme: &Theme) -> gpui::Div {
    div()
        .h(px(16.))
        .flex_none()
        .flex()
        .items_center()
        .px(px(4.))
        .rounded(px(3.))
        .bg(hsla(theme.raised))
        .text_color(hsla(theme.muted))
        .text_size(px(11.))
        .line_height(px(16.))
        .font_family(code_font_name())
        .child(text.into())
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ButtonKind {
    Primary,
    Secondary,
    Ghost,
    Danger,
}

/// A 28 px button (24 when `compact`); callers attach `on_click`.
pub fn button(
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
    kind: ButtonKind,
    compact: bool,
    theme: &Theme,
) -> Stateful<gpui::Div> {
    let t = *theme;
    let base = div()
        .id(id)
        .h(px(if compact { 24. } else { 28. }))
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .px(px(12.))
        .rounded(px(6.))
        .text_size(px(12.))
        .line_height(px(16.))
        .font_weight(gpui::FontWeight::MEDIUM)
        .whitespace_nowrap()
        .cursor_pointer();
    match kind {
        ButtonKind::Primary => base
            .bg(hsla(t.accent))
            .text_color(hsla(t.on_accent))
            .hover(move |d| d.bg(mix(t.accent, t.text, 0.08)))
            .active(move |d| d.bg(mix(t.accent, t.text, 0.12))),
        ButtonKind::Secondary | ButtonKind::Danger => base
            .bg(hsla(t.surface2))
            .border_1()
            .border_color(hsla(t.border_strong))
            .text_color(hsla(if kind == ButtonKind::Danger {
                t.danger
            } else {
                t.text
            }))
            .hover(move |d| d.bg(hsla(t.hover)))
            .active(move |d| d.bg(hsla(t.raised))),
        ButtonKind::Ghost => base
            .text_color(hsla(t.text))
            .hover(move |d| d.bg(hsla(t.hover)))
            .active(move |d| d.bg(hsla(t.raised))),
    }
    .child(label.into())
}

/// Uppercase section caption: 11 px, 500, `muted`.
pub fn section_label(text: &str, theme: &Theme) -> gpui::Div {
    div()
        .text_size(px(11.))
        .line_height(px(16.))
        .font_weight(gpui::FontWeight::MEDIUM)
        .text_color(hsla(theme.muted))
        .child(text.to_uppercase())
}

/// A 28 px panel header on `surface2` with a hairline below. The title is
/// `12 / 500 text`; callers add the right-hand side.
pub fn panel_header(title: impl Into<SharedString>, theme: &Theme) -> gpui::Div {
    div()
        .h(px(28.))
        .flex_none()
        .flex()
        .items_center()
        .gap(px(8.))
        .pl(px(12.))
        .pr(px(8.))
        .bg(hsla(theme.surface2))
        .border_b_1()
        .border_color(hsla(theme.border))
        .text_size(px(12.))
        .line_height(px(16.))
        .font_weight(gpui::FontWeight::MEDIUM)
        .text_color(hsla(theme.text))
        .child(title.into())
}

/// A monospace 11 px muted label (addresses, segments).
pub fn mono_note(text: impl Into<SharedString>, theme: &Theme) -> gpui::Div {
    div()
        .font_family(code_font_name())
        .text_size(px(11.))
        .line_height(px(16.))
        .text_color(hsla(theme.muted))
        .whitespace_nowrap()
        .child(text.into())
}

/// A small dot, e.g. the unsaved marker or the analysis state.
pub fn dot(size: f32, color: u32) -> gpui::Div {
    div()
        .size(px(size))
        .flex_none()
        .rounded(px(size / 2.))
        .bg(hsla(color))
}

/// The tooltip body: the label and, when given, its shortcut.
struct Tip {
    text: SharedString,
    shortcut: Option<SharedString>,
}

impl Render for Tip {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::get(cx);
        div()
            .px(px(8.))
            .py(px(4.))
            .rounded(px(6.))
            .bg(hsla(theme.popover))
            .border_1()
            .border_color(hsla(theme.border_strong))
            .shadow(drop_shadow(&theme, 8., 24.))
            .flex()
            .items_center()
            .gap(px(8.))
            .text_size(px(11.))
            .line_height(px(16.))
            .text_color(hsla(theme.text))
            .child(self.text.clone())
            .when_some(self.shortcut.clone(), |d, s| {
                d.child(
                    div()
                        .font_family(code_font_name())
                        .text_color(hsla(theme.muted))
                        .child(s),
                )
            })
    }
}

/// The builder `Interactivity::tooltip` expects.
pub fn tip(
    text: impl Into<SharedString>,
    shortcut: Option<String>,
) -> impl Fn(&mut Window, &mut App) -> AnyView + 'static {
    let text = text.into();
    move |_, cx| {
        let (text, shortcut) = (
            text.clone(),
            shortcut
                .clone()
                .filter(|s| !s.is_empty())
                .map(SharedString::from),
        );
        cx.new(|_| Tip { text, shortcut }).into()
    }
}

/// The segmented control's track: 28 px, `surface2`, 1 px border, radius 8.
/// Callers add `segment`s.
pub fn segmented_track(theme: &Theme) -> gpui::Div {
    div()
        .h(px(28.))
        .flex_none()
        .flex()
        .items_center()
        .gap(px(2.))
        .p(px(2.))
        .rounded(px(8.))
        .bg(hsla(theme.surface2))
        .border_1()
        .border_color(hsla(theme.border))
}

/// One segment: 22 px, `muted`, hover `hover`; selected is `accent` on `accent_soft`.
pub fn segment(
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
    selected: bool,
    theme: &Theme,
) -> Stateful<gpui::Div> {
    let t = *theme;
    div()
        .id(id)
        .h(px(22.))
        .flex()
        .items_center()
        .px(px(12.))
        .rounded(px(6.))
        .text_size(px(12.))
        .line_height(px(16.))
        .font_weight(gpui::FontWeight::MEDIUM)
        .cursor_pointer()
        .text_color(hsla(if selected { t.accent } else { t.muted }))
        .when(selected, |d| d.bg(hsla(t.accent_soft)))
        .when(!selected, |d| d.hover(move |d| d.bg(hsla(t.hover))))
        .child(label.into())
}
