// The editor view: owns a `Doc`, handles actions, mouse and IME, and renders
// through `EditorElement`. Text logic lives in `doc`; this file is glue.
use crate::doc::{Doc, Pos};
use crate::element::EditorElement;
use crate::highlight::{Highlighter, StateCache};
use crate::text::{self, TAB_WIDTH};
use crate::theme::EditorTheme;
use gpui::{
    App, Bounds, Context, CursorStyle, EntityInputHandler, EventEmitter, FocusHandle, Focusable,
    KeyBinding, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, Pixels, Point,
    ScrollWheelEvent, ShapedLine, SharedString, UTF16Selection, Window, actions, div, point,
    prelude::*, px,
};
use std::ops::Range;

actions!(
    zui_editor,
    [
        Left,
        Right,
        Up,
        Down,
        SelectLeft,
        SelectRight,
        SelectUp,
        SelectDown,
        WordLeft,
        WordRight,
        SelectWordLeft,
        SelectWordRight,
        Home,
        End,
        SelectHome,
        SelectEnd,
        DocStart,
        DocEnd,
        SelectDocStart,
        SelectDocEnd,
        PageUp,
        PageDown,
        SelectPageUp,
        SelectPageDown,
        Newline,
        InsertNewline,
        Indent,
        Outdent,
        Backspace,
        Delete,
        DeleteWordLeft,
        DeleteWordRight,
        Undo,
        Redo,
        Cut,
        Copy,
        Paste,
        SelectAll,
        DuplicateLine,
        Submit,
    ]
);

pub const DEFAULT_FONT: &str = "Cascadia Mono";

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EditorEvent {
    /// The text changed (typing, paste, undo, `insert_at_cursor`; not `set_text`).
    Changed,
    /// The caret or selection moved or the text changed.
    CursorMoved,
    /// Ctrl+Enter was pressed.
    Submit,
    /// Console mode: Enter was pressed. Nothing was inserted; call
    /// [`Editor::newline`] to continue the statement on a new line.
    Enter,
    /// Console mode: Up was pressed with the caret on the first line.
    HistoryPrev,
    /// Console mode: Down was pressed with the caret on the last line.
    HistoryNext,
    /// Console mode: Tab was pressed with the caret right after a name
    /// character; nothing was inserted. The host of the editor offers
    /// completions (otherwise Tab indents).
    Complete,
}

/// A parser error on whole lines (0-based, inclusive). The lines get a squiggle
/// and a gutter marker; the message is shown after the first line.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ErrorMarker {
    pub start_line: usize,
    pub end_line: usize,
    pub message: SharedString,
}

impl ErrorMarker {
    pub fn new(start_line: usize, end_line: usize, message: impl Into<SharedString>) -> Self {
        Self {
            start_line,
            end_line: end_line.max(start_line),
            message: message.into(),
        }
    }
}

/// What the last paint measured; used for hit testing, paging and IME bounds.
pub(crate) struct Layout {
    pub bounds: Bounds<Pixels>,
    /// Window x of column 0 when not scrolled horizontally.
    pub text_left: Pixels,
    pub line_height: Pixels,
    pub char_width: Pixels,
    pub first_line: usize,
    /// Shaped (tab-expanded) lines from `first_line`; `None` for empty ones.
    pub lines: Vec<Option<ShapedLine>>,
    pub scroll: Point<Pixels>,
    /// Text area and whole document, for scrollbars.
    pub viewport: gpui::Size<Pixels>,
    pub content: gpui::Size<Pixels>,
}

impl Layout {
    /// X offset of a position inside its line, unscrolled.
    fn line_x(&self, doc: &Doc, pos: Pos) -> Pixels {
        let text = doc.line(pos.line);
        let disp = text::to_display(text, pos.col, TAB_WIDTH);
        match pos
            .line
            .checked_sub(self.first_line)
            .and_then(|i| self.lines.get(i))
        {
            Some(Some(shaped)) => shaped.x_for_index(disp),
            Some(None) => px(0.),
            None => self.char_width * text::display_col(text, pos.col, TAB_WIDTH) as f32,
        }
    }

    fn line_top(&self, line: usize) -> Pixels {
        self.bounds.top() + self.line_height * line as f32 - self.scroll.y
    }
}

pub struct Editor {
    pub(crate) focus_handle: FocusHandle,
    pub(crate) doc: Doc,
    pub(crate) theme: EditorTheme,
    pub(crate) font_family: SharedString,
    pub(crate) font_size: Pixels,
    pub(crate) line_height: Pixels,
    pub(crate) show_gutter: bool,
    pub(crate) always_caret: bool,
    pub(crate) errors: Vec<ErrorMarker>,
    pub(crate) marked: Option<Range<Pos>>,
    /// Top-left of the viewport inside the content, in pixels.
    pub(crate) scroll: Point<Pixels>,
    /// Set by anything that moves the caret; paint scrolls it into view.
    pub(crate) reveal: bool,
    pub(crate) layout: Option<Layout>,
    /// Paint the built-in thin scrollbars; a host that draws its own turns them off.
    pub(crate) own_scrollbars: bool,
    highlighter: Option<Box<dyn Highlighter>>,
    hl_cache: StateCache,
    read_only: bool,
    selecting: bool,
    console: bool,
}

impl EventEmitter<EditorEvent> for Editor {}

impl Editor {
    pub fn new(cx: &mut Context<Self>) -> Self {
        Self {
            focus_handle: cx.focus_handle().tab_stop(true),
            doc: Doc::new(""),
            theme: EditorTheme::dark(),
            font_family: DEFAULT_FONT.into(),
            font_size: px(13.),
            line_height: px(19.),
            show_gutter: true,
            always_caret: false,
            errors: Vec::new(),
            marked: None,
            scroll: point(px(0.), px(0.)),
            reveal: false,
            layout: None,
            own_scrollbars: true,
            highlighter: None,
            hl_cache: StateCache::new(),
            read_only: false,
            selecting: false,
            console: false,
        }
    }

    // ---- builder ----

    pub fn with_text(mut self, text: &str) -> Self {
        self.doc.set_text(text);
        self
    }

    pub fn with_theme(mut self, theme: EditorTheme) -> Self {
        self.theme = theme;
        self
    }

    pub fn with_highlighter(mut self, highlighter: impl Highlighter + 'static) -> Self {
        self.highlighter = Some(Box::new(highlighter));
        self
    }

    pub fn with_font(
        mut self,
        family: impl Into<SharedString>,
        size: Pixels,
        line_height: Pixels,
    ) -> Self {
        self.font_family = family.into();
        self.font_size = size;
        self.line_height = line_height;
        self
    }

    pub fn with_gutter(mut self, show: bool) -> Self {
        self.show_gutter = show;
        self
    }

    pub fn with_read_only(mut self, read_only: bool) -> Self {
        self.read_only = read_only;
        self
    }

    // ---- state ----

    pub fn text(&self) -> String {
        self.doc.text()
    }

    pub fn selected_text(&self) -> String {
        self.doc.selected_text()
    }

    pub fn line_count(&self) -> usize {
        self.doc.line_count()
    }

    /// Replaces the whole text; resets caret, scroll and undo history. Emits
    /// no event.
    pub fn set_text(&mut self, text: &str, cx: &mut Context<Self>) {
        self.doc.set_text(text);
        self.hl_cache.reset();
        self.errors.clear();
        self.marked = None;
        self.scroll = point(px(0.), px(0.));
        self.reveal = false;
        cx.notify();
    }

    /// Inserts at the caret, replacing the selection. A no-op when read-only.
    pub fn insert_at_cursor(&mut self, text: &str, cx: &mut Context<Self>) {
        self.edit(cx, |d| d.insert(text));
    }

    /// Console mode, for a REPL input: Enter emits [`EditorEvent::Enter`]
    /// instead of inserting a line (Shift+Enter still does), and Up on the
    /// first line / Down on the last emit the history events instead of moving.
    pub fn set_console_mode(&mut self, console: bool) {
        self.console = console;
    }

    /// The caret is a bare caret right after a letter, digit, `_` or `.`.
    fn after_name_char(&self) -> bool {
        let sel = self.doc.selection();
        if sel.start != sel.end {
            return false;
        }
        self.doc.line(sel.end.line)[..sel.end.col]
            .chars()
            .next_back()
            .is_some_and(|c| c.is_alphanumeric() || c == '_' || c == '.')
    }

    /// Inserts a line break at the caret with the usual auto-indent.
    pub fn newline(&mut self, cx: &mut Context<Self>) {
        self.edit(cx, Doc::newline);
    }

    /// Replaces the whole text and puts the caret at its end.
    pub fn set_text_at_end(&mut self, text: &str, cx: &mut Context<Self>) {
        self.set_text(text, cx);
        self.doc.move_doc_end(false);
        self.after_move(cx);
    }

    pub fn is_read_only(&self) -> bool {
        self.read_only
    }

    pub fn set_read_only(&mut self, read_only: bool, cx: &mut Context<Self>) {
        self.read_only = read_only;
        cx.notify();
    }

    pub fn set_theme(&mut self, theme: EditorTheme, cx: &mut Context<Self>) {
        self.theme = theme;
        cx.notify();
    }

    pub fn set_highlighter(
        &mut self,
        highlighter: Option<Box<dyn Highlighter>>,
        cx: &mut Context<Self>,
    ) {
        self.highlighter = highlighter;
        self.hl_cache.reset();
        cx.notify();
    }

    pub fn set_font(
        &mut self,
        family: impl Into<SharedString>,
        size: Pixels,
        line_height: Pixels,
        cx: &mut Context<Self>,
    ) {
        self.font_family = family.into();
        self.font_size = size;
        self.line_height = line_height;
        cx.notify();
    }

    pub fn set_show_gutter(&mut self, show: bool, cx: &mut Context<Self>) {
        self.show_gutter = show;
        cx.notify();
    }

    /// Characters that make Enter indent the next line one level (default `{`;
    /// use `"{:"` for Python).
    pub fn set_indent_openers(&mut self, openers: &str) {
        self.doc.set_indent_openers(openers);
    }

    /// Paint the caret even without focus (read-only viewers, screenshots).
    pub fn set_always_show_caret(&mut self, always: bool, cx: &mut Context<Self>) {
        self.always_caret = always;
        cx.notify();
    }

    /// Replaces the error markers. Any edit clears them, since their line
    /// numbers no longer describe the text.
    pub fn set_errors(&mut self, errors: Vec<ErrorMarker>, cx: &mut Context<Self>) {
        self.errors = errors;
        cx.notify();
    }

    pub fn clear_errors(&mut self, cx: &mut Context<Self>) {
        self.set_errors(Vec::new(), cx);
    }

    pub fn errors(&self) -> &[ErrorMarker] {
        &self.errors
    }

    pub fn cursor(&self) -> Pos {
        self.doc.head()
    }

    pub fn selection(&self) -> Range<Pos> {
        self.doc.selection()
    }

    /// Selects from `anchor` to `head` and scrolls the head into view.
    pub fn set_selection(&mut self, anchor: Pos, head: Pos, cx: &mut Context<Self>) {
        self.doc.set_selection(anchor, head);
        self.after_move(cx);
    }

    pub fn set_cursor(&mut self, pos: Pos, cx: &mut Context<Self>) {
        self.set_selection(pos, pos, cx);
    }

    /// Scrolls so `line` is the first visible line.
    pub fn scroll_to_line(&mut self, line: usize, cx: &mut Context<Self>) {
        self.scroll.y = self.line_height * line.min(self.doc.line_count() - 1) as f32;
        cx.notify();
    }

    /// Whether the editor paints its own scrollbars (the default).
    pub fn set_own_scrollbars(&mut self, own: bool) {
        self.own_scrollbars = own;
    }

    /// `(viewport, content, offset)` in pixels along x and y, as of the last
    /// paint. `None` before the first one.
    pub fn scroll_metrics(&self) -> Option<[(f32, f32, f32); 2]> {
        let l = self.layout.as_ref()?;
        Some([
            (
                f32::from(l.viewport.width),
                f32::from(l.content.width),
                f32::from(l.scroll.x),
            ),
            (
                f32::from(l.viewport.height),
                f32::from(l.content.height),
                f32::from(l.scroll.y),
            ),
        ])
    }

    /// Scrolls to a pixel offset on one axis (`horizontal` or vertical).
    pub fn scroll_to_offset(&mut self, horizontal: bool, offset: f32, cx: &mut Context<Self>) {
        if horizontal {
            self.scroll.x = px(offset.max(0.));
        } else {
            self.scroll.y = px(offset.max(0.));
        }
        cx.notify();
    }

    pub fn focus(&self, window: &mut Window, cx: &mut App) {
        window.focus(&self.focus_handle, cx);
    }

    // ---- highlighting ----

    /// Spans for `lines`, computing start states only as far as needed.
    pub(crate) fn spans_for(&mut self, lines: Range<usize>) -> Vec<crate::highlight::Spans> {
        let Some(hl) = self.highlighter.as_deref() else {
            return lines.map(|_| Vec::new()).collect();
        };
        let mut state = self.hl_cache.start_state(hl, self.doc.lines(), lines.start);
        lines
            .map(|i| hl.highlight(self.doc.line(i), &mut state))
            .collect()
    }

    // ---- editing plumbing ----

    /// Feeds document changes to the highlight cache and notifies listeners.
    fn sync(&mut self, cx: &mut Context<Self>) {
        let edits = self.doc.take_line_edits();
        let changed = !edits.is_empty();
        for e in edits {
            self.hl_cache.apply_edit(e);
        }
        if changed {
            self.marked = None;
            self.errors.clear();
            cx.emit(EditorEvent::Changed);
        }
        self.after_move(cx);
    }

    fn after_move(&mut self, cx: &mut Context<Self>) {
        self.reveal = true;
        cx.emit(EditorEvent::CursorMoved);
        cx.notify();
    }

    fn edit(&mut self, cx: &mut Context<Self>, f: impl FnOnce(&mut Doc)) {
        if self.read_only {
            return;
        }
        f(&mut self.doc);
        self.sync(cx);
    }

    fn mv(&mut self, cx: &mut Context<Self>, extend: bool, f: impl FnOnce(&mut Doc, bool)) {
        f(&mut self.doc, extend);
        self.after_move(cx);
    }

    fn page(&mut self, cx: &mut Context<Self>, dir: isize, extend: bool) {
        let rows = self
            .layout
            .as_ref()
            .map_or(10, |l| (l.bounds.size.height / l.line_height) as isize - 1)
            .max(1);
        self.mv(cx, extend, |d, e| d.move_lines(dir * rows, e));
    }

    fn undo(&mut self, cx: &mut Context<Self>) {
        self.edit(cx, |d| {
            d.undo();
        });
    }

    fn redo(&mut self, cx: &mut Context<Self>) {
        self.edit(cx, |d| {
            d.redo();
        });
    }

    fn copy(&mut self, cx: &mut Context<Self>) {
        let text = self.doc.selected_text();
        if !text.is_empty() {
            crate::clipboard::write(text, cx);
        }
    }

    fn cut(&mut self, cx: &mut Context<Self>) {
        if self.read_only {
            return self.copy(cx);
        }
        self.copy(cx);
        self.edit(cx, |d| {
            if d.selection().start != d.selection().end {
                d.delete()
            }
        });
    }

    fn paste(&mut self, cx: &mut Context<Self>) {
        if let Some(text) = crate::clipboard::read(cx) {
            self.edit(cx, |d| d.insert(&text));
        }
    }

    // ---- mouse ----

    fn pos_for_point(&self, position: Point<Pixels>) -> Pos {
        let Some(layout) = self.layout.as_ref() else {
            return Pos::default();
        };
        let y = position.y - layout.bounds.top() + layout.scroll.y;
        let line =
            ((y / layout.line_height).floor().max(0.) as usize).min(self.doc.line_count() - 1);
        let x = position.x - layout.text_left + layout.scroll.x;
        let text = self.doc.line(line);
        let col = match line
            .checked_sub(layout.first_line)
            .and_then(|i| layout.lines.get(i))
        {
            Some(Some(shaped)) => {
                text::from_display(text, shaped.closest_index_for_x(x), TAB_WIDTH)
            }
            Some(None) => 0,
            None => text::byte_at_col(
                text,
                (x / layout.char_width).round().max(0.) as usize,
                TAB_WIDTH,
            ),
        };
        Pos { line, col }
    }

    fn on_mouse_down(
        &mut self,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        window.focus(&self.focus_handle, cx);
        let pos = self.pos_for_point(event.position);
        self.selecting = false;
        match event.click_count {
            0 | 1 => {
                let anchor = if event.modifiers.shift {
                    self.doc.anchor()
                } else {
                    pos
                };
                self.doc.set_selection(anchor, pos);
                self.selecting = true;
            }
            2 => self.doc.select_word_at(pos),
            _ => self.doc.select_line(pos.line),
        }
        self.after_move(cx);
    }

    fn on_mouse_move(&mut self, event: &MouseMoveEvent, _: &mut Window, cx: &mut Context<Self>) {
        if self.selecting {
            let pos = self.pos_for_point(event.position);
            self.doc.set_selection(self.doc.anchor(), pos);
            self.after_move(cx);
        }
    }

    fn on_mouse_up(&mut self, _: &MouseUpEvent, _: &mut Window, _: &mut Context<Self>) {
        self.selecting = false;
    }

    fn on_scroll(&mut self, event: &ScrollWheelEvent, _: &mut Window, cx: &mut Context<Self>) {
        let mut delta = event.delta.pixel_delta(self.line_height);
        if event.modifiers.shift {
            delta = point(delta.y, delta.x);
        }
        // Painting clamps to the content size.
        self.scroll.x = (self.scroll.x - delta.x).max(px(0.));
        self.scroll.y = (self.scroll.y - delta.y).max(px(0.));
        cx.notify();
    }

    // ---- UTF-16 helpers for the platform IME ----

    fn range_to_utf16(&self, range: &Range<Pos>) -> Range<usize> {
        self.doc.pos_to_utf16(range.start)..self.doc.pos_to_utf16(range.end)
    }

    fn range_from_utf16(&self, range: &Range<usize>) -> Range<Pos> {
        self.doc.utf16_to_pos(range.start)..self.doc.utf16_to_pos(range.end)
    }
}

impl EntityInputHandler for Editor {
    fn text_for_range(
        &mut self,
        range_utf16: Range<usize>,
        actual_range: &mut Option<Range<usize>>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<String> {
        let range = self.range_from_utf16(&range_utf16);
        actual_range.replace(self.range_to_utf16(&range));
        Some(self.doc.text_in(range))
    }

    fn selected_text_range(
        &mut self,
        _ignore_disabled_input: bool,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        Some(UTF16Selection {
            range: self.range_to_utf16(&self.doc.selection()),
            reversed: self.doc.head() < self.doc.anchor(),
        })
    }

    fn marked_text_range(
        &self,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Range<usize>> {
        self.marked.as_ref().map(|r| self.range_to_utf16(r))
    }

    fn unmark_text(&mut self, _window: &mut Window, _cx: &mut Context<Self>) {
        self.marked = None;
    }

    fn replace_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        new_text: &str,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.read_only {
            return;
        }
        let range = range_utf16
            .map(|r| self.range_from_utf16(&r))
            .or_else(|| self.marked.clone());
        match range {
            Some(r) => self.doc.replace_range(r, new_text),
            None => self.doc.insert(new_text),
        }
        self.sync(cx);
        self.marked = None;
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        new_text: &str,
        new_selected_range_utf16: Option<Range<usize>>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.read_only {
            return;
        }
        let range = range_utf16
            .map(|r| self.range_from_utf16(&r))
            .or_else(|| self.marked.clone())
            .unwrap_or_else(|| self.doc.selection());
        let start16 = self.doc.pos_to_utf16(range.start);
        self.doc.replace_range(range.clone(), new_text);
        let end = self.doc.head();
        self.sync(cx);
        self.marked = (!new_text.is_empty()).then_some(range.start..end);
        if let Some(sel) = new_selected_range_utf16 {
            let anchor = self.doc.utf16_to_pos(start16 + sel.start);
            let head = self.doc.utf16_to_pos(start16 + sel.end);
            self.doc.set_selection(anchor, head);
        }
    }

    fn bounds_for_range(
        &mut self,
        range_utf16: Range<usize>,
        _bounds: Bounds<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        let layout = self.layout.as_ref()?;
        let range = self.range_from_utf16(&range_utf16);
        let left = layout.text_left - layout.scroll.x;
        let top = layout.line_top(range.start.line);
        let start = left + layout.line_x(&self.doc, range.start);
        let end = if range.end.line == range.start.line {
            left + layout.line_x(&self.doc, range.end)
        } else {
            start
        };
        Some(Bounds::new(
            point(start, top),
            gpui::size((end - start).max(px(1.)), layout.line_height),
        ))
    }

    fn character_index_for_point(
        &mut self,
        point: Point<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<usize> {
        let layout = self.layout.as_ref()?;
        layout
            .bounds
            .contains(&point)
            .then(|| self.doc.pos_to_utf16(self.pos_for_point(point)))
    }
}

impl Render for Editor {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .id("zui-editor")
            .key_context("Editor")
            .track_focus(&self.focus_handle)
            .cursor(CursorStyle::IBeam)
            .on_action(cx.listener(|e, _: &Left, _, cx| e.mv(cx, false, Doc::move_left)))
            .on_action(cx.listener(|e, _: &Right, _, cx| e.mv(cx, false, Doc::move_right)))
            .on_action(cx.listener(|e, _: &Up, _, cx| {
                if e.console && e.doc.head().line == 0 && e.doc.selection().is_empty() {
                    cx.emit(EditorEvent::HistoryPrev);
                } else {
                    e.mv(cx, false, Doc::move_up);
                }
            }))
            .on_action(cx.listener(|e, _: &Down, _, cx| {
                if e.console
                    && e.doc.head().line + 1 >= e.doc.line_count()
                    && e.doc.selection().is_empty()
                {
                    cx.emit(EditorEvent::HistoryNext);
                } else {
                    e.mv(cx, false, Doc::move_down);
                }
            }))
            .on_action(cx.listener(|e, _: &SelectLeft, _, cx| e.mv(cx, true, Doc::move_left)))
            .on_action(cx.listener(|e, _: &SelectRight, _, cx| e.mv(cx, true, Doc::move_right)))
            .on_action(cx.listener(|e, _: &SelectUp, _, cx| e.mv(cx, true, Doc::move_up)))
            .on_action(cx.listener(|e, _: &SelectDown, _, cx| e.mv(cx, true, Doc::move_down)))
            .on_action(cx.listener(|e, _: &WordLeft, _, cx| e.mv(cx, false, Doc::move_word_left)))
            .on_action(cx.listener(|e, _: &WordRight, _, cx| e.mv(cx, false, Doc::move_word_right)))
            .on_action(
                cx.listener(|e, _: &SelectWordLeft, _, cx| e.mv(cx, true, Doc::move_word_left)),
            )
            .on_action(
                cx.listener(|e, _: &SelectWordRight, _, cx| e.mv(cx, true, Doc::move_word_right)),
            )
            .on_action(cx.listener(|e, _: &Home, _, cx| e.mv(cx, false, Doc::move_home)))
            .on_action(cx.listener(|e, _: &End, _, cx| e.mv(cx, false, Doc::move_end)))
            .on_action(cx.listener(|e, _: &SelectHome, _, cx| e.mv(cx, true, Doc::move_home)))
            .on_action(cx.listener(|e, _: &SelectEnd, _, cx| e.mv(cx, true, Doc::move_end)))
            .on_action(cx.listener(|e, _: &DocStart, _, cx| e.mv(cx, false, Doc::move_doc_start)))
            .on_action(cx.listener(|e, _: &DocEnd, _, cx| e.mv(cx, false, Doc::move_doc_end)))
            .on_action(
                cx.listener(|e, _: &SelectDocStart, _, cx| e.mv(cx, true, Doc::move_doc_start)),
            )
            .on_action(cx.listener(|e, _: &SelectDocEnd, _, cx| e.mv(cx, true, Doc::move_doc_end)))
            .on_action(cx.listener(|e, _: &PageUp, _, cx| e.page(cx, -1, false)))
            .on_action(cx.listener(|e, _: &PageDown, _, cx| e.page(cx, 1, false)))
            .on_action(cx.listener(|e, _: &SelectPageUp, _, cx| e.page(cx, -1, true)))
            .on_action(cx.listener(|e, _: &SelectPageDown, _, cx| e.page(cx, 1, true)))
            .on_action(cx.listener(|e, _: &SelectAll, _, cx| {
                e.doc.select_all();
                e.after_move(cx);
            }))
            .on_action(cx.listener(|e, _: &Newline, _, cx| {
                if e.console {
                    cx.emit(EditorEvent::Enter);
                } else {
                    e.edit(cx, Doc::newline);
                }
            }))
            .on_action(cx.listener(|e, _: &InsertNewline, _, cx| e.edit(cx, Doc::newline)))
            .on_action(cx.listener(|e, _: &Indent, _, cx| {
                if e.console && e.after_name_char() {
                    cx.emit(EditorEvent::Complete);
                } else {
                    e.edit(cx, Doc::indent);
                }
            }))
            .on_action(cx.listener(|e, _: &Outdent, _, cx| e.edit(cx, Doc::outdent)))
            .on_action(cx.listener(|e, _: &Backspace, _, cx| e.edit(cx, Doc::backspace)))
            .on_action(cx.listener(|e, _: &Delete, _, cx| e.edit(cx, Doc::delete)))
            .on_action(
                cx.listener(|e, _: &DeleteWordLeft, _, cx| e.edit(cx, Doc::delete_word_left)),
            )
            .on_action(
                cx.listener(|e, _: &DeleteWordRight, _, cx| e.edit(cx, Doc::delete_word_right)),
            )
            .on_action(cx.listener(|e, _: &DuplicateLine, _, cx| e.edit(cx, Doc::duplicate_line)))
            .on_action(cx.listener(|e, _: &Undo, _, cx| e.undo(cx)))
            .on_action(cx.listener(|e, _: &Redo, _, cx| e.redo(cx)))
            .on_action(cx.listener(|e, _: &Copy, _, cx| e.copy(cx)))
            .on_action(cx.listener(|e, _: &Cut, _, cx| e.cut(cx)))
            .on_action(cx.listener(|e, _: &Paste, _, cx| e.paste(cx)))
            .on_action(cx.listener(|_, _: &Submit, _, cx| cx.emit(EditorEvent::Submit)))
            .on_mouse_down(MouseButton::Left, cx.listener(Self::on_mouse_down))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::on_mouse_up))
            .on_mouse_up_out(MouseButton::Left, cx.listener(Self::on_mouse_up))
            .on_mouse_move(cx.listener(Self::on_mouse_move))
            .on_scroll_wheel(cx.listener(Self::on_scroll))
            .size_full()
            .overflow_hidden()
            .bg(self.theme.background)
            .child(EditorElement {
                editor: cx.entity(),
            })
    }
}

impl Focusable for Editor {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

/// Registers the editor's keys under the "Editor" key context. Call once at
/// startup.
pub fn bind_keys(cx: &mut App) {
    let ctx = Some("Editor");
    cx.bind_keys([
        KeyBinding::new("left", Left, ctx),
        KeyBinding::new("right", Right, ctx),
        KeyBinding::new("up", Up, ctx),
        KeyBinding::new("down", Down, ctx),
        KeyBinding::new("shift-left", SelectLeft, ctx),
        KeyBinding::new("shift-right", SelectRight, ctx),
        KeyBinding::new("shift-up", SelectUp, ctx),
        KeyBinding::new("shift-down", SelectDown, ctx),
        KeyBinding::new("ctrl-left", WordLeft, ctx),
        KeyBinding::new("ctrl-right", WordRight, ctx),
        KeyBinding::new("ctrl-shift-left", SelectWordLeft, ctx),
        KeyBinding::new("ctrl-shift-right", SelectWordRight, ctx),
        KeyBinding::new("home", Home, ctx),
        KeyBinding::new("end", End, ctx),
        KeyBinding::new("shift-home", SelectHome, ctx),
        KeyBinding::new("shift-end", SelectEnd, ctx),
        KeyBinding::new("ctrl-home", DocStart, ctx),
        KeyBinding::new("ctrl-end", DocEnd, ctx),
        KeyBinding::new("ctrl-shift-home", SelectDocStart, ctx),
        KeyBinding::new("ctrl-shift-end", SelectDocEnd, ctx),
        KeyBinding::new("pageup", PageUp, ctx),
        KeyBinding::new("pagedown", PageDown, ctx),
        KeyBinding::new("shift-pageup", SelectPageUp, ctx),
        KeyBinding::new("shift-pagedown", SelectPageDown, ctx),
        KeyBinding::new("enter", Newline, ctx),
        KeyBinding::new("shift-enter", InsertNewline, ctx),
        KeyBinding::new("ctrl-enter", Submit, ctx),
        KeyBinding::new("tab", Indent, ctx),
        KeyBinding::new("shift-tab", Outdent, ctx),
        KeyBinding::new("backspace", Backspace, ctx),
        KeyBinding::new("shift-backspace", Backspace, ctx),
        KeyBinding::new("delete", Delete, ctx),
        KeyBinding::new("ctrl-backspace", DeleteWordLeft, ctx),
        KeyBinding::new("ctrl-delete", DeleteWordRight, ctx),
        KeyBinding::new("ctrl-z", Undo, ctx),
        KeyBinding::new("ctrl-y", Redo, ctx),
        KeyBinding::new("ctrl-shift-z", Redo, ctx),
        KeyBinding::new("ctrl-a", SelectAll, ctx),
        KeyBinding::new("ctrl-c", Copy, ctx),
        KeyBinding::new("ctrl-x", Cut, ctx),
        KeyBinding::new("ctrl-v", Paste, ctx),
        KeyBinding::new("ctrl-d", DuplicateLine, ctx),
    ]);
}
