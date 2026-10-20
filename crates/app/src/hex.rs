// Hex view. Rows are 16 bytes and map onto segments only, so sparse 64-bit
// layouts stay small. Bytes arrive in 4 KiB pages fetched on demand.
use crate::actions::*;
use crate::code::{self, line_height};
use crate::scrollbar::{self, Axes, Axis, Metrics, Source};
use crate::selection::{Anchor, BytesFormat, bytes_text};
use crate::session::{Origin, Session, SessionEvent, ViewKind};
use crate::settings;
use crate::theme::{Theme, hsla};
use gpui::{
    App, Bounds, Context, Entity, FocusHandle, Focusable, KeyDownEvent, MouseButton,
    MouseDownEvent, MouseMoveEvent, Pixels, Point, ScrollWheelEvent, Subscription, Window, canvas,
    div, point, prelude::*, px,
};
use ida_proto::{ByteChunk, Bytes, DbgMemRead, Ea, PatchBytes, PatchList, SegRow, Segments};
use std::collections::{HashMap, HashSet};

const PAGE: u64 = 4096;
const ROW: u64 = 16;
const MAX_PAGES: usize = 512;

/// Where the columns of a row sit, in characters from the view's left edge:
/// 18 characters of address, a gap, then 16 bytes as `XX ` with one extra
/// space between groups of `group` bytes, a gap, and the ASCII.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Cols {
    /// Bytes per group: 1, 2, 4 or 8.
    pub group: u64,
}

impl Cols {
    pub const HEX0: f32 = 20.;

    pub fn new(group: u64) -> Self {
        Self {
            group: if matches!(group, 1 | 2 | 4 | 8) {
                group
            } else {
                8
            },
        }
    }

    /// The first character of byte `col` (0..16) in the hex column.
    pub fn hex_x(&self, col: u64) -> f32 {
        Self::HEX0 + col as f32 * 3. + (col / self.group) as f32
    }

    /// The first character of the ASCII column.
    pub fn ascii_x(&self) -> f32 {
        Self::HEX0 + 48. + (ROW / self.group - 1) as f32 + 2.
    }

    /// The byte under character `x`, in either column.
    pub fn col_at(&self, x: f32) -> Option<u64> {
        let hex_end = self.hex_x(ROW - 1) + 2.;
        if x >= Self::HEX0 && x < hex_end + 1. {
            // Each byte owns its two digits and the space after them.
            let col = (0..ROW).rev().find(|c| x >= self.hex_x(*c))?;
            return Some(col);
        }
        let a = self.ascii_x();
        (x >= a && x < a + ROW as f32).then_some((x - a) as u64)
    }

    /// The header row: `00 01 02 ...` spaced like the bytes below it.
    pub fn header(&self) -> String {
        let mut out = String::new();
        for i in 0..ROW {
            if i > 0 && i % self.group == 0 {
                out.push(' ');
            }
            out.push_str(&format!("{i:02X} "));
        }
        out
    }
}

/// Overtyping bytes in the hex view (F2): the digits typed since the caret
/// was where `start` is. Nothing reaches the database until it is committed.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct HexEdit {
    pub start: Ea,
    pub bytes: Vec<u8>,
    /// The first digit of a byte whose second is still to come.
    pub half: Option<u8>,
}

impl HexEdit {
    pub fn new(start: Ea) -> Self {
        Self {
            start,
            bytes: Vec::new(),
            half: None,
        }
    }

    /// A typed character; false when it is not a hex digit.
    pub fn digit(&mut self, c: char) -> bool {
        let Some(d) = c.to_digit(16) else {
            return false;
        };
        match self.half.take() {
            Some(hi) => self.bytes.push((hi << 4) | d as u8),
            None => self.half = Some(d as u8),
        }
        true
    }

    pub fn backspace(&mut self) {
        if self.half.take().is_none() {
            self.bytes.pop();
        }
    }

    pub fn is_empty(&self) -> bool {
        self.bytes.is_empty() && self.half.is_none()
    }

    /// Where the caret is: the next byte to be written.
    pub fn caret(&self) -> Ea {
        self.start + self.bytes.len() as Ea
    }

    /// The bytes to patch, as hex pairs; `None` when none are complete.
    pub fn hex(&self) -> Option<String> {
        (!self.bytes.is_empty()).then(|| {
            self.bytes
                .iter()
                .map(|b| format!("{b:02X}"))
                .collect::<Vec<_>>()
                .join(" ")
        })
    }
}

pub struct HexView {
    session: Entity<Session>,
    /// This view\'s identity in `SessionEvent::CursorMoved`.
    origin: Origin,
    focus: FocusHandle,
    segs: Vec<SegRow>,
    /// Cumulative row index where each segment starts.
    seg_rows: Vec<u64>,
    total_rows: u64,
    pages: HashMap<Ea, ByteChunk>,
    pending: HashSet<Ea>,
    top: f64,
    cursor: Ea,
    viewport_rows: usize,
    bounds: Option<Bounds<Pixels>>,
    char_w: f32,
    pub visible: bool,
    /// Show the suspended process's memory instead of the database's bytes.
    process: bool,
    /// The other end of the byte selection (the caret is this end).
    sel: Anchor,
    /// The byte a mouse drag started on while the button is held.
    drag_from: Option<Ea>,
    /// Column layout (bytes per group).
    cols: Cols,
    /// Patched bytes, drawn in their own colour.
    patched: HashSet<Ea>,
    /// Overtyping bytes (F2); `None` when not editing.
    edit: Option<HexEdit>,
    /// Drag-select past an edge: which way, and whether the ticker runs.
    auto_dir: isize,
    auto_running: bool,
    /// The viewport has been measured (until then `show` centres on a guess).
    measured: bool,
    _subs: Vec<Subscription>,
}

/// The header row above the bytes: `00 01 ... 0F`.
const HEADER_ROWS: f32 = 1.;

/// Selections longer than this are not copied (the clipboard is not a file).
const MAX_COPY: u64 = 1 << 20;

impl HexView {
    pub fn new(session: Entity<Session>, cx: &mut Context<Self>) -> Self {
        let sub = cx.subscribe(&session, |this, _, ev, cx| match ev {
            SessionEvent::Opened => {
                this.load_segments(cx);
                this.load_patches(cx);
            }
            SessionEvent::Navigated { ea, .. } => this.show(*ea, true, cx),
            SessionEvent::CursorMoved { ea, origin } if *origin != this.origin => {
                this.show(*ea, false, cx)
            }
            SessionEvent::Changed => {
                this.pages.clear();
                this.load_patches(cx);
                cx.notify();
            }
            SessionEvent::DebugChanged => {
                // The process ran or ended since the pages were read.
                if this.process {
                    this.pages.clear();
                    this.pending.clear();
                }
                cx.notify();
            }
            SessionEvent::Rebased { .. } => {
                this.pages.clear();
                this.load_segments(cx);
            }
            SessionEvent::Closed => {
                this.segs.clear();
                this.pages.clear();
                this.patched.clear();
                this.edit = None;
                cx.notify();
            }
            _ => {}
        });
        Self {
            session,
            origin: Origin::new(ViewKind::Hex),
            focus: cx.focus_handle(),
            segs: Vec::new(),
            seg_rows: Vec::new(),
            total_rows: 0,
            pages: HashMap::new(),
            pending: HashSet::new(),
            top: 0.,
            cursor: 0,
            viewport_rows: 40,
            bounds: None,
            char_w: 7.8,
            visible: false,
            process: false,
            sel: Anchor::default(),
            drag_from: None,
            cols: Cols::new(settings::get(cx).hex_group),
            patched: HashSet::new(),
            edit: None,
            measured: false,
            auto_dir: 0,
            auto_running: false,
            _subs: vec![sub],
        }
    }

    /// Which bytes are patched, for the colour.
    fn load_patches(&mut self, cx: &mut Context<Self>) {
        if !self.session.read(cx).is_ready() {
            return;
        }
        let fut = self.session.read(cx).call(PatchList {});
        cx.spawn(async move |this, cx| {
            if let Ok(rows) = fut.await {
                this.update(cx, |v, cx| {
                    v.patched = rows.into_iter().map(|r| r.ea).collect();
                    cx.notify();
                })
                .ok();
            }
        })
        .detach();
    }

    /// F2: starts overtyping bytes at the caret, or commits what was typed.
    fn toggle_edit(&mut self, cx: &mut Context<Self>) {
        match self.edit.take() {
            Some(e) => self.commit_edit(e, cx),
            None => {
                if self.shows_process(cx) {
                    self.session.update(cx, |s, cx| {
                        s.info_msg("Process memory cannot be edited here", cx)
                    });
                    return;
                }
                self.sel.clear();
                self.edit = Some(HexEdit::new(self.cursor));
                self.session.update(cx, |s, cx| {
                    s.info_msg(
                        "Hex edit: type hex digits, Enter or F2 applies, Esc cancels",
                        cx,
                    )
                });
            }
        }
        cx.notify();
    }

    /// Writes what was typed as a patch (the originals stay recoverable).
    fn commit_edit(&mut self, e: HexEdit, cx: &mut Context<Self>) {
        if e.is_empty() {
            return;
        }
        if let Some(hex) = e.hex() {
            self.session
                .update(cx, |s, cx| s.mutate(PatchBytes { ea: e.start, hex }, cx));
        }
        self.cursor = e.caret();
    }

    /// Keys while overtyping: digits write, Backspace takes back, Enter
    /// applies, Esc cancels. (The view's key context is `HexEdit` then, so
    /// the one-letter commands do not fire.)
    fn on_edit_key(&mut self, ev: &KeyDownEvent, cx: &mut Context<Self>) {
        let Some(edit) = self.edit.as_mut() else {
            return;
        };
        let key = ev.keystroke.key.as_str();
        match key {
            "enter" => {
                if let Some(e) = self.edit.take() {
                    self.commit_edit(e, cx);
                }
            }
            "escape" => {
                if let Some(e) = self.edit.take() {
                    self.cursor = e.start;
                }
            }
            "backspace" => edit.backspace(),
            _ => {
                let typed = ev.keystroke.key_char.as_deref().unwrap_or(key);
                let mut chars = typed.chars();
                match (chars.next(), chars.next()) {
                    (Some(c), None)
                        if !ev.keystroke.modifiers.control
                            && !ev.keystroke.modifiers.alt
                            && edit.digit(c) =>
                    {
                        self.cursor = edit.caret();
                    }
                    _ => return,
                }
            }
        }
        cx.stop_propagation();
        cx.notify();
    }

    /// Group control: bytes per group.
    fn set_group(&mut self, group: u64, cx: &mut Context<Self>) {
        self.cols = Cols::new(group);
        settings::update(cx, move |s| s.hex_group = group);
        cx.notify();
    }

    fn load_segments(&mut self, cx: &mut Context<Self>) {
        let fut = self.session.read(cx).call(Segments {});
        cx.spawn(async move |this, cx| {
            if let Ok(segs) = fut.await {
                this.update(cx, |v, cx| {
                    v.seg_rows.clear();
                    let mut rows = 0;
                    for s in &segs {
                        v.seg_rows.push(rows);
                        rows += (s.end - (s.start & !(ROW - 1))).div_ceil(ROW);
                    }
                    v.total_rows = rows;
                    v.segs = segs;
                    let ea = v.session.read(cx).cursor;
                    v.show(ea, true, cx);
                })
                .ok();
            }
        })
        .detach();
    }

    fn row_of(&self, ea: Ea) -> Option<u64> {
        let i = self.segs.iter().position(|s| ea >= s.start && ea < s.end)?;
        let base = self.segs[i].start & !(ROW - 1);
        Some(self.seg_rows[i] + (ea - base) / ROW)
    }

    fn ea_of_row(&self, row: u64) -> Option<Ea> {
        let i = self
            .seg_rows
            .partition_point(|&r| r <= row)
            .checked_sub(1)?;
        let base = self.segs[i].start & !(ROW - 1);
        Some(base + (row - self.seg_rows[i]) * ROW)
    }

    pub fn show(&mut self, ea: Ea, center: bool, cx: &mut Context<Self>) {
        let Some(row) = self.row_of(ea) else { return };
        self.cursor = ea;
        let vp = self.viewport_rows.max(2) as f64;
        let r = row as f64;
        if center || r < self.top || r >= self.top + vp - 1. {
            self.top = (r - vp / 3.).max(0.);
        }
        cx.notify();
    }

    fn byte(&self, ea: Ea) -> Option<Option<u8>> {
        let page = ea & !(PAGE - 1);
        let chunk = self.pages.get(&page)?;
        Some(chunk.get((ea - page) as usize))
    }

    /// Process memory is shown only while the process is suspended.
    fn shows_process(&self, cx: &App) -> bool {
        self.process && self.session.read(cx).dbg.suspended()
    }

    /// Debugger > Hex view of process memory: switches the bytes between the
    /// database and the suspended process (re-read at every stop).
    pub fn toggle_process_memory(&mut self, cx: &mut Context<Self>) {
        if !self.process && !self.session.read(cx).dbg.suspended() {
            self.session.update(cx, |s, cx| {
                s.info_msg(
                    "Process memory is available while a process is suspended",
                    cx,
                )
            });
            return;
        }
        self.process = !self.process;
        self.pages.clear();
        self.pending.clear();
        let msg = if self.process {
            "Hex view shows process memory"
        } else {
            "Hex view shows the database"
        };
        self.session.update(cx, |s, cx| s.info_msg(msg, cx));
        cx.notify();
    }

    fn request_pages(&mut self, first_row: u64, rows: usize, cx: &mut Context<Self>) {
        let mut want = Vec::new();
        for r in first_row..(first_row + rows as u64).min(self.total_rows) {
            if let Some(ea) = self.ea_of_row(r) {
                let page = ea & !(PAGE - 1);
                if !self.pages.contains_key(&page)
                    && !self.pending.contains(&page)
                    && !want.contains(&page)
                {
                    want.push(page);
                }
            }
        }
        if want.is_empty() {
            return;
        }
        if self.pages.len() > MAX_PAGES {
            self.pages.clear();
        }
        let process = self.shows_process(cx);
        for page in want {
            self.pending.insert(page);
            let fut = if process {
                let call = self.session.read(cx).call(DbgMemRead {
                    ea: page,
                    len: PAGE as u32,
                });
                futures::future::Either::Left(async move {
                    call.await.map(|m| ByteChunk {
                        ea: m.ea,
                        hex: m.hex,
                    })
                })
            } else {
                futures::future::Either::Right(self.session.read(cx).call(Bytes {
                    ea: page,
                    len: PAGE as u32,
                }))
            };
            cx.spawn(async move |this, cx| {
                let result = fut.await;
                this.update(cx, |v, cx| {
                    v.pending.remove(&page);
                    if let Ok(chunk) = result {
                        v.pages.insert(page, chunk);
                        cx.notify();
                    }
                })
                .ok();
            })
            .detach();
        }
    }

    fn move_rows(&mut self, delta: i64, cx: &mut Context<Self>) {
        self.sel.clear();
        self.step_rows(delta, cx);
    }

    /// Shift+Up / Down / PageUp / PageDown: extends the selection by rows.
    fn extend_rows(&mut self, delta: i64, cx: &mut Context<Self>) {
        self.sel.begin(self.cursor as usize);
        self.step_rows(delta, cx);
    }

    /// Shift+Home / End: to the start / end of the caret's row.
    fn extend_row_edge(&mut self, end: bool, cx: &mut Context<Self>) {
        self.sel.begin(self.cursor as usize);
        let base = self.cursor & !(ROW - 1);
        self.cursor = if end { base + ROW - 1 } else { base };
        let ea = self.cursor;
        self.session
            .update(cx, |s, cx| s.set_cursor(ea, self.origin, cx));
        cx.notify();
    }

    fn step_rows(&mut self, delta: i64, cx: &mut Context<Self>) {
        let Some(row) = self.row_of(self.cursor) else {
            return;
        };
        let col = self.cursor % ROW;
        let target = (row as i64 + delta).clamp(0, self.total_rows as i64 - 1) as u64;
        if let Some(ea) = self.ea_of_row(target) {
            self.cursor = ea + col;
            self.show(self.cursor, false, cx);
            let ea = self.cursor;
            self.session
                .update(cx, |s, cx| s.set_cursor(ea, self.origin, cx));
        }
    }

    fn on_scroll(&mut self, ev: &ScrollWheelEvent, _: &mut Window, cx: &mut Context<Self>) {
        let delta = ev.delta.pixel_delta(px(line_height()));
        let max = (self.total_rows as f64 - 2.).max(0.);
        self.top = (self.top - f64::from(f32::from(delta.y)) / line_height() as f64).clamp(0., max);
        cx.notify();
    }

    /// The byte under `pos`, in the hex or the ASCII column.
    fn byte_at(&self, pos: Point<Pixels>) -> Option<Ea> {
        let b = self.bounds?;
        let y = f32::from(pos.y - b.top()) - HEADER_ROWS * line_height();
        if y < 0. {
            return None;
        }
        let row = (self.top.floor()
            + ((y + (self.top.fract() as f32) * line_height()) / line_height()).floor() as f64)
            as u64;
        let x = f32::from(pos.x - b.left()) / self.char_w;
        let col = self.cols.col_at(x)?;
        Some(self.ea_of_row(row)? + col.min(15))
    }

    fn on_mouse_down(&mut self, ev: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.focus, cx);
        if let Some(byte) = self.byte_at(ev.position) {
            if ev.modifiers.shift {
                self.sel.begin(self.cursor as usize);
            } else {
                self.sel.clear();
                self.drag_from = Some(byte);
            }
            self.cursor = byte;
            let ea = self.cursor;
            if ev.click_count >= 2 {
                self.session.update(cx, |s, cx| s.navigate(ea, cx));
            } else {
                self.session
                    .update(cx, |s, cx| s.set_cursor(ea, self.origin, cx));
            }
            cx.notify();
        }
    }

    /// Where a keyboard-opened context menu appears: under the caret's row.
    fn caret_position(&self) -> Point<Pixels> {
        let (Some(b), Some(row)) = (self.bounds, self.row_of(self.cursor)) else {
            return point(px(200.), px(200.));
        };
        let y = (row as f64 - self.top + 1. + f64::from(HEADER_ROWS)) as f32 * line_height();
        let col = self.cursor % ROW;
        b.origin + point(px(self.cols.hex_x(col) * self.char_w), px(y))
    }

    /// Right click: the caret goes to the clicked byte first (unless it is
    /// inside the selection), then the menu opens.
    fn on_right_down(&mut self, ev: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.focus, cx);
        if let Some(byte) = self.byte_at(ev.position) {
            let inside = self
                .sel
                .range(self.cursor as usize)
                .is_some_and(|r| r.contains(&(byte as usize)));
            if !inside {
                self.sel.clear();
                self.cursor = byte;
                self.session
                    .update(cx, |s, cx| s.set_cursor(byte, self.origin, cx));
            }
        }
        self.open_context_menu(ev.position, false, cx);
        cx.notify();
    }

    fn open_context_menu(&mut self, pos: Point<Pixels>, keyboard: bool, cx: &mut Context<Self>) {
        let entries = crate::context_menu::hex(cx);
        let title = format!("{:X}", self.cursor);
        crate::context_menu::show(&self.session, pos, title, entries, keyboard, cx);
    }

    /// Dragging with the left button held selects from the byte it started on.
    fn on_mouse_move(&mut self, ev: &MouseMoveEvent, _: &mut Window, cx: &mut Context<Self>) {
        let Some(from) = self.drag_from else { return };
        if ev.pressed_button != Some(MouseButton::Left) {
            self.drag_from = None;
            return;
        }
        if let Some(b) = self.bounds {
            self.auto_dir = crate::selection::edge_direction(
                f32::from(ev.position.y),
                f32::from(b.top()),
                f32::from(b.bottom()),
            );
            if self.auto_dir != 0 {
                self.start_autoscroll(cx);
                return;
            }
        }
        if let Some(byte) = self.byte_at(ev.position)
            && byte != self.cursor
        {
            self.sel.begin(from as usize);
            self.cursor = byte;
            self.session
                .update(cx, |s, cx| s.set_cursor(byte, self.origin, cx));
            cx.notify();
        }
    }

    fn auto_tick(&mut self, cx: &mut Context<Self>) {
        if let Some(from) = self.drag_from {
            self.sel.begin(from as usize);
            self.step_rows(self.auto_dir as i64, cx);
        }
    }

    /// Past the top or bottom edge with the button down: one line per tick.
    fn start_autoscroll(&mut self, cx: &mut Context<Self>) {
        if self.auto_running {
            return;
        }
        self.auto_running = true;
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(std::time::Duration::from_millis(60))
                    .await;
                let go = this
                    .update(cx, |v, cx| {
                        if v.drag_from.is_none() || v.auto_dir == 0 {
                            v.auto_running = false;
                            return false;
                        }
                        v.auto_tick(cx);
                        true
                    })
                    .unwrap_or(false);
                if !go {
                    break;
                }
            }
        })
        .detach();
    }

    /// Esc: with a selection it only clears the selection; otherwise it is
    /// "back" as everywhere (the action keeps travelling up).
    fn on_back(&mut self, cx: &mut Context<Self>) {
        if self.sel.active(self.cursor as usize) {
            self.sel.clear();
            cx.notify();
        } else {
            cx.propagate();
        }
    }

    /// The selected bytes (start, length), or the one under the caret.
    pub fn selection_span(&self) -> (Ea, u64) {
        let (lo, hi) = self.selected_range();
        (lo, hi - lo + 1)
    }

    /// The selected bytes, or the one under the caret.
    fn selected_range(&self) -> (Ea, Ea) {
        match self.sel.range(self.cursor as usize) {
            Some(r) => (
                *r.start() as Ea,
                (*r.end() as Ea).min(*r.start() as Ea + MAX_COPY - 1),
            ),
            None => (self.cursor, self.cursor),
        }
    }

    /// Copy / Copy as C array / Copy as Python bytes. Pages not loaded yet are
    /// read first. With `dump`, plain Copy becomes "Copy with addresses": a
    /// hex dump with the row address in front.
    fn copy(&mut self, format: BytesFormat, dump: bool, cx: &mut Context<Self>) {
        let (lo, hi) = self.selected_range();
        let process = self.shows_process(cx);
        let mut futs = Vec::new();
        let mut page = lo & !(PAGE - 1);
        while page <= hi {
            if !self.pages.contains_key(&page) {
                futs.push(self.read_page(page, process, cx));
            }
            page += PAGE;
        }
        let have: HashMap<Ea, ByteChunk> =
            self.pages.iter().map(|(k, v)| (*k, v.clone())).collect();
        cx.spawn(async move |_, cx| {
            let mut pages = have;
            for (page, chunk) in futures::future::join_all(futs).await.into_iter().flatten() {
                pages.insert(page, chunk);
            }
            let bytes: Vec<u8> = (lo..=hi)
                .map(|ea| {
                    let page = ea & !(PAGE - 1);
                    pages
                        .get(&page)
                        .and_then(|c| c.get((ea - page) as usize))
                        .unwrap_or(0)
                })
                .collect();
            let text = if dump {
                bytes
                    .chunks(ROW as usize)
                    .enumerate()
                    .map(|(i, row)| {
                        format!(
                            "{:016X}  {}",
                            lo + i as u64 * ROW,
                            bytes_text(row, BytesFormat::Hex)
                        )
                    })
                    .collect::<Vec<_>>()
                    .join("\n")
            } else {
                bytes_text(&bytes, format)
            };
            cx.update(|cx| zui_editor::clipboard::write(text, cx));
        })
        .detach();
    }

    /// One page of bytes, tagged with its address.
    fn read_page(
        &self,
        page: Ea,
        process: bool,
        cx: &App,
    ) -> futures::future::LocalBoxFuture<'static, Option<(Ea, ByteChunk)>> {
        let s = self.session.read(cx);
        if process {
            let call = s.call(DbgMemRead {
                ea: page,
                len: PAGE as u32,
            });
            Box::pin(async move {
                let m = call.await.ok()?;
                Some((
                    page,
                    ByteChunk {
                        ea: m.ea,
                        hex: m.hex,
                    },
                ))
            })
        } else {
            let call = s.call(Bytes {
                ea: page,
                len: PAGE as u32,
            });
            Box::pin(async move { Some((page, call.await.ok()?)) })
        }
    }

    fn render_canvas(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let entity = cx.entity();
        canvas(
            |_, _, _| (),
            move |bounds, (), window, cx| {
                let theme = Theme::get(cx);
                let cw = code::char_width(window);
                let v = entity.read(cx);
                let marks = code::DebugMarks::read(v.session.read(cx));
                let first = v.top.floor() as u64;
                let frac = (v.top - v.top.floor()) as f32;
                let head_h = px(HEADER_ROWS * line_height());
                let rows = ((f32::from(bounds.size.height) - f32::from(head_h)) / line_height())
                    .ceil() as usize
                    + 1;
                let cursor = v.cursor;
                let selected = v.sel.range(cursor as usize);
                let cols = v.cols;
                let patched = v.patched.clone();
                let edit = v.edit.clone();
                let mut lines = Vec::new();
                for k in 0..rows {
                    let r = first + k as u64;
                    if r >= v.total_rows {
                        break;
                    }
                    let Some(base) = v.ea_of_row(r) else { break };
                    let bytes: Vec<Option<Option<u8>>> =
                        (0..ROW).map(|i| v.byte(base + i)).collect();
                    lines.push((k, base, bytes));
                }
                let focused = v.focus.is_focused(window);
                let rows_bounds = Bounds::new(
                    point(bounds.left(), bounds.top() + head_h),
                    gpui::size(bounds.size.width, bounds.size.height - head_h),
                );
                let cell = |col: u64| (cols.hex_x(col), cols.ascii_x() + col as f32);
                window.with_content_mask(
                    Some(gpui::ContentMask {
                        bounds: rows_bounds,
                    }),
                    |window| {
                        for (k, base, bytes) in lines {
                            let y = rows_bounds.top() + px((k as f32 - frac) * line_height());
                            // The selected bytes of this row: the hex cells and the ASCII.
                            if let Some(sel) = &selected {
                                let (lo, hi) = (*sel.start() as Ea, *sel.end() as Ea);
                                if hi >= base && lo < base + ROW {
                                    let a = lo.max(base) - base;
                                    let b = hi.min(base + ROW - 1) - base;
                                    for (x, w) in [
                                        (cell(a).0, cell(b).0 + 2. - cell(a).0),
                                        (cell(a).1, (b - a + 1) as f32),
                                    ] {
                                        window.paint_quad(gpui::fill(
                                            Bounds::new(
                                                point(bounds.left() + cw * x, y),
                                                gpui::size(cw * w, px(line_height())),
                                            ),
                                            hsla(theme.selection),
                                        ));
                                    }
                                }
                            }
                            if cursor >= base && cursor < base + ROW && selected.is_none() {
                                code::paint_row_bg(rows_bounds, y, hsla(theme.caret_row), window);
                                let (hx, ax) = cell(cursor - base);
                                for (x, w) in [(hx, 2.), (ax, 1.)] {
                                    window.paint_quad(gpui::fill(
                                        Bounds::new(
                                            point(bounds.left() + cw * x, y),
                                            gpui::size(cw * w, px(line_height())),
                                        ),
                                        if focused {
                                            hsla(theme.selection)
                                        } else {
                                            hsla(theme.raised)
                                        },
                                    ));
                                }
                            }
                            // The instruction pointer and breakpoint bytes.
                            let tints = marks
                                .ip
                                .map(|ip| (ip, hsla(theme.accent).opacity(0.45)))
                                .into_iter()
                                .chain(
                                    marks
                                        .bpts
                                        .iter()
                                        .filter(|(_, on)| *on)
                                        .map(|(ea, _)| (*ea, hsla(theme.danger).opacity(0.4))),
                                );
                            for (ea, color) in tints {
                                if ea >= base && ea < base + ROW {
                                    let (hx, ax) = cell(ea - base);
                                    for (x, w) in [(hx, 2.), (ax, 1.)] {
                                        window.paint_quad(gpui::fill(
                                            Bounds::new(
                                                point(bounds.left() + cw * x, y),
                                                gpui::size(cw * w, px(line_height())),
                                            ),
                                            color,
                                        ));
                                    }
                                }
                            }
                            // The bytes: patched ones and bytes being typed in their own colours.
                            let (normal, patched_c, edited) =
                                (hsla(theme.text), hsla(theme.warning), hsla(theme.accent));
                            let mut hex = String::with_capacity(52);
                            let mut hex_runs: Vec<(usize, gpui::Hsla)> = Vec::new();
                            let mut ascii = String::with_capacity(16);
                            let mut ascii_runs: Vec<(usize, gpui::Hsla)> = Vec::new();
                            for (i, b) in bytes.iter().enumerate() {
                                let ea = base + i as u64;
                                if i > 0 && (i as u64).is_multiple_of(cols.group) {
                                    hex.push(' ');
                                    hex_runs.push((1, normal));
                                }
                                let typed = edit.as_ref().and_then(|e| {
                                    if ea >= e.start && ea < e.caret() {
                                        Some(Some(e.bytes[(ea - e.start) as usize]))
                                    } else if ea == e.caret() {
                                        e.half.map(|_| None)
                                    } else {
                                        None
                                    }
                                });
                                let (cell_hex, glyph, color) = match (typed, b) {
                                    (Some(Some(t)), _) => (
                                        format!("{t:02X} "),
                                        if t.is_ascii_graphic() || t == b' ' {
                                            t as char
                                        } else {
                                            '.'
                                        },
                                        edited,
                                    ),
                                    (Some(None), _) => (
                                        format!(
                                            "{:X}_ ",
                                            edit.as_ref().and_then(|e| e.half).unwrap_or(0)
                                        ),
                                        ' ',
                                        edited,
                                    ),
                                    (None, Some(Some(b))) => (
                                        format!("{b:02X} "),
                                        if b.is_ascii_graphic() || *b == b' ' {
                                            *b as char
                                        } else {
                                            '.'
                                        },
                                        if patched.contains(&ea) {
                                            patched_c
                                        } else {
                                            normal
                                        },
                                    ),
                                    (None, Some(None)) => ("?? ".to_owned(), ' ', normal),
                                    (None, None) => ("   ".to_owned(), ' ', normal),
                                };
                                hex_runs.push((cell_hex.len(), color));
                                hex.push_str(&cell_hex);
                                ascii.push(glyph);
                                ascii_runs.push((
                                    glyph.len_utf8(),
                                    if color == normal {
                                        hsla(theme.syntax.str)
                                    } else {
                                        color
                                    },
                                ));
                            }
                            let addr = code::shape_plain(
                                window,
                                &format!("{base:016X}"),
                                hsla(theme.faint),
                            );
                            code::paint_shaped(
                                &addr,
                                point(bounds.left() + cw * 1., y),
                                window,
                                cx,
                            );
                            let h = code::shape_colored(window, &hex, &hex_runs);
                            code::paint_shaped(
                                &h,
                                point(bounds.left() + cw * Cols::HEX0, y),
                                window,
                                cx,
                            );
                            let a = code::shape_colored(window, &ascii, &ascii_runs);
                            code::paint_shaped(
                                &a,
                                point(bounds.left() + cw * cols.ascii_x(), y),
                                window,
                                cx,
                            );
                        }
                    },
                );
                // The header: offsets over the byte columns (the rows scroll under it).
                window.paint_quad(gpui::fill(
                    Bounds::new(bounds.origin, gpui::size(bounds.size.width, head_h)),
                    hsla(theme.surface2),
                ));
                window.paint_quad(gpui::fill(
                    Bounds::new(
                        point(bounds.left(), bounds.top() + head_h - px(1.)),
                        gpui::size(bounds.size.width, px(1.)),
                    ),
                    hsla(theme.border),
                ));
                let label = code::shape_plain(window, "Address", hsla(theme.faint));
                code::paint_shaped(
                    &label,
                    point(bounds.left() + cw * 1., bounds.top()),
                    window,
                    cx,
                );
                let head = code::shape_plain(window, &cols.header(), hsla(theme.muted));
                code::paint_shaped(
                    &head,
                    point(bounds.left() + cw * Cols::HEX0, bounds.top()),
                    window,
                    cx,
                );
                let viewport =
                    ((f32::from(bounds.size.height) / line_height()) - HEADER_ROWS) as usize;
                entity.update(cx, |v, cx| {
                    v.bounds = Some(bounds);
                    v.char_w = f32::from(cw);
                    v.viewport_rows = viewport.max(1);
                    if !v.measured {
                        // The first layout: the caret was placed for a guessed height.
                        v.measured = true;
                        let at = v.cursor;
                        v.show(at, true, cx);
                    }
                    v.request_pages(first, rows, cx);
                });
            },
        )
        .size_full()
    }
}

impl Focusable for HexView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for HexView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::get(cx);
        let (m, st) = (cx.entity().downgrade(), cx.entity().downgrade());
        let source = Source::custom(
            move |axis, cx| {
                let v = m.upgrade()?;
                let v = v.read(cx);
                (axis == Axis::Vertical && v.total_rows > 0).then_some(Metrics {
                    viewport: v.viewport_rows as f32,
                    content: v.total_rows as f32,
                    offset: v.top as f32,
                })
            },
            move |_, rows, cx| {
                if let Some(v) = st.upgrade() {
                    v.update(cx, |v, cx| {
                        v.top = f64::from(rows).clamp(0., (v.total_rows as f64 - 2.).max(0.));
                        cx.notify();
                    });
                }
            },
        );
        let bar = scrollbar::scrollbar("hex-scroll", source, Axes::V, &theme, window, cx);
        let page = self.viewport_rows.max(2) as i64 - 1;
        let editing = self.edit.is_some();
        let group = self.cols.group;
        let t = theme;
        div()
            .id("hex")
            .key_context(if editing { "HexEdit" } else { "CodeView" })
            .track_focus(&self.focus)
            .on_key_down(cx.listener(|v, ev: &KeyDownEvent, _, cx| v.on_edit_key(ev, cx)))
            // F2 is "toggle breakpoint" elsewhere; here it is IDA's hex edit mode.
            .on_action(cx.listener(|v, _: &ToggleBreakpoint, _, cx| v.toggle_edit(cx)))
            .size_full()
            .relative()
            .overflow_hidden()
            .bg(hsla(theme.surface))
            .on_scroll_wheel(cx.listener(Self::on_scroll))
            .on_mouse_down(MouseButton::Left, cx.listener(Self::on_mouse_down))
            .on_mouse_down(MouseButton::Right, cx.listener(Self::on_right_down))
            .on_action(cx.listener(|v, _: &ShowContextMenu, _, cx| {
                let pos = v.caret_position();
                v.open_context_menu(pos, true, cx)
            }))
            .on_mouse_move(cx.listener(Self::on_mouse_move))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|v, _, _, _| v.drag_from = None),
            )
            .on_mouse_up_out(
                MouseButton::Left,
                cx.listener(|v, _, _, _| v.drag_from = None),
            )
            .on_action(cx.listener(|v, _: &Back, _, cx| v.on_back(cx)))
            .on_action(cx.listener(|v, _: &Copy, _, cx| v.copy(BytesFormat::Hex, false, cx)))
            .on_action(
                cx.listener(|v, _: &CopyWithAddresses, _, cx| v.copy(BytesFormat::Hex, true, cx)),
            )
            .on_action(
                cx.listener(|v, _: &CopyAsCArray, _, cx| v.copy(BytesFormat::CArray, false, cx)),
            )
            .on_action(
                cx.listener(|v, _: &CopyAsPython, _, cx| v.copy(BytesFormat::Python, false, cx)),
            )
            .on_action(cx.listener(|v, _: &ExtendUp, _, cx| v.extend_rows(-1, cx)))
            .on_action(cx.listener(|v, _: &ExtendDown, _, cx| v.extend_rows(1, cx)))
            .on_action(cx.listener(move |v, _: &ExtendPageUp, _, cx| v.extend_rows(-page, cx)))
            .on_action(cx.listener(move |v, _: &ExtendPageDown, _, cx| v.extend_rows(page, cx)))
            .on_action(cx.listener(|v, _: &ExtendHome, _, cx| v.extend_row_edge(false, cx)))
            .on_action(cx.listener(|v, _: &ExtendEnd, _, cx| v.extend_row_edge(true, cx)))
            .on_action(cx.listener(|v, _: &CursorUp, _, cx| v.move_rows(-1, cx)))
            .on_action(cx.listener(|v, _: &CursorDown, _, cx| v.move_rows(1, cx)))
            .on_action(cx.listener(move |v, _: &PageUp, _, cx| v.move_rows(-page, cx)))
            .on_action(cx.listener(move |v, _: &PageDown, _, cx| v.move_rows(page, cx)))
            .on_action(cx.listener(|v, _: &Follow, _, cx| {
                let ea = v.cursor;
                v.session.update(cx, |s, cx| s.navigate(ea, cx));
            }))
            .child(self.render_canvas(cx))
            // The group control, in the header row.
            .child(
                div()
                    .absolute()
                    .top_0()
                    .right(px(14.))
                    .h(px(line_height()))
                    .flex()
                    .items_center()
                    .gap(px(2.))
                    .text_size(px(11.))
                    .child(div().text_color(hsla(t.faint)).pr(px(4.)).child("Group"))
                    .children([1u64, 2, 4, 8].map(|g| {
                        let on = g == group;
                        div()
                            .id(("hex-group", g as usize))
                            .px(px(6.))
                            .rounded(px(3.))
                            .cursor_pointer()
                            .text_color(hsla(if on { t.text } else { t.muted }))
                            .when(on, |d| d.bg(hsla(t.accent_soft)))
                            .when(!on, |d| d.hover(|d| d.bg(hsla(t.hover))))
                            .on_click(cx.listener(move |v, _, window, cx| {
                                // A clickable child keeps the press from reaching the view's focus.
                                window.focus(&v.focus, cx);
                                v.set_group(g, cx)
                            }))
                            .child(g.to_string())
                    })),
            )
            .when(self.shows_process(cx), |d| {
                d.child(
                    div()
                        .absolute()
                        .top(px(30.))
                        .right(px(14.))
                        .child(crate::ui::badge(
                            "PROCESS MEMORY",
                            crate::ui::BadgeKind::XrefWrite,
                            &theme,
                        )),
                )
            })
            .child(bar)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn groups_of_eight_keep_the_classic_layout() {
        let c = Cols::new(8);
        assert_eq!(c.hex_x(0), 20.);
        assert_eq!(c.hex_x(7), 41.);
        assert_eq!(c.hex_x(8), 45., "one extra space after the eighth byte");
        assert_eq!(c.ascii_x(), 71.);
    }

    #[test]
    fn a_smaller_group_adds_a_space_after_every_group() {
        let c = Cols::new(2);
        assert_eq!(c.hex_x(1), 23.);
        assert_eq!(c.hex_x(2), 27., "a space between the pairs");
        assert_eq!(c.ascii_x(), 20. + 48. + 7. + 2.);
        let one = Cols::new(1);
        assert_eq!(one.hex_x(3), 20. + 9. + 3.);
    }

    #[test]
    fn an_unknown_group_falls_back_to_eight() {
        assert_eq!(Cols::new(3), Cols::new(8));
        assert_eq!(Cols::new(0), Cols::new(8));
    }

    #[test]
    fn every_byte_is_found_by_its_own_cells_in_both_columns() {
        for g in [1, 2, 4, 8] {
            let c = Cols::new(g);
            for col in 0..ROW {
                assert_eq!(c.col_at(c.hex_x(col) + 0.5), Some(col), "hex, group {g}");
                assert_eq!(c.col_at(c.hex_x(col) + 1.5), Some(col), "hex, group {g}");
                assert_eq!(
                    c.col_at(c.ascii_x() + col as f32 + 0.5),
                    Some(col),
                    "ascii, group {g}"
                );
            }
            assert_eq!(c.col_at(3.), None, "the address is no byte");
            assert_eq!(
                c.col_at(c.ascii_x() + ROW as f32 + 0.5),
                None,
                "past the ASCII"
            );
        }
    }

    #[test]
    fn typing_two_digits_makes_a_byte_and_one_leaves_a_nibble_pending() {
        let mut e = HexEdit::new(0x1000);
        assert!(e.digit('4'));
        assert_eq!((e.bytes.clone(), e.half), (vec![], Some(4)));
        assert!(e.digit('8'));
        assert_eq!((e.bytes.clone(), e.half), (vec![0x48], None));
        assert!(e.digit('c') && e.digit('F'));
        assert_eq!(e.bytes, vec![0x48, 0xCF]);
        assert_eq!(e.caret(), 0x1002, "the caret follows the bytes written");
    }

    #[test]
    fn other_keys_are_not_digits() {
        let mut e = HexEdit::new(0);
        for c in ['g', ' ', '-', 'z'] {
            assert!(!e.digit(c), "{c}");
        }
        assert!(e.bytes.is_empty() && e.half.is_none());
    }

    #[test]
    fn backspace_takes_back_the_nibble_then_the_last_byte() {
        let mut e = HexEdit::new(0x10);
        for c in "489".chars() {
            e.digit(c);
        }
        e.backspace();
        assert_eq!((e.bytes.clone(), e.half), (vec![0x48], None));
        e.backspace();
        assert!(e.bytes.is_empty());
        e.backspace();
        assert!(e.is_empty(), "nothing left to take back");
    }

    #[test]
    fn what_is_committed_is_the_whole_bytes_as_hex_pairs() {
        let mut e = HexEdit::new(0x2000);
        assert_eq!(e.hex(), None, "nothing typed, nothing to patch");
        for c in "90cc5".chars() {
            e.digit(c);
        }
        // The pending nibble is dropped, not guessed.
        assert_eq!(e.hex().as_deref(), Some("90 CC"));
    }

    #[test]
    fn the_header_lines_up_with_the_bytes() {
        let c = Cols::new(8);
        let h = c.header();
        assert!(h.starts_with("00 01 02 03 04 05 06 07  08 09"));
        for col in 0..ROW {
            let at = c.hex_x(col) as usize - Cols::HEX0 as usize;
            assert_eq!(&h[at..at + 2], format!("{col:02X}"), "column {col}");
        }
    }
}
