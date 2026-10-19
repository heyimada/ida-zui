// Paints the editor. Only the visible lines are highlighted and shaped, so a
// 10k-line buffer costs the same per frame as a 50-line one.
use crate::doc::Pos;
use crate::editor::{Editor, Layout};
use crate::runs;
use crate::text::{self, TAB_WIDTH};
use gpui::{
    App, Bounds, ContentMask, Element, ElementId, ElementInputHandler, Entity, Font,
    GlobalElementId, InspectorElementId, IntoElement, LayoutId, PaintQuad, Pixels, Point,
    SharedString, Style, TextAlign, TextRun, UnderlineStyle, Window, fill, point, px, relative,
    size,
};
use std::ops::Range;

pub(crate) struct EditorElement {
    pub editor: Entity<Editor>,
}

pub(crate) struct Prepaint {
    gutter: Option<PaintQuad>,
    text_area: Bounds<Pixels>,
    current_line: Option<PaintQuad>,
    selection: Vec<PaintQuad>,
    lines: Vec<(gpui::ShapedLine, Point<Pixels>)>,
    numbers: Vec<(gpui::ShapedLine, Point<Pixels>)>,
    markers: Vec<PaintQuad>,
    messages: Vec<(gpui::ShapedLine, Point<Pixels>)>,
    caret: Option<PaintQuad>,
    scrollbars: Vec<PaintQuad>,
    focus: gpui::FocusHandle,
    layout: Layout,
}

impl IntoElement for EditorElement {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

fn run(font: &Font, len: usize, color: gpui::Hsla, underline: Option<UnderlineStyle>) -> TextRun {
    TextRun {
        len,
        font: font.clone(),
        color,
        background_color: None,
        underline,
        strikethrough: None,
    }
}

/// Visible part of a multi-line position range on `line` (byte columns).
fn range_on_line(range: &Range<Pos>, line: usize, len: usize) -> Option<Range<usize>> {
    if line < range.start.line || line > range.end.line {
        return None;
    }
    let start = if line == range.start.line {
        range.start.col
    } else {
        0
    };
    let end = if line == range.end.line {
        range.end.col
    } else {
        len
    };
    Some(start..end.min(len))
}

impl Element for EditorElement {
    type RequestLayoutState = ();
    type PrepaintState = Prepaint;

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        let mut style = Style::default();
        style.size.width = relative(1.).into();
        style.size.height = relative(1.).into();
        (window.request_layout(style, [], cx), ())
    }

    fn prepaint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> Prepaint {
        let ed = self.editor.read(cx);
        let theme = ed.theme.clone();
        let (font_size, lh) = (ed.font_size, ed.line_height);
        let mut style = window.text_style();
        style.font_family = ed.font_family.clone();
        let font = style.font();
        let line_count = ed.doc.line_count();
        let max_cols = ed.doc.lines().iter().map(|l| l.len()).max().unwrap_or(0);
        let show_gutter = ed.show_gutter;
        let focus = ed.focus_handle.clone();
        let head = ed.doc.head();
        let (mut scroll, reveal) = (ed.scroll, ed.reveal);

        let char_w = {
            let probe = "0000000000";
            let line = window.text_system().shape_line(
                SharedString::from(probe),
                font_size,
                &[run(&font, probe.len(), theme.text, None)],
                None,
            );
            line.width / probe.len() as f32
        };

        let digits = line_count.to_string().len().max(2);
        let marker_w = px(12.);
        let gutter_w = if show_gutter {
            px(6.) + marker_w + char_w * digits as f32 + px(10.)
        } else {
            px(0.)
        };
        let pad = px(8.);
        let text_left = bounds.left() + gutter_w + pad;
        let view_w = (bounds.size.width - gutter_w - pad).max(px(1.));
        let view_h = bounds.size.height;

        // Vertical scroll: clamp, then bring the caret line into view if asked.
        let content_h = lh * line_count as f32;
        let max_y = (content_h - view_h).max(px(0.));
        scroll.y = scroll.y.clamp(px(0.), max_y);
        if reveal {
            let top = lh * head.line as f32;
            if top < scroll.y {
                scroll.y = top;
            } else if top + lh > scroll.y + view_h {
                scroll.y = top + lh - view_h;
            }
            scroll.y = scroll.y.clamp(px(0.), max_y);
        }

        let first = ((scroll.y / lh).floor() as usize).min(line_count - 1);
        let last = (((scroll.y + view_h) / lh).ceil() as usize + 1)
            .min(line_count)
            .max(first + 1);
        let spans = self.editor.update(cx, |e, _| e.spans_for(first..last));

        // Shape the visible lines.
        let ed = self.editor.read(cx);
        let marked = ed.marked.clone();
        let mut shaped: Vec<Option<gpui::ShapedLine>> = Vec::with_capacity(last - first);
        for (i, line_spans) in (first..last).zip(&spans) {
            let raw = ed.doc.line(i);
            let disp = text::expand_tabs(raw, TAB_WIDTH);
            if disp.is_empty() {
                shaped.push(None);
                continue;
            }
            let map = |r: Range<usize>| {
                text::to_display(raw, r.start, TAB_WIDTH)..text::to_display(raw, r.end, TAB_WIDTH)
            };
            let mapped: Vec<_> = line_spans
                .iter()
                .map(|(r, k)| (map(r.clone()), *k))
                .collect();
            let marked_here = marked
                .as_ref()
                .and_then(|m| range_on_line(m, i, raw.len()))
                .map(map);
            let error_here = ed
                .errors
                .iter()
                .any(|e| e.start_line <= i && i <= e.end_line)
                .then(|| text::first_nonblank(raw)..raw.trim_end().len())
                .filter(|r| r.start < r.end)
                .map(map);
            let runs: Vec<TextRun> = runs::segments(&disp, &mapped, marked_here, error_here)
                .into_iter()
                .map(|s| {
                    let color = s.kind.map_or(theme.text, |k| theme.kind_color(k));
                    let underline = if s.error {
                        Some(UnderlineStyle {
                            thickness: px(1.),
                            color: Some(theme.error),
                            wavy: true,
                        })
                    } else if s.marked {
                        Some(UnderlineStyle {
                            thickness: px(1.),
                            color: Some(color),
                            wavy: false,
                        })
                    } else {
                        None
                    };
                    run(&font, s.len, color, underline)
                })
                .collect();
            let text: SharedString = disp.into_owned().into();
            shaped.push(Some(
                window
                    .text_system()
                    .shape_line(text, font_size, &runs, None),
            ));
        }

        // Horizontal scroll: clamp, then keep the caret column visible.
        let content_w = char_w * (max_cols + 2) as f32;
        let max_x = (content_w - view_w).max(px(0.));
        scroll.x = scroll.x.clamp(px(0.), max_x);
        let caret_x = |pos: Pos| -> Pixels {
            let raw = ed.doc.line(pos.line);
            let disp = text::to_display(raw, pos.col, TAB_WIDTH);
            match pos.line.checked_sub(first).and_then(|i| shaped.get(i)) {
                Some(Some(line)) => line.x_for_index(disp),
                _ => px(0.),
            }
        };
        let head_x = caret_x(head);
        if reveal {
            let margin = char_w * 2.;
            if head_x < scroll.x {
                scroll.x = (head_x - margin).max(px(0.));
            } else if head_x + margin > scroll.x + view_w {
                scroll.x = head_x + margin - view_w;
            }
            scroll.x = scroll.x.clamp(px(0.), max_x);
        }

        // Quads, numbers and positions.
        let text_area = Bounds::new(
            point(bounds.left() + gutter_w, bounds.top()),
            size(
                (bounds.size.width - gutter_w).max(px(0.)),
                bounds.size.height,
            ),
        );
        let origin_x = text_left - scroll.x;
        let sel = ed.doc.selection();
        let has_sel = sel.start != sel.end;
        let mut selection = Vec::new();
        let mut current_line = None;
        let mut numbers = Vec::new();
        let mut markers = Vec::new();
        let mut messages = Vec::new();
        let mut lines = Vec::new();
        for (idx, i) in (first..last).enumerate() {
            let y = bounds.top() + lh * i as f32 - scroll.y;
            let line = &shaped[idx];
            let raw = ed.doc.line(i);
            if i == head.line && !has_sel {
                current_line = Some(fill(
                    Bounds::new(point(text_area.left(), y), size(text_area.size.width, lh)),
                    theme.current_line,
                ));
            }
            if let Some(r) = range_on_line(&(sel.clone()), i, raw.len()).filter(|_| has_sel) {
                let x_of = |col: usize| match line {
                    Some(l) => l.x_for_index(text::to_display(raw, col, TAB_WIDTH)),
                    None => px(0.),
                };
                let (x0, mut x1) = (x_of(r.start), x_of(r.end));
                if i < sel.end.line {
                    x1 += char_w; // the newline is selected too
                }
                if x1 > x0 {
                    selection.push(fill(
                        Bounds::new(point(origin_x + x0, y), size(x1 - x0, lh)),
                        theme.selection,
                    ));
                }
            }
            if let Some(l) = line {
                lines.push((l.clone(), point(origin_x, y)));
            }
            let err = ed
                .errors
                .iter()
                .find(|e| e.start_line <= i && i <= e.end_line);
            if show_gutter {
                let active = i == head.line;
                let color = if err.is_some() {
                    theme.error
                } else if active {
                    theme.gutter_text_active
                } else {
                    theme.gutter_text
                };
                let label = (i + 1).to_string();
                let n = window.text_system().shape_line(
                    SharedString::from(label.clone()),
                    font_size,
                    &[run(&font, label.len(), color, None)],
                    None,
                );
                let x = bounds.left() + gutter_w - px(10.) - n.width;
                numbers.push((n, point(x, y)));
                if err.is_some() {
                    let d = px(6.);
                    markers.push(
                        fill(
                            Bounds::new(
                                point(bounds.left() + px(8.), y + (lh - d) / 2.),
                                size(d, d),
                            ),
                            theme.error,
                        )
                        .corner_radii(d / 2.),
                    );
                }
            }
            if let Some(e) = err.filter(|e| e.start_line == i && !e.message.is_empty()) {
                let msg: SharedString = e.message.lines().next().unwrap_or("").to_owned().into();
                if !msg.is_empty() {
                    let width = line.as_ref().map_or(px(0.), |l| l.width);
                    let shaped_msg = window.text_system().shape_line(
                        msg.clone(),
                        font_size,
                        &[run(&font, msg.len(), theme.error_text, None)],
                        None,
                    );
                    messages.push((shaped_msg, point(origin_x + width + char_w * 3., y)));
                }
            }
        }

        let caret = {
            let x = origin_x + head_x;
            let y = bounds.top() + lh * head.line as f32 - scroll.y;
            Some(fill(
                Bounds::new(point(x, y), size(px(2.), lh)),
                theme.caret,
            ))
        };

        let mut scrollbars = Vec::new();
        let thick = px(6.);
        if ed.own_scrollbars && max_y > px(0.) {
            let thumb = (view_h * (view_h / content_h)).max(px(24.));
            let top = (view_h - thumb) * (scroll.y / max_y);
            scrollbars.push(
                fill(
                    Bounds::new(
                        point(bounds.right() - thick - px(2.), bounds.top() + top),
                        size(thick, thumb),
                    ),
                    theme.scrollbar,
                )
                .corner_radii(thick / 2.),
            );
        }
        if ed.own_scrollbars && max_x > px(0.) {
            let track = text_area.size.width;
            let thumb = (track * (view_w / content_w)).max(px(24.));
            let left = (track - thumb) * (scroll.x / max_x);
            scrollbars.push(
                fill(
                    Bounds::new(
                        point(text_area.left() + left, bounds.bottom() - thick - px(2.)),
                        size(thumb, thick),
                    ),
                    theme.scrollbar,
                )
                .corner_radii(thick / 2.),
            );
        }

        let gutter = show_gutter.then(|| {
            fill(
                Bounds::new(bounds.origin, size(gutter_w, bounds.size.height)),
                theme.gutter_background,
            )
        });

        Prepaint {
            gutter,
            text_area,
            current_line,
            selection,
            lines,
            numbers,
            markers,
            messages,
            caret,
            scrollbars,
            focus,
            layout: Layout {
                bounds,
                text_left,
                line_height: lh,
                char_width: char_w,
                first_line: first,
                lines: shaped,
                scroll,
                viewport: size(view_w, view_h),
                content: size(content_w, content_h),
            },
        }
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        prepaint: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        window.handle_input(
            &prepaint.focus,
            ElementInputHandler::new(bounds, self.editor.clone()),
            cx,
        );
        let lh = prepaint.layout.line_height;
        let focused = prepaint.focus.is_focused(window);
        let always_caret = self.editor.read(cx).always_caret;

        window.with_content_mask(
            Some(ContentMask {
                bounds: prepaint.text_area,
            }),
            |window| {
                if let Some(q) = prepaint.current_line.take() {
                    window.paint_quad(q);
                }
                for q in prepaint.selection.drain(..) {
                    window.paint_quad(q);
                }
                for (line, origin) in &prepaint.lines {
                    line.paint(*origin, lh, TextAlign::Left, None, window, cx)
                        .ok();
                }
                for (line, origin) in &prepaint.messages {
                    line.paint(*origin, lh, TextAlign::Left, None, window, cx)
                        .ok();
                }
                if (focused || always_caret)
                    && let Some(q) = prepaint.caret.take()
                {
                    window.paint_quad(q);
                }
            },
        );

        if let Some(q) = prepaint.gutter.take() {
            window.paint_quad(q);
        }
        for (line, origin) in &prepaint.numbers {
            line.paint(*origin, lh, TextAlign::Left, None, window, cx)
                .ok();
        }
        for q in prepaint.markers.drain(..) {
            window.paint_quad(q);
        }
        for q in prepaint.scrollbars.drain(..) {
            window.paint_quad(q);
        }

        // Hand the measurements back for hit testing; no notify from paint.
        let layout = std::mem::replace(
            &mut prepaint.layout,
            Layout {
                bounds,
                text_left: px(0.),
                line_height: lh,
                char_width: px(0.),
                first_line: 0,
                lines: Vec::new(),
                scroll: point(px(0.), px(0.)),
                viewport: size(px(0.), px(0.)),
                content: size(px(0.), px(0.)),
            },
        );
        self.editor.update(cx, |e, _| {
            e.scroll = layout.scroll;
            e.reveal = false;
            e.layout = Some(layout);
        });
    }
}
