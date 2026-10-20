// Linear disassembly. The database can be far larger than anything worth
// holding, so the view keeps a window of lines around the caret and pages
// more in from the host (listing_up / listing_down) as you scroll.
use crate::actions::*;
use crate::addr_map::SegMap;
use crate::code::{self, HitMap, line_height};
use crate::commands::{self, EditKind};
use crate::scrollbar::{self, Axes, Axis, Metrics, Source};
use crate::selection::{Anchor, CopyLine, lines_text};
use crate::session::{Caret, Origin, Session, SessionEvent, ViewKind};
use crate::theme::{Theme, hsla};
use gpui::{
    App, Context, Entity, FocusHandle, Focusable, MouseButton, MouseDownEvent, MouseMoveEvent,
    ScrollWheelEvent, SharedString, Subscription, Window, canvas, div, point, prelude::*, px,
};
use ida_proto::tags::{self, Line};
use ida_proto::{Ea, ListingChunk, ListingDown, ListingUp, Segments};

const PAGE_LINES: u32 = 200;
const MARGIN_ROWS: f32 = 100.;
const MAX_ROWS: usize = 6000;
const UNITS: f32 = 1_000_000.;

pub struct Row {
    pub ea: Ea,
    pub prefix: SharedString,
    pub prefix_color: u8,
    pub line: Line,
}

impl Row {
    fn from_chunk(chunk: ListingChunk) -> Vec<Row> {
        chunk
            .lines
            .into_iter()
            .map(|l| Row {
                ea: l.ea,
                prefix: l.prefix.into(),
                prefix_color: l.prefix_color,
                line: tags::parse(&l.tagged),
            })
            .collect()
    }
}

pub struct ListingView {
    session: Entity<Session>,
    /// This view\'s identity in `SessionEvent::CursorMoved`.
    origin: Origin,
    focus: FocusHandle,
    rows: Vec<Row>,
    top: f32,
    cursor: usize,
    highlight: Option<SharedString>,
    caret_col: usize,
    at_start: bool,
    at_end: bool,
    next_down: Option<Ea>,
    loading_up: bool,
    loading_down: bool,
    generation: u64,
    viewport_rows: usize,
    scroll_x: f32,
    hit: HitMap,
    loading: bool,
    /// The segments, for a scrollbar that spans the whole address space.
    segs: SegMap,
    /// Where a scrollbar drag points while the jump it asked for loads.
    drag_units: Option<f32>,
    pending_jump: Option<Ea>,
    /// Rows of cursor movement (page keys) that outran the buffer; applied as
    /// chunks arrive.
    owed_rows: isize,
    /// Painted sizes: the text area's width and the widest visible line.
    text_w: f32,
    content_w: f32,
    /// The other end of the selection (the caret row is this end).
    sel: Anchor,
    /// The row a mouse drag started on while the button is held.
    drag_from: Option<usize>,
    /// The caret column to keep when a refresh after an edit reloads the rows.
    keep_col: Option<usize>,
    /// Drag-select past an edge: which way, and whether the ticker runs.
    auto_dir: isize,
    auto_running: bool,
    _subs: Vec<Subscription>,
}

impl ListingView {
    pub fn new(session: Entity<Session>, cx: &mut Context<Self>) -> Self {
        let sub = cx.subscribe(&session, |this, _, ev, cx| match ev {
            SessionEvent::Navigated { ea, .. } => this.show(*ea, true, cx),
            SessionEvent::CursorMoved { ea, origin } if *origin != this.origin => {
                this.show(*ea, false, cx)
            }
            SessionEvent::Opened => this.load_segments(cx),
            SessionEvent::Rebased { .. } => this.load_segments(cx),
            SessionEvent::DebugChanged | SessionEvent::MarksChanged => cx.notify(),
            SessionEvent::Changed => this.refresh(cx),
            SessionEvent::Closed => {
                this.rows.clear();
                cx.notify();
            }
            _ => {}
        });
        Self {
            session,
            origin: Origin::new(ViewKind::Listing),
            focus: cx.focus_handle(),
            rows: Vec::new(),
            top: 0.,
            cursor: 0,
            highlight: None,
            caret_col: 0,
            at_start: false,
            at_end: false,
            next_down: None,
            loading_up: false,
            loading_down: false,
            generation: 0,
            viewport_rows: 50,
            scroll_x: 0.,
            hit: HitMap::default(),
            loading: false,
            segs: SegMap::default(),
            drag_units: None,
            pending_jump: None,
            owed_rows: 0,
            text_w: 0.,
            content_w: 0.,
            sel: Anchor::default(),
            drag_from: None,
            keep_col: None,
            auto_dir: 0,
            auto_running: false,
            _subs: vec![sub],
        }
    }

    fn load_segments(&mut self, cx: &mut Context<Self>) {
        let fut = self.session.read(cx).call(Segments {});
        cx.spawn(async move |this, cx| {
            if let Ok(rows) = fut.await {
                this.update(cx, |v, cx| {
                    v.segs = SegMap::new(&rows);
                    cx.notify();
                })
                .ok();
            }
        })
        .detach();
    }

    /// The vertical thumb: position across the whole address space (segments
    /// only), in millionths so the numbers stay exact in an `f32`.
    fn scroll_metrics(&self, axis: Axis) -> Option<Metrics> {
        match axis {
            Axis::Vertical => {
                let total = self.segs.total();
                if total == 0 || self.rows.is_empty() {
                    return None;
                }
                let first = self.top.floor() as usize;
                let last = (first + self.viewport_rows).min(self.rows.len() - 1);
                let pos = |i: usize| {
                    self.segs.pos(self.rows[i.min(self.rows.len() - 1)].ea) as f64 / total as f64
                        * UNITS as f64
                };
                let (a, b) = (pos(first), pos(last));
                let offset = self.drag_units.unwrap_or(a as f32);
                Some(Metrics {
                    viewport: (b - a).max(0.) as f32,
                    content: UNITS,
                    offset,
                })
            }
            Axis::Horizontal => (self.content_w > self.text_w + 1.).then_some(Metrics {
                viewport: self.text_w,
                content: self.content_w,
                offset: self.scroll_x,
            }),
        }
    }

    /// The thumb was dragged (or the track clicked): inside the loaded rows
    /// this just scrolls, outside it asks the host for that part of the database.
    fn scroll_to(&mut self, axis: Axis, units: f32, cx: &mut Context<Self>) {
        if axis == Axis::Horizontal {
            self.scroll_x = units.clamp(0., (self.content_w - self.text_w).max(0.));
            cx.notify();
            return;
        }
        let total = self.segs.total();
        let Some(ea) = self
            .segs
            .ea((units as f64 / UNITS as f64 * total as f64) as u64)
        else {
            return;
        };
        if let (Some(first), Some(last)) = (self.rows.first(), self.rows.last())
            && ea >= first.ea
            && ea <= last.ea
        {
            self.drag_units = None;
            self.top = self.rows.partition_point(|r| r.ea < ea) as f32;
            self.clamp_top();
            self.ensure_buffer(cx);
        } else {
            self.drag_units = Some(units);
            if self.loading {
                self.pending_jump = Some(ea);
            } else {
                self.jump(ea, None, cx);
            }
        }
        cx.notify();
    }

    pub fn cursor_ea(&self) -> Option<Ea> {
        self.rows.get(self.cursor).map(|r| r.ea)
    }

    fn token(&self) -> Option<String> {
        let row = self.rows.get(self.cursor)?;
        // Without a caret inside a name, the first name on the line stands for the item.
        let r = code::token_at(&row.line, self.caret_col).or_else(|| {
            let run = row
                .line
                .runs
                .iter()
                .find(|r| tags::color::is_name(r.color))?;
            code::token_at(&row.line, run.range.start)
        })?;
        Some(row.line.text[r].to_owned())
    }

    /// Row of item `ea` where the caret should sit: its first line that is
    /// not blank, a comment or a bare label.
    fn item_row(&self, ea: Ea) -> Option<usize> {
        let first = self.rows.iter().position(|r| r.ea == ea)?;
        let mut best = first;
        for (i, r) in self.rows.iter().enumerate().skip(first) {
            if r.ea != ea {
                break;
            }
            let t = r.line.text.trim();
            let label = t.ends_with(':') && !t.contains(' ');
            if !t.is_empty() && !t.starts_with(';') && !label {
                best = i;
                break;
            }
            best = i;
        }
        Some(best)
    }

    fn row_for_address(&self, ea: Ea) -> Option<usize> {
        if self.rows.is_empty() || ea < self.rows[0].ea {
            return None;
        }
        // The item containing ea: last row whose item starts at or before ea.
        // Past the buffered tail we cannot tell, so refetch unless at the end.
        let idx = self.rows.partition_point(|r| r.ea <= ea);
        let head = self.rows[idx.checked_sub(1)?].ea;
        if idx == self.rows.len() && !self.at_end && ea != head {
            return None;
        }
        self.item_row(head)
    }

    /// Brings `ea` into view; `center` re-centres even when visible.
    fn show(&mut self, ea: Ea, center: bool, cx: &mut Context<Self>) {
        if let Some(row) = self.row_for_address(ea) {
            self.cursor = row;
            self.caret_col = 0;
            self.ensure_visible(center);
            self.ensure_buffer(cx);
            self.publish_caret(cx);
            cx.notify();
        } else {
            self.jump(ea, None, cx);
        }
    }

    fn ensure_visible(&mut self, center: bool) {
        let vp = self.viewport_rows.max(1) as f32;
        let c = self.cursor as f32;
        if center && (c < self.top || c >= self.top + vp - 1.) {
            self.top = (c - vp / 3.).max(0.);
        } else if c < self.top {
            self.top = c;
        } else if c >= self.top + vp - 1. {
            self.top = c - vp + 2.;
        }
        self.clamp_top();
    }

    fn clamp_top(&mut self) {
        let max = (self.rows.len() as f32 - 1.).max(0.);
        self.top = self.top.clamp(0., max);
    }

    fn jump(&mut self, ea: Ea, keep_screen_row: Option<f32>, cx: &mut Context<Self>) {
        self.owed_rows = 0;
        self.sel.clear();
        self.generation += 1;
        let generation = self.generation;
        let s = self.session.read(cx);
        let up = s.call(ListingUp { ea, count: 120 });
        let down = s.call(ListingDown {
            ea,
            count: PAGE_LINES,
        });
        self.loading = true;
        self.loading_up = true;
        self.loading_down = true;
        cx.spawn(async move |this, cx| {
            let (up, down) = (up.await, down.await);
            this.update(cx, |v, cx| {
                if v.generation != generation {
                    return;
                }
                v.loading = false;
                v.loading_up = false;
                v.loading_down = false;
                // A scrollbar drag that moved on while this loaded.
                match v.pending_jump.take() {
                    Some(next) if next != ea => {
                        v.jump(next, None, cx);
                        return;
                    }
                    _ => v.drag_units = None,
                }
                match (up, down) {
                    (Ok(up), Ok(down)) => {
                        v.at_start = up.at_edge;
                        v.at_end = down.at_edge;
                        v.next_down = down.next;
                        let mut rows = Row::from_chunk(up);
                        rows.extend(Row::from_chunk(down));
                        v.rows = rows;
                        v.cursor = v.row_for_address(ea).unwrap_or(0);
                        match keep_screen_row {
                            Some(offset) => {
                                v.top = v.cursor as f32 - offset;
                                v.clamp_top();
                            }
                            None => {
                                v.top = 0.;
                                v.ensure_visible(true);
                            }
                        }
                        v.ensure_buffer(cx);
                        v.caret_col = v.keep_col.take().unwrap_or(0);
                        v.publish_caret(cx);
                    }
                    (Err(e), _) | (_, Err(e)) => {
                        v.session.update(cx, |s, cx| s.error(e.to_string(), cx));
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn refresh(&mut self, cx: &mut Context<Self>) {
        if let Some(ea) = self.cursor_ea() {
            let offset = self.cursor as f32 - self.top;
            // An edit keeps the caret where it was (several operand commands in a row).
            self.keep_col = Some(self.caret_col);
            self.jump(ea, Some(offset), cx);
        }
    }

    /// Pages lines in near either edge and trims far-away ones.
    fn ensure_buffer(&mut self, cx: &mut Context<Self>) {
        if self.rows.is_empty() {
            return;
        }
        let generation = self.generation;
        if !self.at_start && !self.loading_up && self.top < MARGIN_ROWS {
            self.loading_up = true;
            let anchor = self.rows[0].ea;
            let fut = self.session.read(cx).call(ListingUp {
                ea: anchor,
                count: PAGE_LINES,
            });
            cx.spawn(async move |this, cx| {
                let chunk = fut.await;
                this.update(cx, |v, cx| {
                    if v.generation != generation {
                        return;
                    }
                    v.loading_up = false;
                    // The buffer was trimmed or replaced while the request was
                    // in flight; splicing now would leave a gap or duplicates.
                    if v.rows.first().map(|r| r.ea) != Some(anchor) {
                        v.ensure_buffer(cx);
                        return;
                    }
                    if let Ok(chunk) = chunk {
                        v.at_start = chunk.at_edge || chunk.lines.is_empty();
                        let new = Row::from_chunk(chunk);
                        debug_assert!(
                            new.last().is_none_or(|r| r.ea < anchor),
                            "listing_up overlaps the buffer"
                        );
                        let n = new.len();
                        v.rows.splice(0..0, new);
                        v.top += n as f32;
                        v.cursor += n;
                        v.sel.shift(n as isize);
                        v.trim();
                        v.apply_owed(cx);
                        cx.notify();
                    }
                })
                .ok();
            })
            .detach();
        }
        let bottom = self.top + self.viewport_rows as f32;
        if !self.at_end
            && !self.loading_down
            && bottom + MARGIN_ROWS > self.rows.len() as f32
            && let Some(next) = self.next_down
        {
            self.loading_down = true;
            let fut = self.session.read(cx).call(ListingDown {
                ea: next,
                count: PAGE_LINES,
            });
            cx.spawn(async move |this, cx| {
                let chunk = fut.await;
                this.update(cx, |v, cx| {
                    if v.generation != generation {
                        return;
                    }
                    v.loading_down = false;
                    if v.next_down != Some(next) {
                        v.ensure_buffer(cx);
                        return;
                    }
                    if let Ok(chunk) = chunk {
                        v.at_end = chunk.at_edge;
                        v.next_down = chunk.next;
                        let new = Row::from_chunk(chunk);
                        debug_assert!(
                            new.first()
                                .zip(v.rows.last())
                                .is_none_or(|(n, l)| n.ea > l.ea),
                            "listing_down overlaps the buffer"
                        );
                        v.rows.extend(new);
                        v.trim();
                        v.apply_owed(cx);
                        cx.notify();
                    }
                })
                .ok();
            })
            .detach();
        }
    }

    fn trim(&mut self) {
        if self.rows.len() <= MAX_ROWS {
            return;
        }
        let excess = self.rows.len() - MAX_ROWS / 2;
        if self.top as usize > self.rows.len() / 2 {
            // Drop from the front, at an item boundary.
            let mut cut = excess.min(self.top as usize - 10);
            while cut > 0 && cut < self.rows.len() && self.rows[cut].ea == self.rows[cut - 1].ea {
                cut -= 1;
            }
            if cut == 0 {
                return;
            }
            self.rows.drain(..cut);
            self.top -= cut as f32;
            self.cursor = self.cursor.saturating_sub(cut);
            self.sel.shift(-(cut as isize));
            self.at_start = false;
        } else {
            let mut cut = (self.top as usize + self.viewport_rows + 200).max(MAX_ROWS / 2);
            while cut < self.rows.len() && cut > 0 && self.rows[cut].ea == self.rows[cut - 1].ea {
                cut += 1;
            }
            if cut >= self.rows.len() {
                return;
            }
            self.next_down = Some(self.rows[cut].ea);
            self.rows.truncate(cut);
            self.at_end = false;
            self.cursor = self.cursor.min(self.rows.len() - 1);
        }
    }

    fn move_cursor(&mut self, delta: isize, cx: &mut Context<Self>) {
        self.sel.clear();
        self.step(delta, cx);
    }

    /// Shift+arrow, Shift+PageUp / PageDown / Home / End: moves the caret end
    /// of the selection.
    fn extend(&mut self, delta: isize, cx: &mut Context<Self>) {
        if self.rows.is_empty() {
            return;
        }
        self.sel.begin(self.cursor);
        self.step(delta, cx);
    }

    fn step(&mut self, delta: isize, cx: &mut Context<Self>) {
        if self.rows.is_empty() {
            return;
        }
        let (cursor, owed) = step_cursor(
            self.cursor,
            delta,
            self.rows.len(),
            !self.at_start,
            !self.at_end,
        );
        self.cursor = cursor;
        self.owed_rows += owed;
        self.caret_col = 0;
        self.ensure_visible(false);
        self.ensure_buffer(cx);
        self.publish_cursor(cx);
        cx.notify();
    }

    /// A chunk arrived: carry on with the movement that was waiting for it.
    fn apply_owed(&mut self, cx: &mut Context<Self>) {
        let owed = std::mem::take(&mut self.owed_rows);
        if owed != 0 {
            self.step(owed, cx);
        }
    }

    fn publish_caret(&mut self, cx: &mut Context<Self>) {
        let caret = Caret {
            origin: Some(self.origin),
            line: None,
            col: self.caret_col as u32 + 1,
            block: None,
            token: self.token(),
            operand: self
                .rows
                .get(self.cursor)
                .and_then(|r| crate::operands::operand_at(&r.line.text, self.caret_col)),
        };
        self.session.update(cx, |s, cx| s.set_caret(caret, cx));
    }

    fn publish_cursor(&mut self, cx: &mut Context<Self>) {
        if let Some(ea) = self.cursor_ea() {
            self.session
                .update(cx, |s, cx| s.set_cursor(ea, self.origin, cx));
            self.publish_caret(cx);
        }
    }

    fn on_scroll(&mut self, ev: &ScrollWheelEvent, _: &mut Window, cx: &mut Context<Self>) {
        let delta = ev.delta.pixel_delta(px(line_height()));
        if ev.modifiers.shift || f32::from(delta.x) != 0. {
            let dx = if ev.modifiers.shift { delta.y } else { delta.x };
            self.scroll_x = (self.scroll_x - f32::from(dx)).max(0.);
            cx.notify();
            return;
        }
        self.top -= f32::from(delta.y) / line_height();
        self.clamp_top();
        self.ensure_buffer(cx);
        cx.notify();
    }

    fn on_mouse_down(&mut self, ev: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.focus, cx);
        // A click in the marker gutter sets or clears a breakpoint.
        if let Some(row) = self.hit.gutter_row(ev.position) {
            if let Some(ea) = self.rows.get(row).map(|r| r.ea) {
                self.session.update(cx, |s, cx| s.dbg_toggle_bpt(ea, cx));
            }
            return;
        }
        let Some((row, col)) = self.hit.hit(ev.position) else {
            return;
        };
        if ev.modifiers.shift {
            self.sel.begin(self.cursor);
        } else {
            self.sel.clear();
            self.drag_from = Some(row);
        }
        self.cursor = row;
        self.caret_col = col;
        self.highlight = self.token().map(Into::into);
        self.publish_cursor(cx);
        if ev.click_count >= 2 {
            self.follow(cx);
        }
        cx.notify();
    }

    /// Where a keyboard-opened context menu appears: just under the caret.
    fn caret_position(&self) -> gpui::Point<gpui::Pixels> {
        match self.hit.rows.iter().find(|(r, _, _)| *r == self.cursor) {
            Some((_, y, shaped)) => point(
                self.hit.text_x + shaped.x_for_index(self.caret_col),
                *y + px(line_height()),
            ),
            None => point(px(200.), px(200.)),
        }
    }

    fn open_context_menu(
        &mut self,
        pos: gpui::Point<gpui::Pixels>,
        keyboard: bool,
        cx: &mut Context<Self>,
    ) {
        let Some(ea) = self.cursor_ea() else { return };
        let title = self.token().unwrap_or_else(|| format!("{ea:X}"));
        let entries = crate::context_menu::listing(&self.session.read(cx).ops(), cx);
        crate::context_menu::show(&self.session, pos, title, entries, keyboard, cx);
    }

    /// Right click: the caret goes to the clicked token first (unless it is
    /// inside the selection), then the menu opens.
    fn on_right_down(&mut self, ev: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.focus, cx);
        if let Some((row, col)) = self.hit.hit(ev.position) {
            let inside = self
                .sel
                .range(self.cursor)
                .is_some_and(|r| r.contains(&row));
            if !inside {
                self.sel.clear();
                self.cursor = row;
                self.caret_col = col;
                self.highlight = self.token().map(Into::into);
                self.publish_cursor(cx);
            }
        }
        self.open_context_menu(ev.position, false, cx);
        cx.notify();
    }

    /// Dragging with the left button held selects from the row it started on.
    fn on_mouse_move(&mut self, ev: &MouseMoveEvent, _: &mut Window, cx: &mut Context<Self>) {
        let Some(from) = self.drag_from else { return };
        if ev.pressed_button != Some(MouseButton::Left) {
            self.drag_from = None;
            return;
        }
        if let Some(b) = self.hit.bounds {
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
        let Some((row, col)) = self.hit.hit(ev.position) else {
            return;
        };
        if row != self.cursor {
            self.sel.begin(from);
            self.cursor = row;
            self.caret_col = col;
            self.publish_cursor(cx);
            cx.notify();
        }
    }

    fn auto_tick(&mut self, cx: &mut Context<Self>) {
        if let Some(from) = self.drag_from {
            self.sel.begin(from);
            self.step(self.auto_dir, cx);
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

    /// The bytes of the selected lines, or of the item at the caret: from the
    /// first line's address to the next address after the last line.
    pub fn selection_span(&self) -> Option<(Ea, u64)> {
        let range = self
            .sel
            .range(self.cursor)
            .unwrap_or(self.cursor..=self.cursor);
        let first = self.rows.get(*range.start())?.ea;
        let last = self.rows.get(*range.end())?.ea;
        let end = self.rows[*range.end()..]
            .iter()
            .map(|r| r.ea)
            .find(|&ea| ea > last)
            .unwrap_or(last + 1);
        Some((first, end - first))
    }

    /// Esc: with a selection it only clears the selection; otherwise it is
    /// "back" as everywhere (the action keeps travelling up).
    fn on_back(&mut self, cx: &mut Context<Self>) {
        if self.sel.active(self.cursor) {
            self.sel.clear();
            cx.notify();
        } else {
            cx.propagate();
        }
    }

    /// Ctrl+C: the selected lines, or the current one.
    fn copy(&mut self, with_addresses: bool, cx: &mut Context<Self>) {
        let range = self
            .sel
            .range(self.cursor)
            .unwrap_or(self.cursor..=self.cursor);
        let Some(rows) = self
            .rows
            .get(*range.start()..=(*range.end()).min(self.rows.len() - 1))
        else {
            return;
        };
        let lines: Vec<CopyLine> = rows
            .iter()
            .map(|r| CopyLine {
                addr: Some(&r.prefix),
                text: &r.line.text,
            })
            .collect();
        zui_editor::clipboard::write(lines_text(&lines, with_addresses), cx);
    }

    fn follow(&mut self, cx: &mut Context<Self>) {
        let Some(row) = self.rows.get(self.cursor) else {
            return;
        };
        let ea = row.ea;
        if row.line.text.contains("COLLAPSED") {
            commands::edit(&self.session, EditKind::Collapse, ea, cx);
            return;
        }
        let token = self.rows.get(self.cursor).and_then(|r| {
            code::follow_token_at(&r.line, self.caret_col).map(|t| r.line.text[t].to_owned())
        });
        if let Some(token) = token {
            commands::follow_token(&self.session, token, ea, cx);
        }
    }

    fn on_action_edit(&mut self, kind: EditKind, cx: &mut Context<Self>) {
        if let Some(ea) = self.cursor_ea() {
            commands::edit(&self.session, kind, ea, cx);
        }
    }

    fn render_canvas(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let entity = cx.entity();
        canvas(
            |_, _, _| (),
            move |bounds, (), window, cx| {
                let theme = Theme::get(cx);
                let view = entity.read(cx);
                let first = view.top.floor() as usize;
                let frac = view.top - view.top.floor();
                let visible = (f32::from(bounds.size.height) / line_height()).ceil() as usize + 1;
                let end = (first + visible).min(view.rows.len());
                let cursor = view.cursor;
                let selected = view.sel.range(cursor);
                let highlight = view.highlight.clone();
                // Several lines can share an address (a segment header sits on the
                // first instruction); the markers belong to the last of them.
                let last_of_ea: Vec<bool> = (first..end)
                    .map(|i| view.rows.get(i + 1).is_none_or(|n| n.ea != view.rows[i].ea))
                    .collect();
                let rows: Vec<(usize, SharedString, u8, Line, Ea)> = (first..end)
                    .map(|i| {
                        let r = &view.rows[i];
                        (i, r.prefix.clone(), r.prefix_color, r.line.clone(), r.ea)
                    })
                    .collect();
                let cursor_ea = view.rows.get(cursor).map(|r| r.ea);
                let scroll_x = px(view.scroll_x);
                let focused = view.focus.is_focused(window);
                let cw = code::char_width(window);
                let gutter = cw * 26. + px(code::MARK_W);
                let marks = code::DebugMarks::read(view.session.read(cx));
                let bookmarks: Vec<Ea> = view.session.read(cx).marks.iter().map(|m| m.ea).collect();
                let text_left = bounds.left() + gutter + px(14.);
                let text_x = text_left - scroll_x;
                window.paint_quad(gpui::fill(
                    gpui::Bounds::new(
                        bounds.origin,
                        gpui::size(gutter + px(6.), bounds.size.height),
                    ),
                    hsla(theme.surface2),
                ));
                let widest = rows
                    .iter()
                    .map(|r| r.3.text.chars().count())
                    .max()
                    .unwrap_or(0) as f32;
                let mut hit = HitMap {
                    bounds: Some(bounds),
                    text_x,
                    rows: Vec::with_capacity(rows.len()),
                };
                window.with_content_mask(Some(gpui::ContentMask { bounds }), |window| {
                    for (k, (i, prefix, _pcolor, line, ea)) in rows.into_iter().enumerate() {
                        let y = bounds.top() + px((k as f32 - frac) * line_height());
                        let same_item = cursor_ea == Some(ea);
                        let own = last_of_ea[i - first];
                        if let Some(bg) = marks.row_bg(ea, &theme).filter(|_| own) {
                            code::paint_row_bg(bounds, y, bg, window);
                        }
                        let marked = own && marks.row_bg(ea, &theme).is_some();
                        let in_selection = selected.as_ref().is_some_and(|r| r.contains(&i));
                        if in_selection && !marked {
                            code::paint_row_bg(bounds, y, hsla(theme.selection), window);
                        }
                        if i == cursor {
                            if !marked && !in_selection {
                                code::paint_row_bg(bounds, y, hsla(theme.caret_row), window);
                            }
                            if focused {
                                window.paint_quad(gpui::fill(
                                    gpui::Bounds::new(
                                        point(bounds.left(), y),
                                        gpui::size(px(2.), px(line_height())),
                                    ),
                                    hsla(theme.accent),
                                ));
                            }
                        } else if same_item && !marked && !in_selection {
                            code::paint_row_bg(bounds, y, hsla(theme.caret_row), window);
                        }
                        let shaped = code::shape(window, &line, &theme, highlight.as_deref());
                        let text_bounds = gpui::Bounds::from_corners(
                            point(text_left, bounds.top()),
                            bounds.bottom_right(),
                        );
                        window.with_content_mask(
                            Some(gpui::ContentMask {
                                bounds: text_bounds,
                            }),
                            |window| {
                                code::paint_shaped(&shaped, point(text_x, y), window, cx);
                            },
                        );
                        let p = code::shape_plain(
                            window,
                            &prefix,
                            if i == cursor {
                                hsla(theme.text)
                            } else {
                                hsla(theme.faint)
                            },
                        );
                        code::paint_shaped(
                            &p,
                            point(bounds.left() + px(code::MARK_W + 6.), y),
                            window,
                            cx,
                        );
                        if own {
                            marks.paint(ea, bounds.left(), y, &theme, window);
                            if bookmarks.contains(&ea) {
                                code::paint_mark(
                                    bounds.left() + px(code::MARK_W - 2.),
                                    y,
                                    px(line_height()),
                                    &theme,
                                    window,
                                );
                            }
                        }
                        hit.rows.push((i, y, shaped));
                    }
                });
                let viewport = (f32::from(bounds.size.height) / line_height()) as usize;
                entity.update(cx, |v, _| {
                    v.hit = hit;
                    v.viewport_rows = viewport.max(1);
                    v.text_w = (f32::from(bounds.size.width) - f32::from(gutter) - 14.).max(0.);
                    v.content_w = widest * f32::from(cw);
                });
            },
        )
        .size_full()
    }
}

impl Focusable for ListingView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for ListingView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::get(cx);
        let (m, s) = (cx.entity().downgrade(), cx.entity().downgrade());
        let source = Source::custom(
            move |axis, cx| m.upgrade().and_then(|v| v.read(cx).scroll_metrics(axis)),
            move |axis, units, cx| {
                if let Some(v) = s.upgrade() {
                    v.update(cx, |v, cx| v.scroll_to(axis, units, cx));
                }
            },
        );
        let bar = scrollbar::scrollbar("listing-scroll", source, Axes::BOTH, &theme, window, cx);
        let page = self.viewport_rows.max(2) as isize - 1;
        div()
            .id("listing")
            .key_context("CodeView")
            .track_focus(&self.focus)
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
            .on_action(cx.listener(|v, _: &Copy, _, cx| v.copy(false, cx)))
            .on_action(cx.listener(|v, _: &CopyWithAddresses, _, cx| v.copy(true, cx)))
            .on_action(cx.listener(|v, _: &ExtendUp, _, cx| v.extend(-1, cx)))
            .on_action(cx.listener(|v, _: &ExtendDown, _, cx| v.extend(1, cx)))
            .on_action(cx.listener(move |v, _: &ExtendPageUp, _, cx| v.extend(-page, cx)))
            .on_action(cx.listener(move |v, _: &ExtendPageDown, _, cx| v.extend(page, cx)))
            .on_action(cx.listener(|v, _: &ExtendHome, _, cx| v.extend(-(v.cursor as isize), cx)))
            .on_action(cx.listener(|v, _: &ExtendEnd, _, cx| {
                v.extend(v.rows.len() as isize - 1 - v.cursor as isize, cx)
            }))
            .on_action(cx.listener(|v, _: &CursorUp, _, cx| v.move_cursor(-1, cx)))
            .on_action(cx.listener(|v, _: &CursorDown, _, cx| v.move_cursor(1, cx)))
            .on_action(cx.listener(move |v, _: &PageUp, _, cx| v.move_cursor(-page, cx)))
            .on_action(cx.listener(move |v, _: &PageDown, _, cx| v.move_cursor(page, cx)))
            .on_action(cx.listener(|v, _: &Top, _, cx| {
                let min = v
                    .session
                    .read(cx)
                    .info
                    .as_ref()
                    .map(|i| i.min_ea)
                    .unwrap_or(0);
                v.jump(min, None, cx);
            }))
            .on_action(cx.listener(|v, _: &Bottom, _, cx| {
                let max = v
                    .session
                    .read(cx)
                    .info
                    .as_ref()
                    .map(|i| i.max_ea)
                    .unwrap_or(0);
                v.jump(max.saturating_sub(1), None, cx);
            }))
            .on_action(cx.listener(|v, _: &Follow, _, cx| v.follow(cx)))
            .on_action(cx.listener(|v, _: &Rename, _, cx| {
                let Some(ea) = v.cursor_ea() else { return };
                let token = v.token();
                commands::rename_token(&v.session, token, ea, cx);
            }))
            .on_action(cx.listener(|v, _: &Comment, _, cx| {
                if let Some(ea) = v.cursor_ea() {
                    commands::comment_ea(&v.session, ea, false, cx);
                }
            }))
            .on_action(cx.listener(|v, _: &RepeatableComment, _, cx| {
                if let Some(ea) = v.cursor_ea() {
                    commands::comment_ea(&v.session, ea, true, cx);
                }
            }))
            .on_action(cx.listener(|v, _: &SetType, _, cx| {
                if let Some(ea) = v.cursor_ea() {
                    commands::set_type_ea(&v.session, ea, cx);
                }
            }))
            .on_action(cx.listener(|v, _: &XrefsTo, _, cx| {
                let Some(ea) = v.cursor_ea() else { return };
                let session = v.session.clone();
                commands::token_target(&v.session, v.token(), ea, cx, move |t, cx| {
                    commands::show_xrefs(&session, t, true, cx)
                });
            }))
            .on_action(cx.listener(|v, _: &XrefsFrom, _, cx| {
                if let Some(ea) = v.cursor_ea() {
                    commands::show_xrefs(&v.session, ea, false, cx);
                }
            }))
            .on_action(cx.listener(|v, _: &MakeCode, _, cx| v.on_action_edit(EditKind::Code, cx)))
            .on_action(cx.listener(|v, _: &MakeData, _, cx| v.on_action_edit(EditKind::Data, cx)))
            .on_action(
                cx.listener(|v, _: &MakeString, _, cx| v.on_action_edit(EditKind::String, cx)),
            )
            .on_action(
                cx.listener(|v, _: &Undefine, _, cx| v.on_action_edit(EditKind::Undefine, cx)),
            )
            .on_action(
                cx.listener(|v, _: &MakeFunction, _, cx| v.on_action_edit(EditKind::Function, cx)),
            )
            .on_action(cx.listener(|v, _: &DeleteFunction, _, cx| {
                v.on_action_edit(EditKind::DeleteFunction, cx)
            }))
            .on_action(
                cx.listener(|v, _: &ToggleCollapse, _, cx| {
                    v.on_action_edit(EditKind::Collapse, cx)
                }),
            )
            .child(self.render_canvas(cx))
            .child(bar)
            .when(self.loading && self.rows.is_empty(), |d| {
                d.child(
                    div()
                        .absolute()
                        .top(px(12.))
                        .left(px(16.))
                        .text_color(hsla(theme.muted))
                        .child("Loading…"),
                )
            })
    }
}

/// Where a cursor move of `delta` rows lands in a buffer of `len` rows, and
/// how many rows of it are still owed because the buffer ends before the
/// database does (`more_above` / `more_below`): that remainder waits for the
/// next chunk instead of being dropped.
fn step_cursor(
    cursor: usize,
    delta: isize,
    len: usize,
    more_above: bool,
    more_below: bool,
) -> (usize, isize) {
    if len == 0 {
        return (0, 0);
    }
    let max = len as isize - 1;
    let target = cursor as isize + delta;
    let owed = if target < 0 && more_above {
        target
    } else if target > max && more_below {
        target - max
    } else {
        0
    };
    (target.clamp(0, max) as usize, owed)
}

#[cfg(test)]
mod tests {
    use super::step_cursor;

    #[test]
    fn inside_the_buffer_nothing_is_owed() {
        assert_eq!(step_cursor(100, -40, 400, true, true), (60, 0));
        assert_eq!(step_cursor(100, 40, 400, true, true), (140, 0));
    }

    #[test]
    fn a_page_up_past_the_top_owes_the_rest_while_more_exists_above() {
        assert_eq!(step_cursor(10, -40, 400, true, false), (0, -30));
    }

    #[test]
    fn a_page_up_at_the_real_start_owes_nothing() {
        assert_eq!(step_cursor(10, -40, 400, false, false), (0, 0));
    }

    #[test]
    fn a_page_down_past_the_end_owes_the_rest_while_more_exists_below() {
        assert_eq!(step_cursor(390, 40, 400, false, true), (399, 31));
        assert_eq!(step_cursor(390, 40, 400, false, false), (399, 0));
    }

    #[test]
    fn an_empty_buffer_owes_nothing() {
        assert_eq!(step_cursor(0, -40, 0, true, true), (0, 0));
    }
}
