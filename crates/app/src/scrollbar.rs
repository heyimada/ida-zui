// The overlay scrollbar: a 10 px track that
// is transparent, a thumb that appears while the pointer is over the surface,
// while it scrolls and while it is dragged, `raised` at rest and `borderStrong`
// under the pointer, 2 px inset, at least 24 px long. A click on the track
// pages towards the pointer, dragging the thumb scrolls.
//
// It is one painted layer on top of a surface (the surface's parent must be
// `relative`), reading its position through a `Source`: gpui's own scroll
// handles for elements that use `overflow_*_scroll` or `uniform_list`, or a
// pair of closures for surfaces that draw and page themselves (the listing,
// the hex view, the graph canvases).
use crate::theme::{Theme, hsla};
use gpui::{
    App, BorderStyle, Bounds, Corners, DispatchPhase, ElementId, HitboxBehavior, IntoElement,
    MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, Pixels, ScrollHandle,
    UniformListScrollHandle, Window, canvas, point, prelude::*, px, quad, size,
};
use std::cell::RefCell;
use std::rc::Rc;
use std::time::{Duration, Instant};

pub const TRACK: f32 = 10.;
const INSET: f32 = 2.;
const MIN_THUMB: f32 = 24.;
/// How long the thumb stays after the surface last moved.
const LINGER: Duration = Duration::from_millis(900);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Axis {
    Vertical,
    Horizontal,
}

/// Which bars a surface wants.
#[derive(Clone, Copy)]
pub struct Axes {
    pub vertical: bool,
    pub horizontal: bool,
}

impl Axes {
    pub const V: Axes = Axes {
        vertical: true,
        horizontal: false,
    };
    pub const BOTH: Axes = Axes {
        vertical: true,
        horizontal: true,
    };
}

/// Where a surface is along one axis, in any unit as long as the three agree
/// (pixels, rows or bytes). `offset` is the distance of the viewport's start
/// from the content's start.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Metrics {
    pub viewport: f32,
    pub content: f32,
    pub offset: f32,
}

pub trait ScrollApi {
    /// `None` when the surface has nothing to scroll along `axis`.
    fn metrics(&self, axis: Axis, cx: &App) -> Option<Metrics>;
    fn set_offset(&self, axis: Axis, offset: f32, cx: &mut App);
}

#[derive(Clone)]
pub struct Source(Rc<dyn ScrollApi>);

struct HandleSource(ScrollHandle);

impl ScrollApi for HandleSource {
    fn metrics(&self, axis: Axis, _: &App) -> Option<Metrics> {
        let (b, max, off) = (self.0.bounds(), self.0.max_offset(), self.0.offset());
        let (viewport, max, off) = match axis {
            Axis::Vertical => (b.size.height, max.y, off.y),
            Axis::Horizontal => (b.size.width, max.x, off.x),
        };
        let (viewport, max, off) = (f32::from(viewport), f32::from(max), f32::from(off));
        (viewport > 0.).then_some(Metrics {
            viewport,
            content: viewport + max,
            offset: -off,
        })
    }

    fn set_offset(&self, axis: Axis, offset: f32, _: &mut App) {
        let cur = self.0.offset();
        self.0.set_offset(match axis {
            Axis::Vertical => point(cur.x, px(-offset)),
            Axis::Horizontal => point(px(-offset), cur.y),
        });
    }
}

struct Custom {
    metrics: Box<dyn Fn(Axis, &App) -> Option<Metrics>>,
    set: Box<dyn Fn(Axis, f32, &mut App)>,
}

impl ScrollApi for Custom {
    fn metrics(&self, axis: Axis, cx: &App) -> Option<Metrics> {
        (self.metrics)(axis, cx)
    }

    fn set_offset(&self, axis: Axis, offset: f32, cx: &mut App) {
        (self.set)(axis, offset, cx)
    }
}

impl Source {
    /// An element with `overflow_*_scroll` tracked by `handle`.
    pub fn handle(handle: &ScrollHandle) -> Self {
        Self(Rc::new(HandleSource(handle.clone())))
    }

    /// A `uniform_list` tracked by `handle`.
    pub fn list(handle: &UniformListScrollHandle) -> Self {
        Self(Rc::new(HandleSource(handle.0.borrow().base_handle.clone())))
    }

    /// A surface that pages itself. `metrics` reports its position, `set`
    /// moves it (called while the thumb is dragged or the track clicked).
    pub fn custom(
        metrics: impl Fn(Axis, &App) -> Option<Metrics> + 'static,
        set: impl Fn(Axis, f32, &mut App) + 'static,
    ) -> Self {
        Self(Rc::new(Custom {
            metrics: Box::new(metrics),
            set: Box::new(set),
        }))
    }
}

/// The thumb of a track `track` long: `(start, length)` along the track, or
/// `None` when everything fits. The length follows the viewport's share of the
/// content but never drops under the minimum or exceeds the track.
pub fn thumb(track: f32, m: Metrics) -> Option<(f32, f32)> {
    if track <= 0. || m.viewport <= 0. || m.content <= m.viewport + 0.5 {
        return None;
    }
    let len = (track * m.viewport / m.content)
        .max(MIN_THUMB.min(track))
        .min(track);
    let max_offset = m.content - m.viewport;
    let start = (track - len) * (m.offset / max_offset).clamp(0., 1.);
    Some((start, len))
}

/// The content offset that puts a thumb of `len` at `start` on a track.
pub fn offset_at(track: f32, len: f32, start: f32, m: Metrics) -> f32 {
    let room = track - len;
    if room <= 0. {
        return 0.;
    }
    (start / room).clamp(0., 1.) * (m.content - m.viewport)
}

/// A click on the track outside the thumb moves one page towards it.
pub fn page_target(click: f32, thumb_start: f32, m: Metrics) -> f32 {
    let page = (m.viewport * 0.9).max(1.);
    let max = (m.content - m.viewport).max(0.);
    let towards = if click < thumb_start {
        m.offset - page
    } else {
        m.offset + page
    };
    towards.clamp(0., max)
}

/// A scroll handle that lives as long as the element with this id keeps
/// rendering, for surfaces that do not keep one in a field.
pub fn handle(id: &'static str, window: &mut Window, cx: &mut App) -> ScrollHandle {
    window
        .use_keyed_state(id, cx, |_, _| ScrollHandle::new())
        .read(cx)
        .clone()
}

/// Where a pannable canvas (the graph views) is, in pixels.
pub struct PanState {
    pub pan: (f32, f32),
    pub zoom: f32,
    /// The laid-out graph, in graph units.
    pub graph: (f32, f32),
    pub viewport: (f32, f32),
}

/// Room around the graph that panning may show.
const PAN_MARGIN: f32 = 24.;

/// The metrics of a canvas against its graph bounds: a graph that fits the
/// viewport with the margin has nothing to scroll. `offset` is the viewport
/// start in content pixels, where the content is the graph plus a margin.
pub fn pan_metrics(s: &PanState, axis: Axis) -> Option<Metrics> {
    let (pan, g, vp) = match axis {
        Axis::Vertical => (s.pan.1, s.graph.1, s.viewport.1),
        Axis::Horizontal => (s.pan.0, s.graph.0, s.viewport.0),
    };
    (vp > 0. && g > 0.).then_some(Metrics {
        viewport: vp,
        content: g * s.zoom + 2. * PAN_MARGIN,
        offset: PAN_MARGIN - pan,
    })
}

/// The pan that puts the viewport at `offset` (the inverse of `pan_metrics`).
pub fn pan_for_offset(offset: f32) -> f32 {
    PAN_MARGIN - offset
}

/// A source for a view that keeps `pan`, `zoom`, a layout and its canvas
/// bounds. `state` reads them; `set` stores a new pan along one axis.
pub fn pan_source<V: 'static>(
    view: gpui::WeakEntity<V>,
    state: impl Fn(&V) -> Option<PanState> + 'static,
    set: impl Fn(&mut V, Axis, f32, &mut gpui::Context<V>) + 'static,
) -> Source {
    let reader = view.clone();
    Source::custom(
        move |axis, cx| pan_metrics(&state(reader.upgrade()?.read(cx))?, axis),
        move |axis, offset, cx| {
            if let Some(v) = view.upgrade() {
                v.update(cx, |v, cx| set(v, axis, pan_for_offset(offset), cx));
            }
        },
    )
}

/// A source for a `zui_editor::Editor`, which scrolls itself in pixels. Turn
/// off the editor's own thin bars (`set_own_scrollbars(false)`) when using it.
pub fn editor_source(editor: &gpui::Entity<zui_editor::Editor>) -> Source {
    let (reader, writer) = (editor.downgrade(), editor.downgrade());
    Source::custom(
        move |axis, cx| {
            let m = reader.upgrade()?.read(cx).scroll_metrics()?;
            let (viewport, content, offset) = m[idx(axis)];
            (viewport > 0.).then_some(Metrics {
                viewport,
                content,
                offset,
            })
        },
        move |axis, offset, cx| {
            if let Some(e) = writer.upgrade() {
                e.update(cx, |e, cx| {
                    e.scroll_to_offset(axis == Axis::Horizontal, offset, cx)
                });
            }
        },
    )
}

/// A list that fills the space left in a column, with its scrollbar on top.
pub fn area(content: impl IntoElement, bar: impl IntoElement) -> gpui::Div {
    gpui::div()
        .relative()
        .flex_1()
        .min_h_0()
        .flex()
        .flex_col()
        .child(content)
        .child(bar)
}

#[derive(Default)]
struct Inner {
    last: [Option<f32>; 2],
    moved_at: Option<Instant>,
    /// Axis being dragged and where in the thumb it was grabbed.
    drag: Option<(Axis, f32)>,
    /// The pointer is over the surface / over this axis's track.
    over: bool,
    over_track: [bool; 2],
}

struct State(Rc<RefCell<Inner>>);

fn idx(axis: Axis) -> usize {
    match axis {
        Axis::Vertical => 0,
        Axis::Horizontal => 1,
    }
}

struct Layout {
    axis: Axis,
    track: Bounds<Pixels>,
    thumb: Bounds<Pixels>,
    len: f32,
    start: f32,
    track_len: f32,
    m: Metrics,
}

fn layout(bounds: Bounds<Pixels>, axis: Axis, m: Metrics, corner: bool) -> Option<Layout> {
    let reserve = if corner { TRACK } else { 0. };
    let (x, y, w, h) = (
        bounds.left(),
        bounds.top(),
        bounds.size.width,
        bounds.size.height,
    );
    let (track, track_len) = match axis {
        Axis::Vertical => {
            let len = f32::from(h) - reserve;
            (
                Bounds::new(
                    point(x + w - px(TRACK), y),
                    size(px(TRACK), px(len.max(0.))),
                ),
                len,
            )
        }
        Axis::Horizontal => {
            let len = f32::from(w) - reserve;
            (
                Bounds::new(
                    point(x, y + h - px(TRACK)),
                    size(px(len.max(0.)), px(TRACK)),
                ),
                len,
            )
        }
    };
    let inner = track_len - 2. * INSET;
    let (start, len) = thumb(inner, m)?;
    let thumb = match axis {
        Axis::Vertical => Bounds::new(
            point(track.left() + px(INSET), track.top() + px(INSET + start)),
            size(px(TRACK - 2. * INSET), px(len)),
        ),
        Axis::Horizontal => Bounds::new(
            point(track.left() + px(INSET + start), track.top() + px(INSET)),
            size(px(len), px(TRACK - 2. * INSET)),
        ),
    };
    Some(Layout {
        axis,
        track,
        thumb,
        len,
        start,
        track_len: inner,
        m,
    })
}

/// The scrollbar layer for a surface. Put it as the last child of a
/// `relative` parent that does not itself scroll. `id` must be unique among
/// the scrollbars of one view.
pub fn scrollbar<I: Into<ElementId>>(
    id: I,
    source: Source,
    axes: Axes,
    theme: &Theme,
    window: &mut Window,
    cx: &mut App,
) -> impl IntoElement + use<I> {
    let state = window
        .use_keyed_state(id, cx, |_, _| State(Rc::default()))
        .read(cx)
        .0
        .clone();
    let (rest, hover) = (hsla(theme.raised), hsla(theme.border_strong));
    canvas(
        |bounds, window, _| window.insert_hitbox(bounds, HitboxBehavior::Normal),
        move |bounds, hitbox, window, cx| {
            let both = axes.vertical && axes.horizontal;
            let layouts: Vec<Layout> = [
                (Axis::Vertical, axes.vertical),
                (Axis::Horizontal, axes.horizontal),
            ]
            .into_iter()
            .filter(|(_, on)| *on)
            .filter_map(|(axis, _)| layout(bounds, axis, source.0.metrics(axis, cx)?, both))
            .collect();
            let now = Instant::now();
            let visible = {
                let mut s = state.borrow_mut();
                // A position that changed since the last frame is a scroll.
                for axis in [Axis::Vertical, Axis::Horizontal] {
                    let at = source.0.metrics(axis, cx).map(|m| m.offset);
                    if s.last[idx(axis)]
                        .zip(at)
                        .is_some_and(|(a, b)| (a - b).abs() > 0.01)
                    {
                        s.moved_at = Some(now);
                    }
                    s.last[idx(axis)] = at;
                }
                let recent = s.moved_at.is_some_and(|t| now.duration_since(t) < LINGER);
                if recent {
                    window.request_animation_frame();
                }
                s.drag.is_some() || hitbox.is_hovered(window) || recent
            };
            if !layouts.is_empty() && visible {
                let dragging = state.borrow().drag.map(|(a, _)| a);
                for l in &layouts {
                    let hot = dragging == Some(l.axis) || state.borrow().over_track[idx(l.axis)];
                    let r = (TRACK - 2. * INSET) / 2.;
                    window.paint_quad(quad(
                        l.thumb,
                        Corners::all(px(r)),
                        if hot { hover } else { rest },
                        0.,
                        gpui::transparent_black(),
                        BorderStyle::default(),
                    ));
                }
            }

            let geometry: Rc<Vec<(Axis, Bounds<Pixels>, Bounds<Pixels>, f32, f32, f32, Metrics)>> =
                Rc::new(
                    layouts
                        .iter()
                        .map(|l| (l.axis, l.track, l.thumb, l.len, l.start, l.track_len, l.m))
                        .collect(),
                );
            {
                let (state, geometry, source, hitbox) = (
                    state.clone(),
                    geometry.clone(),
                    source.clone(),
                    hitbox.clone(),
                );
                window.on_mouse_event(move |ev: &MouseDownEvent, phase, window, cx| {
                    if phase != DispatchPhase::Bubble || ev.button != MouseButton::Left {
                        return;
                    }
                    let Some((axis, track, thumb, len, start, track_len, m)) = geometry
                        .iter()
                        .find(|g| g.1.contains(&ev.position))
                        .copied()
                    else {
                        return;
                    };
                    if !hitbox.is_hovered(window) {
                        return;
                    }
                    let (pos, from, th_start) = match axis {
                        Axis::Vertical => (ev.position.y, track.top(), thumb.top()),
                        Axis::Horizontal => (ev.position.x, track.left(), thumb.left()),
                    };
                    if thumb.contains(&ev.position) {
                        state.borrow_mut().drag = Some((axis, f32::from(pos - th_start)));
                    } else {
                        let click = f32::from(pos - from) - INSET;
                        source.0.set_offset(axis, page_target(click, start, m), cx);
                        let _ = (len, track_len);
                    }
                    window.refresh();
                    cx.stop_propagation();
                });
            }
            {
                let (state, geometry, source, hitbox) = (
                    state.clone(),
                    geometry.clone(),
                    source.clone(),
                    hitbox.clone(),
                );
                window.on_mouse_event(move |ev: &MouseMoveEvent, phase, window, cx| {
                    if phase != DispatchPhase::Bubble {
                        return;
                    }
                    let drag = state.borrow().drag;
                    if let Some((axis, grab)) = drag {
                        if ev.pressed_button != Some(MouseButton::Left) {
                            state.borrow_mut().drag = None;
                        } else if let Some((_, track, _, len, _, track_len, m)) =
                            geometry.iter().find(|g| g.0 == axis).copied()
                        {
                            let pos = match axis {
                                Axis::Vertical => ev.position.y - track.top(),
                                Axis::Horizontal => ev.position.x - track.left(),
                            };
                            let start = f32::from(pos) - INSET - grab;
                            source
                                .0
                                .set_offset(axis, offset_at(track_len, len, start, m), cx);
                        }
                        window.refresh();
                        cx.stop_propagation();
                        return;
                    }
                    let over = hitbox.is_hovered(window);
                    let on = [Axis::Vertical, Axis::Horizontal].map(|a| {
                        geometry
                            .iter()
                            .any(|g| g.0 == a && g.1.contains(&ev.position))
                    });
                    let mut s = state.borrow_mut();
                    if s.over != over || s.over_track != on {
                        s.over = over;
                        s.over_track = on;
                        window.refresh();
                    }
                });
            }
            {
                let state = state.clone();
                window.on_mouse_event(move |ev: &MouseUpEvent, phase, window, _| {
                    if phase == DispatchPhase::Bubble
                        && ev.button == MouseButton::Left
                        && state.borrow_mut().drag.take().is_some()
                    {
                        window.refresh();
                    }
                });
            }
        },
    )
    .absolute()
    .top_0()
    .left_0()
    .size_full()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn m(viewport: f32, content: f32, offset: f32) -> Metrics {
        Metrics {
            viewport,
            content,
            offset,
        }
    }

    // Failure modes: nothing to scroll, a degenerate track or viewport, a huge
    // document (thumb under the minimum), an offset outside the range, and the
    // pointer position mapping back to the offset it came from.
    #[test]
    fn no_thumb_when_everything_fits() {
        assert_eq!(thumb(200., m(100., 100., 0.)), None);
        assert_eq!(thumb(200., m(100., 100.3, 0.)), None);
        assert_eq!(thumb(0., m(100., 400., 0.)), None);
        assert_eq!(thumb(200., m(0., 400., 0.)), None);
    }

    #[test]
    fn the_thumb_is_proportional_and_travels_the_whole_track() {
        let (start, len) = thumb(200., m(100., 400., 0.)).unwrap();
        assert_eq!((start, len), (0., 50.));
        let (start, len) = thumb(200., m(100., 400., 300.)).unwrap();
        assert_eq!((start + len, len), (200., 50.));
    }

    #[test]
    fn a_huge_document_keeps_a_grabbable_thumb() {
        let (_, len) = thumb(200., m(30., 10_000_000., 0.)).unwrap();
        assert_eq!(len, MIN_THUMB);
        // And a track shorter than the minimum is filled, not overrun.
        let (start, len) = thumb(10., m(30., 1000., 500.)).unwrap();
        assert!(len <= 10. && start >= 0. && start + len <= 10.);
    }

    #[test]
    fn offsets_outside_the_range_stay_on_the_track() {
        let (start, len) = thumb(200., m(100., 400., 9999.)).unwrap();
        assert_eq!(start + len, 200.);
        let (start, _) = thumb(200., m(100., 400., -50.)).unwrap();
        assert_eq!(start, 0.);
    }

    #[test]
    fn dragging_maps_back_to_the_offset() {
        let met = m(100., 400., 120.);
        let (start, len) = thumb(200., met).unwrap();
        assert!((offset_at(200., len, start, met) - 120.).abs() < 0.01);
        assert_eq!(offset_at(200., len, -30., met), 0.);
        assert_eq!(offset_at(200., len, 500., met), 300.);
        assert_eq!(offset_at(50., 50., 10., met), 0.);
    }

    // Failure modes: a graph that fits shows no bar, panning off the graph
    // keeps the thumb on the track, and a dragged offset maps back to the pan.
    #[test]
    fn a_pan_canvas_scrolls_only_when_the_graph_is_bigger_than_the_view() {
        let st = |pan: (f32, f32), zoom: f32| PanState {
            pan,
            zoom,
            graph: (1000., 400.),
            viewport: (800., 500.),
        };
        let fits = st((150., 50.), 0.7);
        let (h, v) = (
            pan_metrics(&fits, Axis::Horizontal).unwrap(),
            pan_metrics(&fits, Axis::Vertical).unwrap(),
        );
        assert!(
            thumb(800., h).is_none() && thumb(500., v).is_none(),
            "{h:?} {v:?}"
        );
        let big = pan_metrics(&st((0., 0.), 1.), Axis::Horizontal).unwrap();
        assert!(thumb(800., big).is_some());
        let off = pan_metrics(&st((-9999., 0.), 1.), Axis::Horizontal).unwrap();
        let (start, len) = thumb(800., off).unwrap();
        assert!(start + len <= 800.01);
        assert_eq!(pan_for_offset(big.offset), 0.);
    }

    #[test]
    fn a_track_click_pages_towards_the_pointer_and_stops_at_the_ends() {
        let met = m(100., 400., 10.);
        assert_eq!(page_target(5., 20., met), 0.);
        assert_eq!(page_target(180., 20., met), 100.);
        assert_eq!(page_target(180., 20., m(100., 400., 290.)), 300.);
    }
}
