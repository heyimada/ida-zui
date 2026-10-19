// The proximity browser: the call neighbourhood of a function as a graph.
// Callers sit above the centre and callees below, so edges read top to
// bottom like the flow graph. A node's + / - buttons add or remove its
// callers and callees; a click recentres the view on a node and a double
// click jumps to it in the listing.
use crate::actions::*;
use crate::code::{self, char_width};
use crate::docs::DocEvent;
use crate::graph_layout::{self, Layout};
use crate::graph_paint::{self, MINIMAP_GAP};
use crate::scrollbar::{self, Axes};
use crate::session::{Session, SessionEvent};
use crate::theme::{Theme, code_font_name, hsla};
use crate::ui;
use gpui::{
    App, Bounds, Context, Entity, EventEmitter, FocusHandle, Focusable, MouseButton,
    MouseDownEvent, MouseMoveEvent, MouseUpEvent, Pixels, Point, ScrollWheelEvent, SharedString,
    Subscription, Task, Window, canvas, div, point, prelude::*, px, size,
};
use ida_proto::{Ea, Proximity, ProximityEdge, ProximityGraph, ProximityNode};
use std::time::Duration;

const PAD: f32 = 10.;
/// The smallest zoom that fit picks: names stay legible.
const READABLE: f32 = 0.3;
/// Nodes per row of a level, and per level before the rest folds into "+N more".
const PER_ROW: usize = 4;
const LEVEL_CAP: usize = 8;
/// Width of the colour strip at a node's left edge.
const STRIP: f32 = 4.;
const MAX_CHARS: usize = 34;
/// `Node::kind` of the "+N more" chip a crowded level ends with.
const MORE: &str = "more";
const HEADER_H: f32 = 32.;
const MAX_NODES: u32 = 150;
/// Neighbours per node and side in the first load, and when one is expanded.
const FANOUT: u32 = 12;
const EXPAND_FANOUT: u32 = 40;
/// Radius of the + / - buttons, in graph units.
const BTN_R: f32 = 8.;
/// A single click recentres after this long, unless a second click turns it
/// into a double click.
const RECENTRE_DELAY: Duration = Duration::from_millis(260);

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Dir {
    Callers,
    Callees,
}

/// One response merged into the view: the first load, or an expansion.
struct Part {
    anchor: Option<(Ea, Dir)>,
    graph: ProximityGraph,
}

struct Node {
    info: ProximityNode,
    w: f32,
}

/// What a level shows: the kept nodes, how many each crowded level folds away,
/// and the row (layer index) of every kept node and chip.
#[derive(Debug, PartialEq)]
struct Arrangement {
    keep: Vec<usize>,
    /// `(depth, hidden count)` per folded level.
    folded: Vec<(i32, usize)>,
    /// Layers of `keep`, then of one chip per entry of `folded`.
    layers: Vec<usize>,
}

/// Splits each level (`depths[i]`) into rows of `PER_ROW`; a level over
/// `LEVEL_CAP` that is not in `expanded` keeps `LEVEL_CAP - 1` nodes and a
/// chip. Rows are numbered top to bottom over all levels.
fn arrange(depths: &[i32], expanded: &[i32]) -> Arrangement {
    let mut levels: Vec<i32> = depths.to_vec();
    levels.sort_unstable();
    levels.dedup();
    let (mut keep, mut folded, mut layers, mut chip_layers) =
        (Vec::new(), Vec::new(), Vec::new(), Vec::new());
    let mut row0 = 0;
    for d in levels {
        let members: Vec<usize> = (0..depths.len()).filter(|&i| depths[i] == d).collect();
        let fold = d != 0 && members.len() > LEVEL_CAP && !expanded.contains(&d);
        let shown = if fold { LEVEL_CAP - 1 } else { members.len() };
        for (k, &i) in members.iter().take(shown).enumerate() {
            keep.push(i);
            layers.push(row0 + k / PER_ROW);
        }
        let mut slots = shown;
        if fold {
            folded.push((d, members.len() - shown));
            chip_layers.push(row0 + shown / PER_ROW);
            slots += 1;
        }
        row0 += slots.div_ceil(PER_ROW).max(1);
    }
    layers.extend(chip_layers);
    Arrangement {
        keep,
        folded,
        layers,
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Btn {
    Expand,
    Collapse,
}

pub struct ProximityView {
    session: Entity<Session>,
    centre: Ea,
    up: u32,
    down: u32,
    data: bool,
    parts: Vec<Part>,
    /// Everything the parts hold; `nodes` / `edges` are what is shown of it.
    all: Vec<ProximityNode>,
    all_edges: Vec<ProximityEdge>,
    /// Levels whose "+N more" chip was opened.
    expanded: Vec<i32>,
    nodes: Vec<Node>,
    edges: Vec<ProximityEdge>,
    layout: Layout,
    selected: Option<Ea>,
    pan: Point<Pixels>,
    zoom: f32,
    drag: Option<(Point<Pixels>, Point<Pixels>)>,
    bounds: Option<Bounds<Pixels>>,
    minimap: Option<(Bounds<Pixels>, f32)>,
    show_minimap: bool,
    char_w: f32,
    needs_fit: bool,
    generation: u64,
    loading: bool,
    error: Option<SharedString>,
    truncated: bool,
    recentre: Option<Task<()>>,
    focus: FocusHandle,
    _sub: Subscription,
}

impl EventEmitter<DocEvent> for ProximityView {}

impl ProximityView {
    pub fn new(session: Entity<Session>, centre: Ea, cx: &mut Context<Self>) -> Self {
        let sub = cx.subscribe(&session, |this, _, ev, cx| {
            if matches!(ev, SessionEvent::Changed) {
                this.load(cx);
            }
        });
        let mut me = Self {
            session,
            centre,
            up: 2,
            down: 2,
            data: false,
            parts: Vec::new(),
            all: Vec::new(),
            all_edges: Vec::new(),
            expanded: Vec::new(),
            nodes: Vec::new(),
            edges: Vec::new(),
            layout: Layout::default(),
            selected: None,
            pan: point(px(0.), px(0.)),
            zoom: 1.,
            drag: None,
            bounds: None,
            minimap: None,
            show_minimap: true,
            char_w: 7.8,
            needs_fit: true,
            generation: 0,
            loading: false,
            error: None,
            truncated: false,
            recentre: None,
            focus: cx.focus_handle(),
            _sub: sub,
        };
        me.load(cx);
        me
    }

    /// The database moved by `delta` (a debugger rebase): follow it.
    pub fn rebase(&mut self, delta: i64, cx: &mut Context<Self>) {
        self.centre = crate::debugger::shift(self.centre, delta);
        self.load(cx);
    }

    /// Fetches the neighbourhood of the centre afresh (expansions are dropped).
    fn load(&mut self, cx: &mut Context<Self>) {
        self.generation += 1;
        let generation = self.generation;
        self.loading = true;
        let fut = self.session.read(cx).call(Proximity {
            ea: self.centre,
            up: self.up,
            down: self.down,
            fanout: FANOUT,
            max_nodes: MAX_NODES,
            data: self.data,
        });
        cx.notify();
        cx.spawn(async move |this, cx| {
            let r = fut.await;
            this.update(cx, |v, cx| {
                if v.generation != generation {
                    return;
                }
                v.loading = false;
                match r {
                    Ok(g) => {
                        v.error = None;
                        v.centre = g.centre;
                        v.truncated = g.truncated;
                        if let Some(n) = g.nodes.iter().find(|n| n.ea == g.centre) {
                            cx.emit(DocEvent::Title(
                                format!("Proximity: {}", short(&n.name)).into(),
                            ));
                        }
                        v.parts = vec![Part {
                            anchor: None,
                            graph: g,
                        }];
                        v.rebuild();
                        v.needs_fit = true;
                    }
                    Err(e) => {
                        v.parts.clear();
                        v.rebuild();
                        v.error = Some(e.to_string().into());
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Adds the callers or callees of a node.
    fn expand(&mut self, ea: Ea, dir: Dir, cx: &mut Context<Self>) {
        let fut = self.session.read(cx).call(Proximity {
            ea,
            up: u32::from(dir == Dir::Callers),
            down: u32::from(dir == Dir::Callees),
            fanout: EXPAND_FANOUT,
            max_nodes: MAX_NODES,
            data: self.data,
        });
        let generation = self.generation;
        cx.spawn(async move |this, cx| {
            let r = fut.await;
            this.update(cx, |v, cx| {
                if v.generation != generation {
                    return;
                }
                match r {
                    Ok(graph) => {
                        let before: Vec<Ea> = v.nodes.iter().map(|n| n.info.ea).collect();
                        v.parts.push(Part {
                            anchor: Some((ea, dir)),
                            graph,
                        });
                        v.rebuild();
                        // The new nodes may lie off screen: bring them in.
                        let mut show: Vec<Ea> = v
                            .nodes
                            .iter()
                            .map(|n| n.info.ea)
                            .filter(|e| !before.contains(e))
                            .collect();
                        show.push(ea);
                        v.reveal(&show);
                    }
                    Err(e) => v
                        .session
                        .update(cx, |s, cx| s.error(format!("proximity: {e}"), cx)),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Removes what an expansion added (and expansions hanging off it).
    fn collapse(&mut self, ea: Ea, dir: Dir) {
        self.parts.retain(|p| p.anchor != Some((ea, dir)));
        self.rebuild();
    }

    /// Merges the parts into one node list and lays it out. A part's depths
    /// are relative to its anchor.
    fn rebuild(&mut self) {
        let mut nodes: Vec<ProximityNode> = Vec::new();
        let mut edges: Vec<ProximityEdge> = Vec::new();
        let mut i = 0;
        while i < self.parts.len() {
            let base = match self.parts[i].anchor {
                None => 0,
                Some((a, _)) => match nodes.iter().find(|n| n.ea == a) {
                    Some(n) => n.depth,
                    // The anchor is gone: so is this expansion.
                    None => {
                        self.parts.remove(i);
                        continue;
                    }
                },
            };
            for n in &self.parts[i].graph.nodes {
                if !nodes.iter().any(|m| m.ea == n.ea) {
                    let mut n = n.clone();
                    n.depth += base;
                    nodes.push(n);
                }
            }
            for e in &self.parts[i].graph.edges {
                if !edges.iter().any(|f| f.from == e.from && f.to == e.to) {
                    edges.push(*e);
                }
            }
            i += 1;
        }
        self.all = nodes;
        self.all_edges = edges;
        self.relayout();
    }

    /// Card height: two code lines and the padding.
    fn card_h() -> f32 {
        code::line_height() * 2. + 14.
    }

    /// Shows what the levels allow (rows, "+N more" chips) and lays it out.
    fn relayout(&mut self) {
        let depths: Vec<i32> = self.all.iter().map(|n| n.depth).collect();
        let arr = arrange(&depths, &self.expanded);
        let mut nodes: Vec<ProximityNode> = arr.keep.iter().map(|&i| self.all[i].clone()).collect();
        for (k, &(depth, hidden)) in arr.folded.iter().enumerate() {
            nodes.push(ProximityNode {
                ea: Ea::MAX - k as Ea,
                name: format!("+{hidden} more"),
                kind: MORE.into(),
                depth,
                ..Default::default()
            });
        }
        let cw = self.char_w;
        self.nodes = nodes.into_iter().map(|info| Node { w: 0., info }).collect();
        for n in &mut self.nodes {
            let name = short(&n.info.name).chars().count().min(MAX_CHARS) as f32;
            let sub = sub_line(&n.info).chars().count() as f32 * 0.85;
            n.w = (name.max(sub) * cw + PAD * 2. + STRIP).max(if n.info.kind == MORE {
                110.
            } else {
                170.
            });
        }
        let shown = |ea: Ea| self.nodes.iter().position(|n| n.info.ea == ea);
        self.edges = self
            .all_edges
            .iter()
            .filter(|e| shown(e.from).is_some() && shown(e.to).is_some())
            .copied()
            .collect();
        let sizes: Vec<(f32, f32)> = self.nodes.iter().map(|n| (n.w, Self::card_h())).collect();
        let at = |ea: Ea| self.nodes.iter().position(|n| n.info.ea == ea);
        let edges: Vec<(usize, usize)> = self
            .edges
            .iter()
            .filter_map(|e| Some((at(e.from)?, at(e.to)?)))
            .collect();
        self.layout = graph_layout::layout_in_layers(&sizes, &edges, &arr.layers);
    }

    fn index_of(&self, ea: Ea) -> Option<usize> {
        self.nodes.iter().position(|n| n.info.ea == ea)
    }

    /// The + / - button a node shows on a side, if any.
    fn button(&self, i: usize, dir: Dir) -> Option<Btn> {
        let n = &self.nodes[i].info;
        if self.parts.iter().any(|p| p.anchor == Some((n.ea, dir))) {
            return Some(Btn::Collapse);
        }
        let shown = self
            .edges
            .iter()
            .filter(|e| match dir {
                Dir::Callers => e.to == n.ea,
                Dir::Callees => e.from == n.ea,
            })
            .count() as u64;
        let total = if dir == Dir::Callers {
            n.callers
        } else {
            n.callees
        };
        (total > shown).then_some(Btn::Expand)
    }

    /// Graph position of a node's button: top and bottom edge, right side.
    fn button_at(&self, i: usize, dir: Dir) -> (f32, f32) {
        let r = self.layout.nodes[i];
        (
            r.right() - 18.,
            if dir == Dir::Callers { r.y } else { r.bottom() },
        )
    }

    fn to_graph(&self, p: Point<Pixels>) -> Option<(f32, f32)> {
        let b = self.bounds?;
        Some((
            f32::from(p.x - b.left() - self.pan.x) / self.zoom,
            f32::from(p.y - b.top() - self.pan.y) / self.zoom,
        ))
    }

    fn hit_button(&self, gx: f32, gy: f32) -> Option<(usize, Dir, Btn)> {
        for i in 0..self.nodes.len() {
            for dir in [Dir::Callers, Dir::Callees] {
                let Some(btn) = self.button(i, dir) else {
                    continue;
                };
                let (x, y) = self.button_at(i, dir);
                if (gx - x).hypot(gy - y) <= BTN_R + 2. {
                    return Some((i, dir, btn));
                }
            }
        }
        None
    }

    fn hit_node(&self, gx: f32, gy: f32) -> Option<usize> {
        self.layout
            .nodes
            .iter()
            .position(|r| gx >= r.x && gx <= r.right() && gy >= r.y && gy <= r.bottom())
    }

    fn on_mouse_down(&mut self, ev: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.focus, cx);
        if self.minimap_click(ev.position) {
            cx.notify();
            return;
        }
        let Some((gx, gy)) = self.to_graph(ev.position) else {
            return;
        };
        if let Some((i, dir, btn)) = self.hit_button(gx, gy) {
            let ea = self.nodes[i].info.ea;
            match btn {
                Btn::Expand => self.expand(ea, dir, cx),
                Btn::Collapse => self.collapse(ea, dir),
            }
            cx.notify();
            return;
        }
        match self.hit_node(gx, gy) {
            Some(i) if self.nodes[i].info.kind == MORE => {
                let depth = self.nodes[i].info.depth;
                let before: Vec<Ea> = self.nodes.iter().map(|n| n.info.ea).collect();
                self.expanded.push(depth);
                self.relayout();
                let mut show: Vec<Ea> = self
                    .nodes
                    .iter()
                    .map(|n| n.info.ea)
                    .filter(|e| !before.contains(e))
                    .collect();
                show.push(self.centre);
                self.reveal(&show);
            }
            Some(i) => {
                let ea = self.nodes[i].info.ea;
                self.selected = Some(ea);
                if ev.click_count >= 2 {
                    self.recentre = None;
                    cx.emit(DocEvent::Navigate(ea));
                } else if ea != self.centre {
                    self.recentre = Some(cx.spawn(async move |this, cx| {
                        cx.background_executor().timer(RECENTRE_DELAY).await;
                        this.update(cx, |v, cx| {
                            v.recentre = None;
                            v.centre = ea;
                            v.load(cx);
                        })
                        .ok();
                    }));
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
        let delta = ev.delta.pixel_delta(px(code::line_height()));
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
            let gx = f32::from(a.x - self.pan.x) / self.zoom;
            let gy = f32::from(a.y - self.pan.y) / self.zoom;
            self.pan = point(a.x - px(gx * new), a.y - px(gy * new));
        }
        self.zoom = new;
    }

    /// Shows the whole graph when it stays readable, else the centre node at
    /// a readable size (pan and zoom out to see more).
    fn fit(&mut self) {
        let Some(bounds) = self.bounds else { return };
        let (gw, gh) = (self.layout.width.max(1.), self.layout.height.max(1.));
        let (w, h) = (f32::from(bounds.size.width), f32::from(bounds.size.height));
        let whole = ((w - 48.) / gw).min((h - 48.) / gh).min(1.);
        if whole >= READABLE {
            self.zoom = whole;
            self.pan = point(
                px((w - gw * whole) / 2.),
                px(((h - gh * whole) / 2.).max(16.)),
            );
        } else {
            self.zoom = READABLE;
            let r = self
                .index_of(self.centre)
                .and_then(|i| self.layout.nodes.get(i).copied());
            let (cx, cy) = r.map_or((gw / 2., gh / 2.), |r| (r.x + r.w / 2., r.y + r.h / 2.));
            self.pan = point(px(w / 2. - cx * READABLE), px(h / 2. - cy * READABLE));
        }
    }

    /// Pans (and zooms out, down to the readable limit) so the nodes `eas`
    /// are all in view; does nothing when they already are.
    fn reveal(&mut self, eas: &[Ea]) {
        let Some(bounds) = self.bounds else { return };
        let rects: Vec<_> = eas
            .iter()
            .filter_map(|e| self.index_of(*e))
            .filter_map(|i| self.layout.nodes.get(i).copied())
            .collect();
        if rects.is_empty() {
            return;
        }
        let x0 = rects.iter().map(|r| r.x).fold(f32::MAX, f32::min);
        let y0 = rects.iter().map(|r| r.y).fold(f32::MAX, f32::min);
        let x1 = rects.iter().map(|r| r.x + r.w).fold(f32::MIN, f32::max);
        let y1 = rects.iter().map(|r| r.y + r.h).fold(f32::MIN, f32::max);
        let (w, h) = (f32::from(bounds.size.width), f32::from(bounds.size.height));
        let (px0, py0) = (f32::from(self.pan.x), f32::from(self.pan.y));
        let z = self.zoom;
        let inside =
            px0 + x0 * z >= 0. && py0 + y0 * z >= 0. && px0 + x1 * z <= w && py0 + y1 * z <= h;
        if inside {
            return;
        }
        let (bw, bh) = ((x1 - x0).max(1.), (y1 - y0).max(1.));
        let z = z.min((w - 48.) / bw).min((h - 48.) / bh).max(READABLE);
        self.zoom = z;
        self.pan = point(
            px((w - bw * z) / 2. - x0 * z),
            px(((h - bh * z) / 2.).max(16.) - y0 * z),
        );
    }

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
                let cw = f32::from(char_width(window));
                // Bring the state up to date before drawing it: a fit applied
                // after the paint would only show with the next frame.
                entity.update(cx, |v, cx| {
                    v.bounds = Some(bounds);
                    if (v.char_w - cw).abs() > 0.01 {
                        v.char_w = cw;
                        v.relayout();
                    }
                    if v.needs_fit && !v.loading && !v.layout.nodes.is_empty() {
                        v.needs_fit = false;
                        v.fit();
                        // One-shot: the header's zoom label was drawn before the fit.
                        cx.notify();
                    }
                });
                let v = entity.read(cx);
                let (zoom, pan) = (v.zoom, v.pan);
                let layout = v.layout.clone();
                let centre = v.centre;
                let selected = v.selected;
                let show_minimap = v.show_minimap;
                let infos: Vec<(ProximityNode, [Option<Btn>; 2])> = (0..v.nodes.len())
                    .map(|i| {
                        (
                            v.nodes[i].info.clone(),
                            [v.button(i, Dir::Callers), v.button(i, Dir::Callees)],
                        )
                    })
                    .collect();
                let edge_eas: Vec<(Ea, Ea)> = v
                    .edges
                    .iter()
                    .filter_map(|e| {
                        Some((
                            v.index_of(e.from).map(|_| e.from)?,
                            v.index_of(e.to).map(|_| e.to)?,
                        ))
                    })
                    .collect();
                let to_screen = |x: f32, y: f32| {
                    point(
                        bounds.left() + pan.x + px(x * zoom),
                        bounds.top() + pan.y + px(y * zoom),
                    )
                };
                window.with_content_mask(Some(gpui::ContentMask { bounds }), |window| {
                    for (k, e) in layout.edges.iter().enumerate() {
                        let (from, to) = edge_eas.get(k).copied().unwrap_or((0, 0));
                        let hot = from == centre
                            || to == centre
                            || Some(from) == selected
                            || Some(to) == selected;
                        let color = if hot {
                            hsla(theme.accent)
                        } else {
                            hsla(theme.muted).opacity(0.6)
                        };
                        graph_paint::edge(window, &e.points, color, zoom, &to_screen);
                    }
                    for (i, r) in layout.nodes.iter().enumerate() {
                        let Some((n, btns)) = infos.get(i) else {
                            continue;
                        };
                        let origin = to_screen(r.x, r.y);
                        let rect = Bounds::new(origin, size(px(r.w * zoom), px(r.h * zoom)));
                        if !rect.intersects(&bounds) {
                            continue;
                        }
                        let is_more = n.kind == MORE;
                        let is_centre = n.ea == centre;
                        let is_sel = Some(n.ea) == selected;
                        window.paint_quad(gpui::quad(
                            rect,
                            px(6. * zoom),
                            hsla(if is_centre {
                                theme.accent_soft
                            } else if is_more {
                                theme.surface2
                            } else {
                                theme.surface
                            }),
                            px(if is_centre || is_sel { 1.5 } else { 1. }),
                            hsla(if is_centre || is_sel {
                                theme.accent
                            } else {
                                theme.border_strong
                            }),
                            gpui::BorderStyle::Solid,
                        ));
                        let strip = match (n.kind.as_str(), n.library) {
                            (_, true) => theme.muted,
                            ("import", _) => theme.syntax.imp_name,
                            ("data", _) => theme.syntax.data_name,
                            _ => theme.syntax.code_name,
                        };
                        if !is_more {
                            window.paint_quad(gpui::quad(
                                Bounds::new(
                                    point(origin.x + px(1.), origin.y + px(6. * zoom)),
                                    size(px(STRIP * zoom), px((r.h - 12.) * zoom)),
                                ),
                                px(2. * zoom),
                                hsla(strip),
                                px(0.),
                                gpui::transparent_black(),
                                gpui::BorderStyle::Solid,
                            ));
                        }
                        if zoom >= 0.3 {
                            let x = origin.x + px((PAD + STRIP) * zoom);
                            let (fs, lh) = (code::font_size(), code::line_height());
                            let name = shape_name(
                                window,
                                &short(&n.name),
                                hsla(if is_more { theme.accent } else { theme.text }),
                                px(fs * zoom),
                            );
                            let ny = if is_more { (r.h - lh) / 2. } else { 5. };
                            let _ = name.paint(
                                point(x, origin.y + px(ny * zoom)),
                                px(lh * zoom),
                                gpui::TextAlign::Left,
                                None,
                                window,
                                cx,
                            );
                            if !is_more && zoom >= 0.45 {
                                let sub = sub_line(n);
                                let note = code::shape_plain_sized(
                                    window,
                                    &sub,
                                    hsla(theme.muted),
                                    px(fs * 0.85 * zoom),
                                );
                                let _ = note.paint(
                                    point(x, origin.y + px((5. + lh) * zoom)),
                                    px(lh * zoom),
                                    gpui::TextAlign::Left,
                                    None,
                                    window,
                                    cx,
                                );
                            }
                        }
                        for (b, dir) in [(btns[0], Dir::Callers), (btns[1], Dir::Callees)] {
                            let Some(b) = b else { continue };
                            let by = if dir == Dir::Callers { r.y } else { r.bottom() };
                            let c = to_screen(r.right() - 18., by);
                            let rr = px(BTN_R * zoom);
                            window.paint_quad(gpui::quad(
                                Bounds::new(point(c.x - rr, c.y - rr), size(rr * 2., rr * 2.)),
                                rr,
                                hsla(theme.surface2),
                                px(1.),
                                hsla(theme.accent),
                                gpui::BorderStyle::Solid,
                            ));
                            if zoom >= 0.5 {
                                let glyph = code::shape_plain_sized(
                                    window,
                                    if b == Btn::Expand { "+" } else { "−" },
                                    hsla(theme.accent),
                                    px(12. * zoom),
                                );
                                let _ = glyph.paint(
                                    point(c.x - glyph.width / 2., c.y - px(7.5 * zoom)),
                                    px(15. * zoom),
                                    gpui::TextAlign::Left,
                                    None,
                                    window,
                                    cx,
                                );
                            }
                        }
                    }
                    let minimap = if show_minimap {
                        graph_paint::minimap(window, bounds, &theme, &layout, pan, zoom)
                    } else {
                        None
                    };
                    entity.update(cx, |v, _| v.minimap = minimap);
                });
            },
        )
        .size_full()
    }

    fn render_header(&self, theme: &Theme, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let t = *theme;
        let stepper = |label: &'static str,
                       id: &'static str,
                       value: u32,
                       dir: Dir,
                       cx: &mut Context<Self>| {
            div()
                .flex()
                .items_center()
                .gap(px(2.))
                .child(
                    div()
                        .mr(px(4.))
                        .text_size(px(11.))
                        .text_color(hsla(t.muted))
                        .child(label),
                )
                .child(
                    ui::icon_button((id, 0usize), "minus", 24., false, value > 1, &t).on_click(
                        cx.listener(move |v, _, _, cx| {
                            let n = if dir == Dir::Callers {
                                &mut v.up
                            } else {
                                &mut v.down
                            };
                            if *n > 1 {
                                *n -= 1;
                                v.load(cx);
                            }
                        }),
                    ),
                )
                .child(
                    div()
                        .w(px(18.))
                        .flex()
                        .justify_center()
                        .text_size(px(11.))
                        .font_family(code_font_name())
                        .child(value.to_string()),
                )
                .child(
                    ui::icon_button((id, 1usize), "plus", 24., false, value < 6, &t).on_click(
                        cx.listener(move |v, _, _, cx| {
                            let n = if dir == Dir::Callers {
                                &mut v.up
                            } else {
                                &mut v.down
                            };
                            if *n < 6 {
                                *n += 1;
                                v.load(cx);
                            }
                        }),
                    ),
                )
        };
        let swatch = |color: u32, label: &'static str| {
            div()
                .flex()
                .items_center()
                .gap(px(6.))
                .child(div().w(px(4.)).h(px(12.)).rounded(px(2.)).bg(hsla(color)))
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
                ui::icon_button("prox-zoom-out", "minus", 24., false, true, &t)
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
                    .font_family(code_font_name())
                    .child(format!("{:.0}%", self.zoom * 100.)),
            )
            .child(
                ui::icon_button("prox-zoom-in", "plus", 24., false, true, &t)
                    .tooltip(ui::tip("Zoom in", None))
                    .on_click(cx.listener(|v, _, _, cx| {
                        v.zoom_at(1.2, None);
                        cx.notify();
                    })),
            )
            .child(
                ui::icon_button("prox-fit", "expand", 24., false, true, &t)
                    .tooltip(ui::tip("Fit", None))
                    .on_click(cx.listener(|v, _, _, cx| {
                        v.fit();
                        cx.notify();
                    })),
            )
            .child(div().w(px(16.)))
            .child(stepper("Callers", "prox-up", self.up, Dir::Callers, cx))
            .child(div().w(px(12.)))
            .child(stepper("Callees", "prox-down", self.down, Dir::Callees, cx))
            .child(div().w(px(12.)))
            .child(
                ui::button(
                    "prox-data",
                    "Data",
                    if self.data {
                        ui::ButtonKind::Primary
                    } else {
                        ui::ButtonKind::Secondary
                    },
                    true,
                    &t,
                )
                .tooltip(ui::tip("Include referenced data", None))
                .on_click(cx.listener(|v, _, _, cx| {
                    v.data = !v.data;
                    v.load(cx);
                })),
            )
            .child(div().flex_1().min_w(px(24.)))
            .child(
                div()
                    .flex()
                    .flex_none()
                    .items_center()
                    .gap(px(16.))
                    .text_size(px(11.))
                    .text_color(hsla(t.muted))
                    .child(swatch(t.syntax.code_name, "Function"))
                    .child(swatch(t.muted, "Library"))
                    .child(swatch(t.syntax.imp_name, "Import"))
                    .child(swatch(t.syntax.data_name, "Data")),
            )
            .child(div().flex_1())
            .child(
                ui::icon_button("prox-minimap", "view", 24., self.show_minimap, true, &t)
                    .tooltip(ui::tip("Minimap", None))
                    .on_click(cx.listener(|v, _, _, cx| {
                        v.show_minimap = !v.show_minimap;
                        if !v.show_minimap {
                            v.minimap = None;
                        }
                        cx.notify();
                    })),
            )
    }
}

fn sub_line(n: &ProximityNode) -> String {
    format!(
        "{:#x}  ·  {} callers  ·  {} callees",
        n.ea, n.callers, n.callees
    )
}

/// A function name without its argument list, cut to the card width.
fn short(name: &str) -> String {
    let n = if name.ends_with(')') {
        let mut depth = 0;
        let mut cut = None;
        for (i, c) in name.char_indices().rev() {
            match c {
                ')' => depth += 1,
                '(' => {
                    depth -= 1;
                    if depth == 0 {
                        cut = Some(i);
                        break;
                    }
                }
                _ => {}
            }
        }
        cut.filter(|&i| i > 0).map_or(name, |i| &name[..i])
    } else {
        name
    };
    if n.chars().count() > MAX_CHARS {
        format!("{}…", n.chars().take(MAX_CHARS - 1).collect::<String>())
    } else {
        n.to_owned()
    }
}

fn shape_name(
    window: &mut Window,
    text: &str,
    color: gpui::Hsla,
    size: Pixels,
) -> gpui::ShapedLine {
    code::shape_plain_sized(window, text, color, size)
}

impl Focusable for ProximityView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for ProximityView {
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
        if self.needs_fit && !self.loading && !self.layout.nodes.is_empty() && self.bounds.is_some()
        {
            self.needs_fit = false;
            self.fit();
        }
        let header = self.render_header(&theme, cx);
        let note = if self.loading {
            Some(SharedString::from("Reading the call graph…"))
        } else if let Some(e) = self.error.clone() {
            Some(e)
        } else if self.nodes.len() == 1 {
            Some(SharedString::from(
                "This function has no callers or callees in the database.",
            ))
        } else {
            None
        };
        div()
            .id("proximity")
            .key_context("CodeView")
            .track_focus(&self.focus)
            .size_full()
            .flex()
            .flex_col()
            .bg(hsla(theme.surface))
            .on_action(cx.listener(|v, _: &ZoomIn, _, cx| {
                v.zoom_at(1.2, None);
                cx.notify();
            }))
            .on_action(cx.listener(|v, _: &ZoomOut, _, cx| {
                v.zoom_at(1. / 1.2, None);
                cx.notify();
            }))
            .on_action(cx.listener(|v, _: &ZoomReset, _, cx| {
                v.fit();
                cx.notify();
            }))
            .child(header)
            .child(
                div()
                    .id("proximity-canvas")
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
                    .when_some(note, |d, e| d.child(div().absolute().top(px(16.)).left(px(16.)).text_color(hsla(theme.muted)).child(e)))
                    .when(self.truncated, |d| {
                        d.child(
                            div()
                                .absolute()
                                .bottom(px(MINIMAP_GAP))
                                .left(px(16.))
                                .text_size(px(11.))
                                .text_color(hsla(theme.warning))
                                .child(format!("Showing the first {MAX_NODES} functions; lower the levels to see the rest.")),
                        )
                    }),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Failure modes: a level of exactly the cap must not fold, a folded level
    // loses nothing silently (kept + hidden = all), the centre is never
    // hidden, rows never exceed PER_ROW, levels stay in depth order, and an
    // opened level shows everything.
    #[test]
    fn small_levels_show_everything_in_rows() {
        let depths = [0, 1, 1, 1, 1, 1, 1, 1];
        let a = arrange(&depths, &[]);
        assert_eq!(a.keep.len(), 8);
        assert!(a.folded.is_empty());
        assert_eq!(a.layers, vec![0, 1, 1, 1, 1, 2, 2, 2]);
    }

    #[test]
    fn a_crowded_level_keeps_the_cap_and_counts_the_rest() {
        let mut depths = vec![0];
        depths.extend(std::iter::repeat_n(1, 40));
        let a = arrange(&depths, &[]);
        assert_eq!(a.keep.len() + a.folded[0].1, 41);
        assert_eq!(
            a.keep.len(),
            1 + LEVEL_CAP - 1,
            "the centre plus the cap minus the chip slot"
        );
        assert_eq!(a.folded[0].0, 1);
        assert_eq!(
            a.layers.len(),
            a.keep.len() + 1,
            "one layer per kept node and the chip"
        );
        let mut per_row = std::collections::HashMap::new();
        for l in &a.layers {
            *per_row.entry(*l).or_insert(0) += 1;
        }
        assert!(per_row.values().all(|&c| c <= PER_ROW), "{per_row:?}");
        assert!(a.keep.contains(&0), "the centre stays");
        let at_cap = arrange(&[1; LEVEL_CAP], &[]);
        assert!(
            at_cap.folded.is_empty(),
            "a level of exactly the cap is shown whole"
        );
    }

    #[test]
    fn levels_keep_their_order_and_opened_levels_show_all() {
        let mut depths = vec![-1, -1, 0];
        depths.extend(std::iter::repeat_n(1, 30));
        depths.push(2);
        let a = arrange(&depths, &[1]);
        assert!(a.folded.is_empty());
        let layer_of = |depth: i32| {
            a.keep
                .iter()
                .zip(&a.layers)
                .filter(|(i, _)| depths[**i] == depth)
                .map(|(_, l)| *l)
                .collect::<Vec<_>>()
        };
        assert!(layer_of(-1).iter().max() < layer_of(0).iter().min());
        assert!(layer_of(0).iter().max() < layer_of(1).iter().min());
        assert!(layer_of(1).iter().max() < layer_of(2).iter().min());
        assert_eq!(
            layer_of(1).iter().max().unwrap() - layer_of(1).iter().min().unwrap(),
            29 / PER_ROW
        );
    }
}
