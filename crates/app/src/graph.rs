// Control-flow graph of the function at the cursor: blocks of listing lines,
// laid out by `graph_layout`, painted on a pannable / zoomable canvas.
use crate::actions::*;
use crate::code::{self, font_size, line_height};
use crate::commands::{self, EditKind};
use crate::graph_layout::{self, Layout};
use crate::graph_paint::{self, MINIMAP_GAP};
use crate::scrollbar::{self, Axes};
use crate::session::{Caret, Origin, Session, SessionEvent};
use crate::theme::{Theme, hsla};
use crate::ui;
use gpui::{
    App, Bounds, Context, Entity, FocusHandle, Focusable, MouseButton, MouseDownEvent,
    MouseMoveEvent, MouseUpEvent, Pixels, Point, ScrollWheelEvent, SharedString, Subscription,
    TextRun, Window, canvas, div, point, prelude::*, px, size,
};
use ida_proto::tags::{self, Line};
use ida_proto::{Ea, EdgeKind, Graph};

const PAD: f32 = 8.;
/// Block header strip: the address and the block number.
const HEAD: f32 = 24.;
/// Height of the strip with the zoom controls and the edge legend.
const HEADER_H: f32 = 32.;

/// Graph blocks use a tighter 12 / 18 line.
fn lh() -> f32 {
    (line_height() * 0.9).round()
}

/// What the inspector shows for the block under the caret.
#[derive(Clone, Debug)]
pub struct BlockInfo {
    pub index: usize,
    pub start: Ea,
    pub end: Ea,
    pub insns: usize,
    /// The last instruction, as text.
    pub terminator: String,
    pub succs: Vec<(EdgeKind, usize, Ea)>,
    pub preds: Vec<(EdgeKind, usize, Ea)>,
    /// Call instructions in the block (the callee as written).
    pub calls: Vec<(Ea, String)>,
    pub blocks: usize,
    pub edges: usize,
    pub back_edges: usize,
}

struct Block {
    start: Ea,
    end: Ea,
    lines: Vec<(Ea, Line)>,
    succ_kinds: Vec<(usize, EdgeKind)>,
}

pub struct GraphView {
    session: Entity<Session>,
    focus: FocusHandle,
    func: Option<Ea>,
    name: SharedString,
    blocks: Vec<Block>,
    layout: Layout,
    edge_kinds: Vec<EdgeKind>,
    /// (block, line) of the caret.
    cursor: Option<(usize, usize)>,
    caret_col: usize,
    highlight: Option<SharedString>,
    pan: Point<Pixels>,
    zoom: f32,
    drag: Option<(Point<Pixels>, Point<Pixels>)>,
    bounds: Option<Bounds<Pixels>>,
    pending_reveal: Option<(usize, usize)>,
    char_w: f32,
    generation: u64,
    loading: bool,
    error: Option<SharedString>,
    pub visible: bool,
    wanted: Option<Ea>,
    /// Minimap rectangle and the scale from graph units, as last painted.
    minimap: Option<(Bounds<Pixels>, f32)>,
    /// The minimap is drawn in the bottom-right corner of the canvas.
    show_minimap: bool,
    _subs: Vec<Subscription>,
}

impl GraphView {
    pub fn new(session: Entity<Session>, cx: &mut Context<Self>) -> Self {
        let sub = cx.subscribe(&session, |this, _, ev, cx| match ev {
            SessionEvent::Navigated { ea, .. } => this.follow_address(*ea, true, cx),
            SessionEvent::CursorMoved { ea, origin } if *origin != Origin::Graph => {
                this.follow_address(*ea, false, cx)
            }
            SessionEvent::Changed => {
                if let Some(f) = this.func {
                    this.load(f, true, cx);
                }
            }
            SessionEvent::DebugChanged => cx.notify(),
            SessionEvent::Closed => {
                this.func = None;
                this.blocks.clear();
                cx.notify();
            }
            _ => {}
        });
        Self {
            session,
            focus: cx.focus_handle(),
            func: None,
            name: SharedString::default(),
            blocks: Vec::new(),
            layout: Layout::default(),
            edge_kinds: Vec::new(),
            cursor: None,
            caret_col: 0,
            highlight: None,
            pan: point(px(0.), px(0.)),
            zoom: 1.,
            drag: None,
            bounds: None,
            pending_reveal: None,
            char_w: 7.8,
            generation: 0,
            loading: false,
            error: None,
            visible: false,
            wanted: None,
            minimap: None,
            show_minimap: true,
            _subs: vec![sub],
        }
    }

    pub fn set_visible(&mut self, visible: bool, cx: &mut Context<Self>) {
        if self.visible == visible {
            return;
        }
        self.visible = visible;
        if visible {
            let ea = self.wanted.unwrap_or(self.session.read(cx).cursor);
            self.follow_address(ea, true, cx);
        }
    }

    fn locate(&self, ea: Ea) -> Option<(usize, usize)> {
        for (bi, b) in self.blocks.iter().enumerate() {
            let first = b.lines.first()?.0;
            let last = b.lines.last()?.0;
            if ea >= first && ea <= last {
                // Prefer the instruction line of the item, as in the listing.
                let mut best = None;
                for (li, (lea, line)) in b.lines.iter().enumerate() {
                    if *lea == ea {
                        best.get_or_insert(li);
                        let t = line.text.trim();
                        if !t.is_empty()
                            && !t.starts_with(';')
                            && !(t.ends_with(':') && !t.contains(' '))
                        {
                            return Some((bi, li));
                        }
                    }
                }
                if let Some(li) = best {
                    return Some((bi, li));
                }
                let li = b.lines.iter().rposition(|(lea, _)| *lea <= ea).unwrap_or(0);
                return Some((bi, li));
            }
        }
        None
    }

    fn follow_address(&mut self, ea: Ea, center: bool, cx: &mut Context<Self>) {
        self.wanted = Some(ea);
        if !self.visible {
            return;
        }
        let func = self.session.read(cx).function_at(ea).map(|f| f.start);
        match func {
            Some(f) if Some(f) == self.func && !self.loading => {
                if let Some(c) = self.locate(ea) {
                    self.cursor = Some(c);
                    self.reveal(c, center);
                    cx.notify();
                }
            }
            Some(f) if Some(f) == self.func => {}
            Some(f) => self.load(f, false, cx),
            None => {
                self.func = None;
                self.blocks.clear();
                self.layout = Layout::default();
                self.error = Some(
                    "The cursor is not inside a function. Press Space for the linear view.".into(),
                );
                cx.notify();
            }
        }
    }

    fn load(&mut self, func: Ea, keep_view: bool, cx: &mut Context<Self>) {
        self.generation += 1;
        let generation = self.generation;
        self.loading = true;
        self.func = Some(func);
        let fut = self.session.read(cx).call(Graph { ea: func });
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = fut.await;
            this.update(cx, |v, cx| {
                if v.generation != generation {
                    return;
                }
                v.loading = false;
                match result {
                    Ok(g) => {
                        v.error = None;
                        v.name = g.name.into();
                        v.blocks = g
                            .blocks
                            .into_iter()
                            .map(|b| Block {
                                start: b.start,
                                end: b.end,
                                lines: {
                                    let eas: Vec<Ea> = b.lines.iter().map(|l| l.ea).collect();
                                    let mut parsed: Vec<Line> =
                                        b.lines.iter().map(|l| tags::parse(&l.tagged)).collect();
                                    tags::compact_block(&mut parsed);
                                    eas.into_iter().zip(parsed).collect()
                                },
                                succ_kinds: b
                                    .succs
                                    .iter()
                                    .map(|s| (s.to as usize, s.kind))
                                    .collect(),
                            })
                            .collect();
                        v.relayout();
                        let wanted = v.wanted.unwrap_or(func);
                        v.cursor = v.locate(wanted).or(Some((0, 0)));
                        if !keep_view {
                            v.zoom = 1.;
                            if let Some(c) = v.cursor {
                                v.reveal(c, true);
                            }
                        }
                    }
                    Err(e) => {
                        v.blocks.clear();
                        v.layout = Layout::default();
                        v.error = Some(e.to_string().into());
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn relayout(&mut self) {
        let cw = self.char_w;
        let sizes: Vec<(f32, f32)> = self
            .blocks
            .iter()
            .map(|b| {
                let chars = b
                    .lines
                    .iter()
                    .map(|(_, l)| l.text.chars().count())
                    .max()
                    .unwrap_or(1);
                (
                    chars as f32 * cw + PAD * 2.,
                    b.lines.len().max(1) as f32 * lh() + PAD * 2. + HEAD,
                )
            })
            .collect();
        let mut edges = Vec::new();
        let mut kinds = Vec::new();
        for (bi, b) in self.blocks.iter().enumerate() {
            for &(to, kind) in &b.succ_kinds {
                if to < self.blocks.len() {
                    edges.push((bi, to));
                    kinds.push(kind);
                }
            }
        }
        self.layout = graph_layout::layout(&sizes, &edges);
        self.edge_kinds = kinds;
    }

    /// Pans so the caret line is visible; `center` puts it a third down.
    fn reveal(&mut self, (bi, li): (usize, usize), center: bool) {
        let Some(bounds) = self.bounds else {
            self.pending_reveal = Some((bi, li));
            return;
        };
        let Some(r) = self.layout.nodes.get(bi).copied() else {
            return;
        };
        let z = self.zoom;
        let gx = r.x + r.w / 2.;
        let gy = r.y + HEAD + PAD + li as f32 * lh();
        let w = f32::from(bounds.size.width);
        let h = f32::from(bounds.size.height);
        let sx = f32::from(self.pan.x) + gx * z;
        let sy = f32::from(self.pan.y) + gy * z;
        let under_minimap = self.minimap.is_some_and(|(mm, _)| {
            let (mx, my) = (
                f32::from(mm.left() - bounds.left()) - 6.,
                f32::from(mm.top() - bounds.top()) - 6.,
            );
            sx >= mx && sy >= my
        });
        if center || sx < 0. || sx > w || sy < 30. || sy > h - 30. || under_minimap {
            let x = if r.w * z <= w - 80. {
                w / 2. - gx * z
            } else {
                40. - r.x * z
            };
            let y = h / 3. - gy * z;
            self.pan = point(px(x), px(y));
        }
    }
    fn cursor_ea(&self) -> Option<Ea> {
        let (b, l) = self.cursor?;
        Some(self.blocks.get(b)?.lines.get(l)?.0)
    }

    fn token(&self) -> Option<String> {
        let (b, l) = self.cursor?;
        let line = &self.blocks.get(b)?.lines.get(l)?.1;
        let r = code::token_at(line, self.caret_col)?;
        Some(line.text[r].to_owned())
    }

    fn to_graph(&self, p: Point<Pixels>) -> Option<(f32, f32)> {
        let b = self.bounds?;
        Some((
            (f32::from(p.x - b.left() - self.pan.x)) / self.zoom,
            (f32::from(p.y - b.top() - self.pan.y)) / self.zoom,
        ))
    }

    fn hit(&self, p: Point<Pixels>) -> Option<(usize, usize, usize)> {
        let (gx, gy) = self.to_graph(p)?;
        for (bi, r) in self.layout.nodes.iter().enumerate() {
            if gx >= r.x && gx <= r.right() && gy >= r.y && gy <= r.bottom() {
                let li = ((gy - r.y - HEAD - PAD) / lh()).floor().max(0.) as usize;
                let li = li.min(self.blocks[bi].lines.len().saturating_sub(1));
                let col = ((gx - r.x - PAD) / self.char_w).max(0.) as usize;
                let text = &self.blocks[bi].lines.get(li)?.1.text;
                // Column in characters to byte offset.
                let col = text
                    .char_indices()
                    .nth(col)
                    .map(|(i, _)| i)
                    .unwrap_or(text.len());
                return Some((bi, li, col));
            }
        }
        None
    }

    fn publish_caret(&mut self, cx: &mut Context<Self>) {
        let Some((b, l)) = self.cursor else { return };
        let caret = Caret {
            origin: Some(Origin::Graph),
            line: Some(l as u32 + 1),
            col: self.caret_col as u32 + 1,
            block: Some(b),
            token: self.token(),
        };
        self.session.update(cx, |s, cx| s.set_caret(caret, cx));
    }

    /// The instruction under a click in the margin left of a block, where the
    /// breakpoint dots sit.
    fn gutter_ea(&self, p: Point<Pixels>) -> Option<Ea> {
        let b = self.bounds?;
        let (_, gy) = self.to_graph(p)?;
        for (bi, r) in self.layout.nodes.iter().enumerate() {
            let left = b.left() + self.pan.x + px(r.x * self.zoom);
            if p.x >= left - px(code::MARK_W) && p.x < left && gy >= r.y && gy <= r.bottom() {
                let li = ((gy - r.y - HEAD - PAD) / lh()).floor();
                if li < 0. {
                    return None;
                }
                return self.blocks.get(bi)?.lines.get(li as usize).map(|l| l.0);
            }
        }
        None
    }

    fn on_mouse_down(&mut self, ev: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.focus, cx);
        if self.minimap_click(ev.position) {
            cx.notify();
            return;
        }
        if let Some(ea) = self.gutter_ea(ev.position) {
            self.session.update(cx, |s, cx| s.dbg_toggle_bpt(ea, cx));
            return;
        }
        match self.hit(ev.position) {
            Some((b, l, col)) => {
                self.cursor = Some((b, l));
                self.caret_col = col;
                self.highlight = self.token().map(Into::into);
                if let Some(ea) = self.cursor_ea() {
                    self.session
                        .update(cx, |s, cx| s.set_cursor(ea, Origin::Graph, cx));
                }
                self.publish_caret(cx);
                if ev.click_count >= 2 {
                    self.follow(cx);
                }
            }
            None => self.drag = Some((ev.position, self.pan)),
        }
        cx.notify();
    }

    fn on_mouse_move(&mut self, ev: &MouseMoveEvent, _: &mut Window, cx: &mut Context<Self>) {
        if let Some((start, pan)) = self.drag {
            if ev.pressed_button != Some(MouseButton::Left) {
                self.drag = None;
                return;
            }
            self.pan = pan + (ev.position - start);
            cx.notify();
        }
    }

    fn on_mouse_up(&mut self, _: &MouseUpEvent, _: &mut Window, _: &mut Context<Self>) {
        self.drag = None;
    }

    fn on_scroll(&mut self, ev: &ScrollWheelEvent, _: &mut Window, cx: &mut Context<Self>) {
        let delta = ev.delta.pixel_delta(px(line_height()));
        if ev.modifiers.control {
            let factor = (1. + f32::from(delta.y) / 400.).clamp(0.5, 2.);
            self.zoom_at(factor, Some(ev.position));
        } else if ev.modifiers.shift {
            self.pan.x += delta.y;
        } else {
            self.pan += delta;
        }
        cx.notify();
    }

    fn zoom_at(&mut self, factor: f32, at: Option<Point<Pixels>>) {
        let new = (self.zoom * factor).clamp(0.15, 3.);
        let anchor = at
            .or_else(|| self.bounds.map(|b| b.center()))
            .and_then(|p| self.bounds.map(|b| p - b.origin));
        if let Some(a) = anchor {
            // Keep the graph point under `a` fixed.
            let gx = (f32::from(a.x - self.pan.x)) / self.zoom;
            let gy = (f32::from(a.y - self.pan.y)) / self.zoom;
            self.pan = point(a.x - px(gx * new), a.y - px(gy * new));
        }
        self.zoom = new;
    }

    fn follow(&mut self, cx: &mut Context<Self>) {
        let Some(ea) = self.cursor_ea() else { return };
        if let Some(token) = self.token() {
            commands::follow_token(&self.session, token, ea, cx);
        }
    }

    fn move_cursor(&mut self, delta: isize, cx: &mut Context<Self>) {
        // Walk lines in block order, then address order.
        let mut flat: Vec<(usize, usize, Ea)> = Vec::new();
        for (bi, b) in self.blocks.iter().enumerate() {
            for (li, (ea, _)) in b.lines.iter().enumerate() {
                flat.push((bi, li, *ea));
            }
        }
        flat.sort_by_key(|&(bi, li, _)| {
            (self.blocks[bi].lines.first().map(|l| l.0).unwrap_or(0), li)
        });
        let Some(cur) = self.cursor else { return };
        let Some(i) = flat.iter().position(|&(b, l, _)| (b, l) == cur) else {
            return;
        };
        let j = (i as isize + delta).clamp(0, flat.len() as isize - 1) as usize;
        let (b, l, ea) = flat[j];
        self.cursor = Some((b, l));
        self.caret_col = 0;
        self.reveal((b, l), false);
        self.session
            .update(cx, |s, cx| s.set_cursor(ea, Origin::Graph, cx));
        self.publish_caret(cx);
        cx.notify();
    }

    fn edit(&mut self, kind: EditKind, cx: &mut Context<Self>) {
        if let Some(ea) = self.cursor_ea() {
            commands::edit(&self.session, kind, ea, cx);
        }
    }

    /// The block under the caret, for the inspector.
    pub fn block_info(&self) -> Option<BlockInfo> {
        let (bi, _) = self.cursor?;
        let b = self.blocks.get(bi)?;
        let succs = b
            .succ_kinds
            .iter()
            .filter_map(|&(to, k)| Some((k, to, self.blocks.get(to)?.start)))
            .collect();
        let mut preds = Vec::new();
        for (pi, p) in self.blocks.iter().enumerate() {
            for &(to, k) in &p.succ_kinds {
                if to == bi {
                    preds.push((k, pi, p.start));
                }
            }
        }
        let is_insn = |t: &str| {
            let t = t.trim();
            !t.is_empty() && !t.starts_with(';') && !(t.ends_with(':') && !t.contains(' '))
        };
        let insns: Vec<&(Ea, Line)> = b.lines.iter().filter(|(_, l)| is_insn(&l.text)).collect();
        let terminator = insns
            .last()
            .map(|(_, l)| l.text.split_whitespace().collect::<Vec<_>>().join(" "))
            .unwrap_or_default();
        let calls = insns
            .iter()
            .filter_map(|(ea, l)| {
                let t = l.text.trim();
                let rest = t.strip_prefix("call")?.trim_start();
                let target = rest.split(';').next().unwrap_or(rest).trim();
                (!target.is_empty()).then(|| (*ea, target.to_owned()))
            })
            .collect();
        Some(BlockInfo {
            index: bi,
            start: b.start,
            end: b.end,
            insns: insns.len(),
            terminator,
            succs,
            preds,
            calls,
            blocks: self.blocks.len(),
            edges: self.blocks.iter().map(|b| b.succ_kinds.len()).sum(),
            back_edges: self.layout.edges.iter().filter(|e| e.back).count(),
        })
    }

    /// Zoom and pan so the whole graph is visible.
    fn fit(&mut self) {
        let Some(bounds) = self.bounds else { return };
        let (gw, gh) = (self.layout.width.max(1.), self.layout.height.max(1.));
        let (w, h) = (f32::from(bounds.size.width), f32::from(bounds.size.height));
        // Leave the strip with the minimap free so it covers nothing.
        let h = match self.minimap {
            Some((mm, _)) => h - f32::from(mm.size.height) - 12. - MINIMAP_GAP,
            None => h,
        };
        let z = ((w - 48.) / gw).min((h - 48.) / gh).clamp(0.15, 1.);
        self.zoom = z;
        self.pan = point(px((w - gw * z) / 2.), px((h - gh * z) / 2.));
    }

    /// A click in the minimap centres the view on that graph point.
    fn minimap_click(&mut self, p: Point<Pixels>) -> bool {
        let (Some((mm, scale)), Some(b)) = (self.minimap, self.bounds) else {
            return false;
        };
        if !mm.contains(&p) {
            return false;
        }
        let gx = f32::from(p.x - mm.left()) / scale;
        let gy = f32::from(p.y - mm.top()) / scale;
        let (w, h) = (f32::from(b.size.width), f32::from(b.size.height));
        self.pan = point(px(w / 2. - gx * self.zoom), px(h / 2. - gy * self.zoom));
        true
    }

    /// The scrollbars' view of the canvas: the graph against the viewport.
    fn pan_source(&self, cx: &mut Context<Self>) -> scrollbar::Source {
        scrollbar::pan_source(
            cx.entity().downgrade(),
            |v: &Self| {
                let b = v.bounds?;
                Some(scrollbar::PanState {
                    pan: (f32::from(v.pan.x), f32::from(v.pan.y)),
                    zoom: v.zoom,
                    graph: (v.layout.width, v.layout.height),
                    viewport: (f32::from(b.size.width), f32::from(b.size.height)),
                })
            },
            |v, axis, pan, cx| {
                match axis {
                    scrollbar::Axis::Vertical => v.pan.y = px(pan),
                    scrollbar::Axis::Horizontal => v.pan.x = px(pan),
                }
                cx.notify();
            },
        )
    }

    fn render_canvas(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let entity = cx.entity();
        canvas(
            |_, _, _| (),
            move |bounds, (), window, cx| {
                let theme = Theme::get(cx);
                let cw = f32::from(code::char_width(window));
                let v = entity.read(cx);
                let zoom = v.zoom;
                let pan = v.pan;
                let layout = v.layout.clone();
                let kinds = v.edge_kinds.clone();
                let marks = code::DebugMarks::read(v.session.read(cx));
                let cursor = v.cursor;
                let highlight = v.highlight.clone();
                let focused = v.focus.is_focused(window);
                let show_minimap = v.show_minimap;
                let blocks: Vec<(Ea, Vec<(Ea, Line)>)> = v
                    .blocks
                    .iter()
                    .map(|b| (b.start, b.lines.clone()))
                    .collect();
                let char_changed = (v.char_w - cw).abs() > 0.01;
                let to_screen = |x: f32, y: f32| {
                    point(
                        bounds.left() + pan.x + px(x * zoom),
                        bounds.top() + pan.y + px(y * zoom),
                    )
                };
                window.with_content_mask(Some(gpui::ContentMask { bounds }), |window| {
                    // Edges under blocks: taken = success, not taken = danger, unconditional = accent.
                    for (k, e) in layout.edges.iter().enumerate() {
                        let color = if e.back {
                            hsla(theme.accent).opacity(0.75)
                        } else {
                            match kinds.get(k) {
                                Some(EdgeKind::Taken) => hsla(theme.success),
                                Some(EdgeKind::NotTaken) => hsla(theme.danger),
                                _ => hsla(theme.accent),
                            }
                        };
                        graph_paint::edge(window, &e.points, color, zoom, &to_screen);
                    }
                    // Blocks.
                    let fsize = px(font_size() * zoom);
                    let line_h = lh() * zoom;
                    let head_h = HEAD * zoom;
                    for (bi, r) in layout.nodes.iter().enumerate() {
                        let origin = to_screen(r.x, r.y);
                        let rect = Bounds::new(origin, size(px(r.w * zoom), px(r.h * zoom)));
                        if !rect.intersects(&bounds) {
                            continue;
                        }
                        let current = cursor.is_some_and(|(b, _)| b == bi);
                        window.paint_quad(gpui::quad(
                            rect,
                            px(6. * zoom),
                            hsla(theme.surface),
                            px(if current { 1.5 } else { 1. }),
                            hsla(if current {
                                theme.accent
                            } else {
                                theme.border_strong
                            }),
                            gpui::BorderStyle::Solid,
                        ));
                        let Some((start, lines)) = blocks.get(bi) else {
                            continue;
                        };
                        // Header strip: address left, block number right.
                        window.paint_quad(gpui::fill(
                            Bounds::new(
                                point(origin.x + px(1.), origin.y + px(1.)),
                                size(rect.size.width - px(2.), px(head_h - 1.)),
                            ),
                            hsla(theme.surface2),
                        ));
                        if zoom >= 0.35 {
                            let small = px((11. * zoom).max(6.));
                            let note_h = px(11. * zoom * 1.4);
                            let hy = origin.y + px((head_h - 11. * zoom * 1.4) / 2.);
                            let left = code::shape_plain_sized(
                                window,
                                &format!("{start:#x}"),
                                hsla(theme.muted),
                                small,
                            );
                            let _ = left.paint(
                                point(origin.x + px(PAD * zoom), hy),
                                note_h,
                                gpui::TextAlign::Left,
                                None,
                                window,
                                cx,
                            );
                            let num = code::shape_plain_sized(
                                window,
                                &format!("#{bi}"),
                                hsla(theme.muted),
                                small,
                            );
                            let _ = num.paint(
                                point(origin.x + rect.size.width - num.width - px(PAD * zoom), hy),
                                note_h,
                                gpui::TextAlign::Left,
                                None,
                                window,
                                cx,
                            );
                        }
                        for (li, (lea, line)) in lines.iter().enumerate() {
                            let y = origin.y + px((HEAD + PAD + li as f32 * lh()) * zoom);
                            if y + px(line_h) < bounds.top() || y > bounds.bottom() {
                                continue;
                            }
                            let marked = marks.row_bg(*lea, &theme);
                            if let Some(bg) = marked {
                                window.paint_quad(gpui::fill(
                                    Bounds::new(
                                        point(origin.x + px(1.), y),
                                        size(rect.size.width - px(2.), px(line_h)),
                                    ),
                                    bg,
                                ));
                            }
                            marks.paint_at(
                                *lea,
                                origin.x - px(code::MARK_W),
                                y,
                                px(line_h),
                                &theme,
                                window,
                            );
                            if marked.is_none() && cursor == Some((bi, li)) {
                                window.paint_quad(gpui::fill(
                                    Bounds::new(
                                        point(origin.x + px(1.), y),
                                        size(rect.size.width - px(2.), px(line_h)),
                                    ),
                                    hsla(if focused {
                                        theme.caret_row
                                    } else {
                                        theme.hover
                                    }),
                                ));
                            }
                            if zoom < 0.35 {
                                continue; // too small to read; boxes and edges only
                            }
                            let font = code::code_font();
                            let runs: Vec<TextRun> =
                                code::text_runs(line, &theme, highlight.as_deref(), &font);
                            let shaped = window.text_system().shape_line(
                                line.text.clone().into(),
                                fsize,
                                &runs,
                                None,
                            );
                            let _ = shaped.paint(
                                point(origin.x + px(PAD * zoom), y),
                                px(line_h),
                                gpui::TextAlign::Left,
                                None,
                                window,
                                cx,
                            );
                        }
                    }
                    // Minimap: the blocks at a glance and the part in view.
                    let minimap = if show_minimap {
                        graph_paint::minimap(window, bounds, &theme, &layout, pan, zoom)
                    } else {
                        None
                    };
                    entity.update(cx, |v, _| v.minimap = minimap);
                });
                entity.update(cx, |v, cx| {
                    v.bounds = Some(bounds);
                    if char_changed {
                        v.char_w = cw;
                        v.relayout();
                    }
                    // A reveal requested before the first paint: apply once
                    // and repaint (one extra frame, not a loop).
                    if let Some(c) = v.pending_reveal.take() {
                        v.reveal(c, true);
                        cx.notify();
                    }
                });
            },
        )
        .size_full()
    }

    /// The strip above the canvas: zoom controls on the left, the edge legend
    /// in the middle and the minimap toggle on the right. Being a row of its
    /// own, it can never cover code.
    fn render_header(&self, theme: &Theme, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let t = *theme;
        let pct = format!("{:.0}%", self.zoom * 100.);
        let swatch = |color: u32, label: &'static str| {
            div()
                .flex()
                .items_center()
                .gap(px(6.))
                .child(div().w(px(16.)).h(px(2.)).rounded(px(1.)).bg(hsla(color)))
                .child(label)
        };
        div()
            .h(px(HEADER_H))
            .flex_none()
            .flex()
            .items_center()
            .gap(px(2.))
            .px(px(8.))
            .bg(hsla(t.surface2))
            .border_b_1()
            .border_color(hsla(t.border))
            .child(
                ui::icon_button("zoom-out", "minus", 24., false, true, &t)
                    .tooltip(ui::tip("Zoom out", None))
                    .on_click(cx.listener(|v, _, _, cx| {
                        v.zoom_at(1. / 1.2, None);
                        cx.notify();
                    })),
            )
            .child(
                div()
                    .w(px(48.))
                    .flex()
                    .justify_center()
                    .text_size(px(11.))
                    .font_weight(gpui::FontWeight::MEDIUM)
                    .font_family(crate::theme::code_font_name())
                    .child(pct),
            )
            .child(
                ui::icon_button("zoom-in", "plus", 24., false, true, &t)
                    .tooltip(ui::tip("Zoom in", None))
                    .on_click(cx.listener(|v, _, _, cx| {
                        v.zoom_at(1.2, None);
                        cx.notify();
                    })),
            )
            .child(
                ui::icon_button("zoom-fit", "expand", 24., false, true, &t)
                    .tooltip(ui::tip("Fit graph", None))
                    .on_click(cx.listener(|v, _, _, cx| {
                        v.fit();
                        cx.notify();
                    })),
            )
            .child(div().flex_1())
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(16.))
                    .text_size(px(11.))
                    .text_color(hsla(t.muted))
                    .child(swatch(t.success, "Taken"))
                    .child(swatch(t.danger, "Not taken"))
                    .child(swatch(t.accent, "Unconditional")),
            )
            .child(div().flex_1())
            .child(
                ui::icon_button("minimap-toggle", "view", 24., self.show_minimap, true, &t)
                    .tooltip(ui::tip(
                        "Minimap",
                        Some(crate::actions::shortcut_text(&ToggleMinimap, cx)),
                    ))
                    .on_click(cx.listener(|v, _, _, cx| v.toggle_minimap(cx))),
            )
    }

    pub fn toggle_minimap(&mut self, cx: &mut Context<Self>) {
        self.show_minimap = !self.show_minimap;
        if !self.show_minimap {
            // A hidden minimap must not keep swallowing clicks.
            self.minimap = None;
        }
        cx.notify();
    }
}

impl Focusable for GraphView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for GraphView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::get(cx);
        let bar = scrollbar::scrollbar(
            "canvas-scroll",
            self.pan_source(cx),
            Axes::BOTH,
            &theme,
            window,
            cx,
        );
        let header = self.render_header(&theme, cx);
        let busy = self
            .loading
            .then(|| SharedString::from(format!("Building graph of {}…", self.name)));
        div()
            .id("graph")
            .key_context("CodeView")
            .track_focus(&self.focus)
            .size_full()
            .flex()
            .flex_col()
            .bg(hsla(theme.surface))
            .on_action(cx.listener(|v, _: &CursorUp, _, cx| v.move_cursor(-1, cx)))
            .on_action(cx.listener(|v, _: &CursorDown, _, cx| v.move_cursor(1, cx)))
            .on_action(cx.listener(|v, _: &PageUp, _, cx| v.move_cursor(-20, cx)))
            .on_action(cx.listener(|v, _: &PageDown, _, cx| v.move_cursor(20, cx)))
            .on_action(cx.listener(|v, _: &Follow, _, cx| v.follow(cx)))
            .on_action(cx.listener(|v, _: &ZoomIn, _, cx| {
                v.zoom_at(1.2, None);
                cx.notify();
            }))
            .on_action(cx.listener(|v, _: &ZoomOut, _, cx| {
                v.zoom_at(1. / 1.2, None);
                cx.notify();
            }))
            .on_action(cx.listener(|v, _: &ZoomReset, _, cx| {
                v.zoom = 1.;
                if let Some(c) = v.cursor {
                    v.reveal(c, true);
                }
                cx.notify();
            }))
            .on_action(cx.listener(|v, _: &Rename, _, cx| {
                let Some(ea) = v.cursor_ea() else { return };
                commands::rename_token(&v.session, v.token(), ea, cx);
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
            .on_action(cx.listener(|v, _: &MakeCode, _, cx| v.edit(EditKind::Code, cx)))
            .on_action(cx.listener(|v, _: &MakeData, _, cx| v.edit(EditKind::Data, cx)))
            .on_action(cx.listener(|v, _: &Undefine, _, cx| v.edit(EditKind::Undefine, cx)))
            .on_action(cx.listener(|v, _: &MakeFunction, _, cx| v.edit(EditKind::Function, cx)))
            .on_action(
                cx.listener(|v, _: &DeleteFunction, _, cx| v.edit(EditKind::DeleteFunction, cx)),
            )
            .child(header)
            .child(
                div()
                    .id("graph-canvas")
                    .flex_1()
                    .min_h_0()
                    .relative()
                    .overflow_hidden()
                    .on_scroll_wheel(cx.listener(Self::on_scroll))
                    .on_mouse_down(MouseButton::Left, cx.listener(Self::on_mouse_down))
                    .on_mouse_move(cx.listener(Self::on_mouse_move))
                    .on_mouse_up(MouseButton::Left, cx.listener(Self::on_mouse_up))
                    .on_mouse_up_out(MouseButton::Left, cx.listener(Self::on_mouse_up))
                    .child(self.render_canvas(cx))
                    .child(bar)
                    .when_some(self.error.clone().or(busy), |d, e| {
                        d.child(
                            div()
                                .absolute()
                                .top(px(56.))
                                .left(px(16.))
                                .text_color(hsla(theme.muted))
                                .child(e),
                        )
                    }),
            )
    }
}
