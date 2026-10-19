// Hex view. Rows are 16 bytes and map onto segments only, so sparse 64-bit
// layouts stay small. Bytes arrive in 4 KiB pages fetched on demand.
use crate::actions::*;
use crate::code::{self, line_height};
use crate::scrollbar::{self, Axes, Axis, Metrics, Source};
use crate::session::{Origin, Session, SessionEvent};
use crate::theme::{Theme, hsla};
use gpui::{
    App, Bounds, Context, Entity, FocusHandle, Focusable, MouseButton, MouseDownEvent, Pixels,
    ScrollWheelEvent, Subscription, Window, canvas, div, point, prelude::*, px,
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
    _subs: Vec<Subscription>,
}

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

    fn on_mouse_down(&mut self, ev: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.focus, cx);
        let Some(b) = self.bounds else { return };
        let y = f32::from(ev.position.y - b.top());
        let row = (self.top.floor()
            + ((y + (self.top.fract() as f32) * line_height()) / line_height()).floor() as f64)
            as u64;
        let x = f32::from(ev.position.x - b.left()) / self.char_w;
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
        if let (Some(base), Some(col)) = (self.ea_of_row(row), col) {
            self.cursor = base + col.min(15);
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
                        if cursor >= base && cursor < base + ROW {
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
