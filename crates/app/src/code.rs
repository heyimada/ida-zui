// Shared painting and hit-testing for IDA tagged lines (listing, pseudocode,
// graph blocks). Lines are shaped per frame (gpui caches layouts) and the
// shapes are kept so mouse positions can be mapped back to byte offsets.
use crate::theme::{Theme, code_font_name, hsla};
use gpui::{
    App, Bounds, Font, Hsla, Pixels, Point, ShapedLine, SharedString, TextAlign, TextRun, Window,
    fill, font, point, px, size,
};
use ida_proto::Ea;
use ida_proto::tags::{self, Line};
use std::ops::Range;

pub fn font_size() -> f32 {
    crate::theme::code_font_size()
}

pub fn line_height() -> f32 {
    crate::theme::code_line_height()
}

pub fn code_font() -> Font {
    font(code_font_name())
}

/// Splits `line`'s colour runs into text runs, painting every occurrence of
/// `highlight` with a background so the selected identifier stands out.
pub fn text_runs(line: &Line, theme: &Theme, highlight: Option<&str>, font: &Font) -> Vec<TextRun> {
    let marks = highlight
        .filter(|h| !h.is_empty())
        .map(|h| find_word_occurrences(&line.text, h))
        .unwrap_or_default();
    let mut out = Vec::with_capacity(line.runs.len() + marks.len() * 2);
    let mut push = |range: Range<usize>, color: u8, marked: bool| {
        if range.is_empty() {
            return;
        }
        out.push(TextRun {
            len: range.len(),
            font: font.clone(),
            color: theme.tag_text(color, &line.text[range.clone()]),
            background_color: marked.then(|| hsla(theme.hl)),
            underline: None,
            strikethrough: None,
        });
    };
    for run in &line.runs {
        let mut at = run.range.start;
        for m in marks
            .iter()
            .filter(|m| m.start < run.range.end && m.end > run.range.start)
        {
            let s = m.start.max(run.range.start);
            let e = m.end.min(run.range.end);
            push(at..s, run.color, false);
            push(s..e, run.color, true);
            at = e;
        }
        push(at..run.range.end, run.color, false);
    }
    out
}

/// Whole-word occurrences of `word` in `text`.
pub fn find_word_occurrences(text: &str, word: &str) -> Vec<Range<usize>> {
    let bytes = text.as_bytes();
    let is_word =
        |b: u8| b.is_ascii_alphanumeric() || b == b'_' || b == b'$' || b == b'@' || b == b'?';
    let mut out = Vec::new();
    let mut from = 0;
    while let Some(i) = text[from..].find(word) {
        let s = from + i;
        let e = s + word.len();
        let left_ok = s == 0 || !is_word(bytes[s - 1]);
        let right_ok = e >= bytes.len() || !is_word(bytes[e]);
        if left_ok && right_ok {
            out.push(s..e);
        }
        from = e.max(s + 1);
        if from >= text.len() {
            break;
        }
    }
    out
}

/// The token the user means at byte `pos`: the coloured run when it is a
/// name, otherwise the identifier around `pos`.
pub fn token_at(line: &Line, pos: usize) -> Option<Range<usize>> {
    if let Some(run) = line.run_at(pos)
        && tags::color::is_name(run.color)
    {
        let raw = &line.text[run.range.clone()];
        let lead = raw.len() - raw.trim_start().len();
        let body = raw.trim().trim_end_matches(':');
        if !body.is_empty() {
            let start = run.range.start + lead;
            return Some(start..start + body.len());
        }
    }
    let w = line.word_at(pos)?;
    let body = line.text[w.clone()].trim_end_matches(':');
    (!body.is_empty()).then(|| w.start..w.start + body.len())
}

/// Shapes a tagged line for painting.
pub fn shape(
    window: &mut Window,
    line: &Line,
    theme: &Theme,
    highlight: Option<&str>,
) -> ShapedLine {
    let font = code_font();
    let runs = text_runs(line, theme, highlight, &font);
    let text: SharedString = line.text.clone().into();
    window
        .text_system()
        .shape_line(text, px(font_size()), &runs, None)
}

pub fn shape_plain(window: &mut Window, text: &str, color: Hsla) -> ShapedLine {
    let run = TextRun {
        len: text.len(),
        font: code_font(),
        color,
        background_color: None,
        underline: None,
        strikethrough: None,
    };
    window.text_system().shape_line(
        SharedString::from(text.to_owned()),
        px(font_size()),
        &[run],
        None,
    )
}

pub fn shape_plain_sized(window: &mut Window, text: &str, color: Hsla, size: Pixels) -> ShapedLine {
    let run = TextRun {
        len: text.len(),
        font: code_font(),
        color,
        background_color: None,
        underline: None,
        strikethrough: None,
    };
    window
        .text_system()
        .shape_line(SharedString::from(text.to_owned()), size, &[run], None)
}

pub fn paint_shaped(shaped: &ShapedLine, origin: Point<Pixels>, window: &mut Window, cx: &mut App) {
    let _ = shaped.paint(origin, px(line_height()), TextAlign::Left, None, window, cx);
}

pub fn paint_row_bg(bounds: Bounds<Pixels>, row_top: Pixels, color: Hsla, window: &mut Window) {
    window.paint_quad(fill(
        Bounds::new(
            point(bounds.left(), row_top),
            size(bounds.size.width, px(line_height())),
        ),
        color,
    ));
}

/// Width of the gutter column that holds breakpoint dots and the
/// instruction pointer arrow.
pub const MARK_W: f32 = 30.;

/// What the debugger puts in the margin: the instruction pointer and the
/// breakpoints, copied out of the session for a paint.
#[derive(Default, Clone)]
pub struct DebugMarks {
    pub ip: Option<Ea>,
    /// (address, enabled)
    pub bpts: Vec<(Ea, bool)>,
}

impl DebugMarks {
    pub fn read(session: &crate::session::Session) -> Self {
        Self {
            ip: session.dbg.ip,
            bpts: session.dbg.bpts.iter().map(|b| (b.ea, b.enabled)).collect(),
        }
    }

    /// Whether a breakpoint is at `ea`, and if so whether it is enabled.
    pub fn bpt(&self, ea: Ea) -> Option<bool> {
        self.bpts.iter().find(|(e, _)| *e == ea).map(|(_, on)| *on)
    }

    pub fn is_ip(&self, ea: Ea) -> bool {
        self.ip == Some(ea)
    }

    /// The line highlight for `ea`: the instruction pointer wins over a
    /// breakpoint.
    pub fn row_bg(&self, ea: Ea, theme: &Theme) -> Option<Hsla> {
        if self.is_ip(ea) {
            Some(hsla(theme.accent_soft))
        } else if self.bpt(ea) == Some(true) {
            Some(hsla(theme.danger_soft))
        } else {
            None
        }
    }

    /// Paints the dot and the arrow for the line at `ea` whose top is
    /// `row_top`, inside the gutter that starts at `left`.
    pub fn paint(&self, ea: Ea, left: Pixels, row_top: Pixels, theme: &Theme, window: &mut Window) {
        self.paint_at(ea, left, row_top, px(line_height()), theme, window);
    }

    /// `paint` for a line of another height (a zoomed graph).
    pub fn paint_at(
        &self,
        ea: Ea,
        left: Pixels,
        row_top: Pixels,
        row_h: Pixels,
        theme: &Theme,
        window: &mut Window,
    ) {
        paint_marks(
            self.bpt(ea),
            self.is_ip(ea),
            left,
            row_top,
            row_h,
            theme,
            window,
        );
    }
}

/// A breakpoint dot (filled when enabled, an outline when not) and the
/// instruction pointer arrow in the marker gutter.
pub fn paint_marks(
    bpt: Option<bool>,
    ip: bool,
    left: Pixels,
    row_top: Pixels,
    row_h: Pixels,
    theme: &Theme,
    window: &mut Window,
) {
    {
        let mid = row_top + row_h / 2.;
        if let Some(on) = bpt {
            let d = px(10.);
            window.paint_quad(gpui::quad(
                Bounds::new(point(left + px(4.), mid - d / 2.), size(d, d)),
                d / 2.,
                if on {
                    hsla(theme.danger)
                } else {
                    gpui::transparent_black()
                },
                px(1.5),
                hsla(theme.danger),
                gpui::BorderStyle::Solid,
            ));
        }
        if ip {
            let x = left + px(MARK_W - 4.);
            let mut pb = gpui::PathBuilder::fill();
            pb.move_to(point(x - px(8.), mid - px(5.)));
            pb.line_to(point(x, mid));
            pb.line_to(point(x - px(8.), mid + px(5.)));
            pb.close();
            if let Ok(path) = pb.build() {
                window.paint_path(path, hsla(theme.accent));
            }
        }
    }
}

/// What the last paint laid out, for mapping mouse positions to text.
#[derive(Default)]
pub struct HitMap {
    pub bounds: Option<Bounds<Pixels>>,
    pub text_x: Pixels,
    /// (row index, top y, shaped line)
    pub rows: Vec<(usize, Pixels, ShapedLine)>,
}

impl HitMap {
    /// The row whose line is under `pos` when it is in the marker gutter.
    pub fn gutter_row(&self, pos: Point<Pixels>) -> Option<usize> {
        let bounds = self.bounds?;
        if !bounds.contains(&pos) || pos.x > bounds.left() + px(MARK_W) {
            return None;
        }
        self.rows
            .iter()
            .find(|(_, top, _)| pos.y >= *top && pos.y < *top + px(line_height()))
            .map(|(row, _, _)| *row)
    }

    /// Row index and byte offset under `pos`.
    pub fn hit(&self, pos: Point<Pixels>) -> Option<(usize, usize)> {
        let bounds = self.bounds?;
        if !bounds.contains(&pos) {
            return None;
        }
        let (row, _, shaped) = self
            .rows
            .iter()
            .find(|(_, top, _)| pos.y >= *top && pos.y < *top + px(line_height()))?;
        let col = shaped.closest_index_for_x(pos.x - self.text_x);
        Some((*row, col))
    }
}

pub fn char_width(window: &mut Window) -> Pixels {
    let shaped = shape_plain(window, "0000000000", gpui::black());
    shaped.width / 10.
}
