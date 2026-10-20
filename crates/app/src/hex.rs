// Hex view. Rows are 16 bytes and map onto segments only, so sparse 64-bit
// layouts stay small. Bytes arrive in 4 KiB pages fetched on demand.
use crate::actions::*;
use crate::code::{self, line_height};
use crate::scrollbar::{self, Axes, Axis, Metrics, Source};
use crate::selection::{Anchor, BytesFormat, bytes_text};
use crate::session::{Origin, Session, SessionEvent};
use crate::theme::{Theme, hsla};
use gpui::{
    App, Bounds, Context, Entity, FocusHandle, Focusable, MouseButton, MouseDownEvent,
    MouseMoveEvent, Pixels, Point, ScrollWheelEvent, Subscription, Window, canvas, div, point,
    prelude::*, px,
};
use ida_proto::{ByteChunk, Bytes, DbgMemRead, Ea, SegRow, Segments};
use std::collections::{HashMap, HashSet};

const PAGE: u64 = 4096;
const ROW: u64 = 16;
const MAX_PAGES: usize = 512;

pub struct HexView {
    session: Entity<Session>,
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
    _subs: Vec<Subscription>,
}

/// Selections longer than this are not copied (the clipboard is not a file).
const MAX_COPY: u64 = 1 << 20;

impl HexView {
    pub fn new(session: Entity<Session>, cx: &mut Context<Self>) -> Self {
        let sub = cx.subscribe(&session, |this, _, ev, cx| match ev {
            SessionEvent::Opened => this.load_segments(cx),
            SessionEvent::Navigated { ea, .. } => this.show(*ea, true, cx),
            SessionEvent::CursorMoved { ea, origin } if *origin != Origin::Hex => {
                this.show(*ea, false, cx)
            }
            SessionEvent::Changed => {
                this.pages.clear();
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
                cx.notify();
            }
            _ => {}
        });
        Self {
            session,
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
            _subs: vec![sub],
        }
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
            .update(cx, |s, cx| s.set_cursor(ea, Origin::Hex, cx));
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
                .update(cx, |s, cx| s.set_cursor(ea, Origin::Hex, cx));
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
        let y = f32::from(pos.y - b.top());
        let row = (self.top.floor()
            + ((y + (self.top.fract() as f32) * line_height()) / line_height()).floor() as f64)
            as u64;
        let x = f32::from(pos.x - b.left()) / self.char_w;
        // Layout in characters: 18 addr + 2 gap, then 16 x "XX " plus a gap after 8, then 2 + ascii.
        let hex0 = 20.;
        let col = if x >= hex0 && x < hex0 + 49. {
            let c = x - hex0;
            let c = if c > 24. { c - 1. } else { c };
            Some((c / 3.) as u64)
        } else if x >= hex0 + 51. && x < hex0 + 67. {
            Some((x - hex0 - 51.) as u64)
        } else {
            None
        };
        Some(self.ea_of_row(row)? + col?.min(15))
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
                    .update(cx, |s, cx| s.set_cursor(ea, Origin::Hex, cx));
            }
            cx.notify();
        }
    }

    /// Where a keyboard-opened context menu appears: under the caret's row.
    fn caret_position(&self) -> Point<Pixels> {
        let (Some(b), Some(row)) = (self.bounds, self.row_of(self.cursor)) else {
            return point(px(200.), px(200.));
        };
        let y = (row as f64 - self.top + 1.) as f32 * line_height();
        let col = (self.cursor % ROW) as f32;
        b.origin + point(px((20. + col * 3.) * self.char_w), px(y))
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
                    .update(cx, |s, cx| s.set_cursor(byte, Origin::Hex, cx));
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
        if let Some(byte) = self.byte_at(ev.position)
            && byte != self.cursor
        {
            self.sel.begin(from as usize);
            self.cursor = byte;
            self.session
                .update(cx, |s, cx| s.set_cursor(byte, Origin::Hex, cx));
            cx.notify();
        }
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
                let rows = (f32::from(bounds.size.height) / line_height()).ceil() as usize + 1;
                let cursor = v.cursor;
                let selected = v.sel.range(cursor as usize);
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
                window.with_content_mask(Some(gpui::ContentMask { bounds }), |window| {
                    for (k, base, bytes) in lines {
                        let y = bounds.top() + px((k as f32 - frac) * line_height());
                        // The selected bytes of this row: the hex cells and the ASCII.
                        if let Some(sel) = &selected {
                            let (lo, hi) = (*sel.start() as Ea, *sel.end() as Ea);
                            if hi >= base && lo < base + ROW {
                                let a = lo.max(base) - base;
                                let b = hi.min(base + ROW - 1) - base;
                                let hx =
                                    |c: u64| 20. + c as f32 * 3. + if c >= 8 { 1. } else { 0. };
                                for (x, w) in [
                                    (hx(a), hx(b) + 2. - hx(a)),
                                    (20. + 51. + a as f32, (b - a + 1) as f32),
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
                            code::paint_row_bg(bounds, y, hsla(theme.caret_row), window);
                            let col = (cursor - base) as f32;
                            let hx = 20. + col * 3. + if col >= 8. { 1. } else { 0. };
                            let ax = 20. + 51. + col;
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
                                let col = (ea - base) as f32;
                                let hx = 20. + col * 3. + if col >= 8. { 1. } else { 0. };
                                let ax = 20. + 51. + col;
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
                        let mut hex = String::with_capacity(52);
                        let mut ascii = String::with_capacity(16);
                        for (i, b) in bytes.iter().enumerate() {
                            if i == 8 {
                                hex.push(' ');
                            }
                            match b {
                                Some(Some(b)) => {
                                    hex.push_str(&format!("{b:02X} "));
                                    ascii.push(if b.is_ascii_graphic() || *b == b' ' {
                                        *b as char
                                    } else {
                                        '.'
                                    });
                                }
                                Some(None) => {
                                    hex.push_str("?? ");
                                    ascii.push(' ');
                                }
                                None => {
                                    hex.push_str("   ");
                                    ascii.push(' ');
                                }
                            }
                        }
                        let addr =
                            code::shape_plain(window, &format!("{base:016X}"), hsla(theme.faint));
                        code::paint_shaped(&addr, point(bounds.left() + cw * 1., y), window, cx);
                        let h = code::shape_plain(window, &hex, hsla(theme.text));
                        code::paint_shaped(&h, point(bounds.left() + cw * 20., y), window, cx);
                        let a = code::shape_plain(window, &ascii, hsla(theme.syntax.str));
                        code::paint_shaped(&a, point(bounds.left() + cw * 71., y), window, cx);
                    }
                });
                let viewport = (f32::from(bounds.size.height) / line_height()) as usize;
                entity.update(cx, |v, cx| {
                    v.bounds = Some(bounds);
                    v.char_w = f32::from(cw);
                    v.viewport_rows = viewport.max(1);
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
        div()
            .id("hex")
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
            .when(self.shows_process(cx), |d| {
                d.child(
                    div()
                        .absolute()
                        .top(px(6.))
                        .left(px(700.))
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
