// Control-flow graph of the function at the cursor: blocks of listing lines,
// laid out by `graph_layout`, painted on a pannable / zoomable canvas.
use crate::actions::*;
use crate::code::{self, font_size, line_height};
use crate::commands::{self, EditKind};
use crate::graph_layout::{self, Layout};
use crate::graph_paint::{self, MINIMAP_GAP};
use crate::scrollbar::{self, Axes};
use crate::selection::{Anchor, CopyLine, lines_text};
use crate::session::{Caret, Origin, Session, SessionEvent, ViewKind};
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
/// Smallest zoom (3%), so a function of hundreds of blocks fits on screen.
const MIN_ZOOM: f32 = 0.03;
/// Below this zoom blocks are drawn as plain boxes: the text would be unreadable.
const TEXT_ZOOM: f32 = 0.3;
/// Canvases narrower than this hide the minimap.
const MINIMAP_MIN_W: f32 = 600.;
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
    /// While a long block is folded: its complete lines. `lines` then holds
    /// the head, the fold chip (at index `chip`) and the tail.
    full: Option<Vec<(Ea, Line)>>,
    chip: Option<usize>,
}

/// Blocks longer than this fold their middle into one "N lines" chip.
const FOLD_OVER: usize = 40;
/// Lines kept at each end of a folded block.
const FOLD_KEEP: usize = 14;

/// The line of `lines` that stands for the item at `ea`: its instruction
/// line, as in the listing (a segment banner or label shares the address).
fn locate_line(lines: &[(Ea, Line)], ea: Ea) -> Option<usize> {
    let (first, last) = (lines.first()?.0, lines.last()?.0);
    if ea < first || ea > last {
        return None;
    }
    let mut best = None;
    for (li, (lea, line)) in lines.iter().enumerate() {
        if *lea == ea {
            best.get_or_insert(li);
            let t = line.text.trim();
            if !t.is_empty() && !t.starts_with(';') && !(t.ends_with(':') && !t.contains(' ')) {
                return Some(li);
            }
        }
    }
    best.or_else(|| Some(lines.iter().rposition(|(lea, _)| *lea <= ea).unwrap_or(0)))
}

/// Folds a long block: (lines to show, the full lines, chip index).
fn fold_lines(lines: Vec<(Ea, Line)>) -> (Vec<(Ea, Line)>, Option<Vec<(Ea, Line)>>, Option<usize>) {
    if lines.len() <= FOLD_OVER {
        return (lines, None, None);
    }
    let hidden = lines.len() - 2 * FOLD_KEEP;
    let text = format!("\u{22ef} {hidden} lines hidden (click to expand)");
    let mut chip = Line::plain(text.clone());
    chip.runs = vec![tags::Run {
        range: 0..text.len(),
        color: tags::color::REGCMT,
    }];
    let mut shown: Vec<(Ea, Line)> = lines[..FOLD_KEEP].to_vec();
    shown.push((lines[FOLD_KEEP].0, chip));
    shown.extend_from_slice(&lines[lines.len() - FOLD_KEEP..]);
    (shown, Some(lines), Some(FOLD_KEEP))
}

pub struct GraphView {
    session: Entity<Session>,
    /// This view\'s identity in `SessionEvent::CursorMoved`.
    origin: Origin,
    /// A pinned view (a tab opened with "Open in new tab") only moves on its own jumps.
    pub pinned: bool,
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
    /// A selection runs between this line and the caret line, inside the
    /// block `sel_block` (a selection never spans blocks).
    sel: Anchor,
    sel_block: usize,
    /// The (block, line) a mouse drag selection started on.
    drag_sel: Option<(usize, usize)>,
    /// How far the pointer is past the canvas edge while selecting (x, y), and where it is.
    auto_vec: (f32, f32),
    auto_running: bool,
    last_pointer: Point<Pixels>,
    _subs: Vec<Subscription>,
}

impl GraphView {
    /// Where to show when first shown (a tab opened hidden).
    pub fn set_wanted(&mut self, ea: Ea) {
        self.wanted = Some(ea);
    }

    pub fn origin(&self) -> Origin {
        self.origin
    }

    /// The address this view is at (for a tab's title), when it has one.
    pub fn current_ea(&self) -> Option<Ea> {
        self.cursor_ea().or(self.func)
    }

    pub fn new(session: Entity<Session>, cx: &mut Context<Self>) -> Self {
        let sub = cx.subscribe(&session, |this, _, ev, cx| match ev {
            SessionEvent::Navigated { ea, .. }
                if this.session.read(cx).follows(this.origin, this.pinned) =>
            {
                this.follow_address(*ea, true, cx)
            }
            SessionEvent::CursorMoved { ea, origin }
                if *origin != this.origin
                    && this.session.read(cx).follows(this.origin, this.pinned) =>
            {
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
            pinned: false,
            origin: Origin::new(ViewKind::Graph),
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
            sel: Anchor::default(),
            sel_block: 0,
            drag_sel: None,
            auto_vec: (0., 0.),
            auto_running: false,
            last_pointer: point(px(0.), px(0.)),
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
        self.blocks
            .iter()
            .enumerate()
            .find_map(|(bi, b)| locate_line(&b.lines, ea).map(|li| (bi, li)))
    }

    /// Shows the whole of a folded block; the caret keeps its line.
    fn expand_block(&mut self, bi: usize) {
        let Some(b) = self.blocks.get_mut(bi) else {
            return;
        };
        let (Some(full), Some(chip)) = (b.full.take(), b.chip.take()) else {
            return;
        };
        let grown = full.len() - b.lines.len();
        b.lines = full;
        self.sel.clear();
        if let Some((cb, cl)) = &mut self.cursor
            && *cb == bi
            && *cl > chip
        {
            *cl += grown;
        }
        self.relayout();
    }

    /// Expands the block that hides the item at `ea`, if any.
    fn unfold_for(&mut self, ea: Ea) {
        let hit = self.blocks.iter().position(|b| {
            b.full.as_ref().is_some_and(|f| {
                locate_line(f, ea).is_some_and(|li| li >= FOLD_KEEP && li < f.len() - FOLD_KEEP)
            })
        });
        if let Some(bi) = hit {
            self.expand_block(bi);
        }
    }

    fn follow_address(&mut self, ea: Ea, center: bool, cx: &mut Context<Self>) {
        self.wanted = Some(ea);
        if !self.visible {
            return;
        }
        let func = self.session.read(cx).function_at(ea).map(|f| f.start);
        match func {
            Some(f) if Some(f) == self.func && !self.loading => {
                self.unfold_for(ea);
                if let Some(c) = self.locate(ea) {
                    self.sel.clear();
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
                            .map(|b| {
                                let eas: Vec<Ea> = b.lines.iter().map(|l| l.ea).collect();
                                let mut parsed: Vec<Line> =
                                    b.lines.iter().map(|l| tags::parse(&l.tagged)).collect();
                                tags::compact_block(&mut parsed);
                                let (lines, full, chip) =
                                    fold_lines(eas.into_iter().zip(parsed).collect());
                                Block {
                                    start: b.start,
                                    end: b.end,
                                    lines,
                                    succ_kinds: b
                                        .succs
                                        .iter()
                                        .map(|s| (s.to as usize, s.kind))
                                        .collect(),
                                    full,
                                    chip,
                                }
                            })
                            .collect();
                        v.relayout();
                        let wanted = v.wanted.unwrap_or(func);
                        v.unfold_for(wanted);
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
    pub fn cursor_ea(&self) -> Option<Ea> {
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
            origin: Some(self.origin),
            line: Some(l as u32 + 1),
            col: self.caret_col as u32 + 1,
            block: Some(b),
            token: self.token(),
            operand: self
                .blocks
                .get(b)
                .and_then(|k| k.lines.get(l))
                .and_then(|(_, line)| crate::operands::operand_at(&line.text, self.caret_col)),
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
                if ev.modifiers.shift && self.cursor.is_some_and(|(cb, _)| cb == b) {
                    self.sel_block = b;
                    self.sel.begin(self.cursor.map_or(l, |(_, cl)| cl));
                } else {
                    self.sel.clear();
                    self.drag_sel = Some((b, l));
                }
                self.cursor = Some((b, l));
                self.caret_col = col;
                self.highlight = self.token().map(Into::into);
                if let Some(ea) = self.cursor_ea() {
                    self.session
                        .update(cx, |s, cx| s.set_cursor(ea, self.origin, cx));
                }
                if self.blocks.get(b).is_some_and(|k| k.chip == Some(l)) {
                    self.expand_block(b);
                }
                self.publish_caret(cx);
                if ev.modifiers.control
                    && let Some(ea) = self.cursor_ea()
                {
                    commands::open_target_in_tab(
                        &self.session,
                        ViewKind::Graph,
                        self.token(),
                        ea,
                        cx,
                    );
                } else if ev.click_count >= 2 {
                    self.follow(cx);
                }
            }
            None => self.drag = Some((ev.position, self.pan)),
        }
        cx.notify();
    }

    /// Extends the drag selection to the line under `p`, clamped to the block
    /// it started in (past its top or bottom: the first or last line).
    fn extend_drag_selection(&mut self, p: Point<Pixels>, cx: &mut Context<Self>) {
        let Some((sb, sl)) = self.drag_sel else {
            return;
        };
        let (Some((gx, gy)), Some(r), Some(block)) = (
            self.to_graph(p),
            self.layout.nodes.get(sb).copied(),
            self.blocks.get(sb),
        ) else {
            return;
        };
        let last = block.lines.len().saturating_sub(1);
        let li = (((gy - r.y - HEAD - PAD) / lh()).floor().max(0.) as usize).min(last);
        let col = ((gx - r.x - PAD) / self.char_w).max(0.) as usize;
        let col = block.lines.get(li).map_or(0, |(_, line)| {
            line.text
                .char_indices()
                .nth(col)
                .map(|(i, _)| i)
                .unwrap_or(line.text.len())
        });
        if self.cursor != Some((sb, li)) {
            self.sel_block = sb;
            self.sel.begin(sl);
            self.cursor = Some((sb, li));
            self.caret_col = col;
            if let Some(ea) = self.cursor_ea() {
                self.session
                    .update(cx, |s, cx| s.set_cursor(ea, self.origin, cx));
            }
            self.publish_caret(cx);
            cx.notify();
        }
    }

    /// Past the edge of the canvas with the button down: the graph pans, and
    /// the selection follows, until the button is released.
    fn start_autoscroll(&mut self, cx: &mut Context<Self>) {
        if self.auto_running {
            return;
        }
        self.auto_running = true;
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(std::time::Duration::from_millis(30))
                    .await;
                let go = this
                    .update(cx, |v, cx| {
                        let (dx, dy) = v.auto_vec;
                        if v.drag_sel.is_none() || (dx == 0. && dy == 0.) {
                            v.auto_running = false;
                            return false;
                        }
                        // The farther past the edge, the faster; the content moves against the pointer.
                        v.pan.x -= px((dx * 0.3).clamp(-30., 30.));
                        v.pan.y -= px((dy * 0.3).clamp(-30., 30.));
                        let at = v.last_pointer;
                        v.extend_drag_selection(at, cx);
                        cx.notify();
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

    fn on_mouse_move(&mut self, ev: &MouseMoveEvent, _: &mut Window, cx: &mut Context<Self>) {
        if self.drag_sel.is_some() {
            if ev.pressed_button != Some(MouseButton::Left) {
                self.drag_sel = None;
            } else {
                self.last_pointer = ev.position;
                self.extend_drag_selection(ev.position, cx);
                if let Some(b) = self.bounds {
                    let past = |v: Pixels, lo: Pixels, hi: Pixels| {
                        if v < lo {
                            f32::from(v - lo)
                        } else if v > hi {
                            f32::from(v - hi)
                        } else {
                            0.
                        }
                    };
                    self.auto_vec = (
                        past(ev.position.x, b.left(), b.right()),
                        past(ev.position.y, b.top(), b.bottom()),
                    );
                    if self.auto_vec != (0., 0.) {
                        self.start_autoscroll(cx);
                    }
                }
            }
        }
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
        self.drag_sel = None;
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
        let new = (self.zoom * factor).clamp(MIN_ZOOM, 3.);
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
        if let Some((b, l)) = self.cursor
            && self.blocks.get(b).is_some_and(|k| k.chip == Some(l))
        {
            self.expand_block(b);
            return;
        }
        let token = self.cursor.and_then(|(b, l)| {
            let line = &self.blocks.get(b)?.lines.get(l)?.1;
            let r = code::follow_token_at(line, self.caret_col)?;
            Some(line.text[r].to_owned())
        });
        if let Some(token) = token {
            commands::follow_token(&self.session, token, ea, cx);
        }
    }

    /// The selected lines of the caret's block, if any.
    fn selection(&self) -> Option<(usize, std::ops::RangeInclusive<usize>)> {
        let (b, l) = self.cursor?;
        (b == self.sel_block)
            .then(|| self.sel.range(l))
            .flatten()
            .map(|r| (b, r))
    }

    /// Shift+arrow, PageUp / PageDown, Home / End: extends the selection
    /// within the block under the caret (`delta` lines, saturating).
    fn extend(&mut self, delta: isize, cx: &mut Context<Self>) {
        let Some((b, l)) = self.cursor else { return };
        let Some(block) = self.blocks.get(b) else {
            return;
        };
        if self.sel_block != b {
            self.sel.clear();
        }
        self.sel_block = b;
        self.sel.begin(l);
        let to = (l as isize + delta).clamp(0, block.lines.len() as isize - 1) as usize;
        let ea = block.lines[to].0;
        self.cursor = Some((b, to));
        self.caret_col = 0;
        self.reveal((b, to), false);
        self.session
            .update(cx, |s, cx| s.set_cursor(ea, self.origin, cx));
        self.publish_caret(cx);
        cx.notify();
    }

    /// Where a keyboard-opened context menu appears: under the caret line.
    fn caret_position(&self) -> Point<Pixels> {
        let (Some(b), Some((bi, li))) = (self.bounds, self.cursor) else {
            return point(px(200.), px(200.));
        };
        match self.layout.nodes.get(bi) {
            Some(r) => {
                b.origin
                    + point(
                        self.pan.x
                            + px((r.x + PAD + self.caret_col as f32 * self.char_w) * self.zoom),
                        self.pan.y + px((r.y + HEAD + PAD + (li as f32 + 1.) * lh()) * self.zoom),
                    )
            }
            None => point(px(200.), px(200.)),
        }
    }

    fn open_context_menu(&mut self, pos: Point<Pixels>, keyboard: bool, cx: &mut Context<Self>) {
        let Some(ea) = self.cursor_ea() else { return };
        let title = self.token().unwrap_or_else(|| format!("{ea:X}"));
        let entries = crate::context_menu::listing(&self.session.read(cx).ops(), cx);
        crate::context_menu::show(&self.session, pos, title, entries, keyboard, cx);
    }

    /// Right click on a line: the caret goes there first (unless it is inside
    /// the selection), then the menu opens.
    fn on_right_down(&mut self, ev: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.focus, cx);
        let Some((b, l, col)) = self.hit(ev.position) else {
            return;
        };
        let inside = self
            .selection()
            .is_some_and(|(sb, r)| sb == b && r.contains(&l));
        if !inside {
            self.sel.clear();
            self.cursor = Some((b, l));
            self.caret_col = col;
            self.highlight = self.token().map(Into::into);
            if let Some(ea) = self.cursor_ea() {
                self.session
                    .update(cx, |s, cx| s.set_cursor(ea, self.origin, cx));
            }
            self.publish_caret(cx);
        }
        self.open_context_menu(ev.position, false, cx);
        cx.notify();
    }

    /// Esc: with a selection it only clears the selection; otherwise it is
    /// "back" as everywhere (the action keeps travelling up).
    fn on_back(&mut self, cx: &mut Context<Self>) {
        if self.selection().is_some() {
            self.sel.clear();
            cx.notify();
        } else {
            cx.propagate();
        }
    }

    /// Ctrl+C: the selected lines, or the current one.
    fn copy(&mut self, with_addresses: bool, cx: &mut Context<Self>) {
        let Some((b, l)) = self.cursor else { return };
        let Some(block) = self.blocks.get(b) else {
            return;
        };
        let range = self.selection().map_or(l..=l, |(_, r)| r);
        let last = (*range.end()).min(block.lines.len().saturating_sub(1));
        let rows = &block.lines[(*range.start()).min(last)..=last];
        let addrs: Vec<String> = rows.iter().map(|(ea, _)| format!("{ea:X}")).collect();
        let lines: Vec<CopyLine> = rows
            .iter()
            .zip(&addrs)
            .map(|((_, line), a)| CopyLine {
                addr: Some(a),
                text: &line.text,
            })
            .collect();
        zui_editor::clipboard::write(lines_text(&lines, with_addresses), cx);
    }

    fn move_cursor(&mut self, delta: isize, cx: &mut Context<Self>) {
        self.sel.clear();
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
            .update(cx, |s, cx| s.set_cursor(ea, self.origin, cx));
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
        let insns: Vec<&(Ea, Line)> = b
            .full
            .as_ref()
            .unwrap_or(&b.lines)
            .iter()
            .filter(|(_, l)| l.is_instruction())
            .collect();
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
        let z = ((w - 48.) / gw).min((h - 48.) / gh).clamp(MIN_ZOOM, 1.);
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
                if v.drag_sel.is_some() {
                    // The element only hears moves over itself; a drag that leaves the
                    // canvas (to autoscroll) needs them everywhere.
                    let e = entity.clone();
                    window.on_mouse_event(move |ev: &MouseMoveEvent, phase, window, cx| {
                        if phase == gpui::DispatchPhase::Bubble {
                            e.update(cx, |v, cx| v.on_mouse_move(ev, window, cx));
                        }
                    });
                }
                let zoom = v.zoom;
                let pan = v.pan;
                let layout = v.layout.clone();
                let kinds = v.edge_kinds.clone();
                let marks = code::DebugMarks::read(v.session.read(cx));
                let cursor = v.cursor;
                let selected = v.selection();
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
                            // Without text the box is all there is to see: give it a body.
                            hsla(if zoom < TEXT_ZOOM {
                                theme.surface2
                            } else {
                                theme.surface
                            }),
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
                        if zoom >= TEXT_ZOOM {
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
                            let in_selection = selected
                                .as_ref()
                                .is_some_and(|(sb, r)| *sb == bi && r.contains(&li));
                            if in_selection && marked.is_none() {
                                window.paint_quad(gpui::fill(
                                    Bounds::new(
                                        point(origin.x + px(1.), y),
                                        size(rect.size.width - px(2.), px(line_h)),
                                    ),
                                    hsla(theme.selection),
                                ));
                            }
                            if marked.is_none() && !in_selection && cursor == Some((bi, li)) {
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
                            if zoom < TEXT_ZOOM {
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
                    // A narrow pane (the split) would put it over the code.
                    let minimap = if show_minimap && bounds.size.width >= px(MINIMAP_MIN_W) {
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
                    .id("edge-legend")
                    .size(px(24.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(ui::icon("info", 16., hsla(t.muted)))
                    .tooltip(ui::tip(
                        "Edges: green = taken, red = not taken, blue = unconditional",
                        None,
                    )),
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
            .on_action(cx.listener(|v, _: &ShowContextMenu, _, cx| {
                let pos = v.caret_position();
                v.open_context_menu(pos, true, cx)
            }))
            .on_action(cx.listener(|v, _: &Back, _, cx| v.on_back(cx)))
            .on_action(cx.listener(|v, _: &Copy, _, cx| v.copy(false, cx)))
            .on_action(cx.listener(|v, _: &CopyWithAddresses, _, cx| v.copy(true, cx)))
            .on_action(cx.listener(|v, _: &ExtendUp, _, cx| v.extend(-1, cx)))
            .on_action(cx.listener(|v, _: &ExtendDown, _, cx| v.extend(1, cx)))
            .on_action(cx.listener(|v, _: &ExtendPageUp, _, cx| v.extend(-20, cx)))
            .on_action(cx.listener(|v, _: &ExtendPageDown, _, cx| v.extend(20, cx)))
            .on_action(cx.listener(|v, _: &ExtendHome, _, cx| v.extend(-(1 << 20), cx)))
            .on_action(cx.listener(|v, _: &ExtendEnd, _, cx| v.extend(1 << 20, cx)))
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
                    .on_mouse_down(MouseButton::Right, cx.listener(Self::on_right_down))
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
