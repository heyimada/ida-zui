// Painting shared by the control-flow graph and the proximity view: routed
// edges with their arrowhead, and the minimap in the canvas corner.
use crate::graph_layout::Layout;
use crate::theme::{Theme, hsla};
use gpui::{Bounds, Hsla, PathBuilder, Pixels, Point, Window, point, px, size};

/// Size of the minimap's content area and its gap to the canvas corner.
pub const MINIMAP_MAX: (f32, f32) = (160., 110.);
pub const MINIMAP_GAP: f32 = 12.;

/// One edge: the polyline through its waypoints and an arrowhead pointing
/// down into the target.
pub fn edge(
    window: &mut Window,
    points: &[(f32, f32)],
    color: Hsla,
    zoom: f32,
    to_screen: &impl Fn(f32, f32) -> Point<Pixels>,
) {
    let Some(&(x, y)) = points.last() else { return };
    let mut pb = PathBuilder::stroke(px((1.5 * zoom).max(1.)));
    for (i, &(x, y)) in points.iter().enumerate() {
        let p = to_screen(x, y);
        if i == 0 { pb.move_to(p) } else { pb.line_to(p) }
    }
    if let Ok(path) = pb.build() {
        window.paint_path(path, color);
    }
    let tip = to_screen(x, y);
    let s = px(4. * zoom.max(0.5));
    let mut ab = PathBuilder::fill();
    ab.move_to(tip);
    ab.line_to(point(tip.x - s, tip.y - s * 1.5));
    ab.line_to(point(tip.x + s, tip.y - s * 1.5));
    ab.close();
    if let Ok(path) = ab.build() {
        window.paint_path(path, color);
    }
}

/// The blocks at a glance and the part in view, in the bottom-right corner
/// of `bounds`. Returns the inner rectangle and the scale from graph units,
/// which clicks use to pan.
pub fn minimap(
    window: &mut Window,
    bounds: Bounds<Pixels>,
    theme: &Theme,
    layout: &Layout,
    pan: Point<Pixels>,
    zoom: f32,
) -> Option<(Bounds<Pixels>, f32)> {
    if layout.width <= 0. || layout.height <= 0. {
        return None;
    }
    let scale = (MINIMAP_MAX.0 / layout.width).min(MINIMAP_MAX.1 / layout.height);
    let (mw, mh) = (layout.width * scale, layout.height * scale);
    let mm = Bounds::new(
        point(
            bounds.right() - px(mw + 12. + MINIMAP_GAP),
            bounds.bottom() - px(mh + 12. + MINIMAP_GAP),
        ),
        size(px(mw + 12.), px(mh + 12.)),
    );
    window.paint_quad(gpui::quad(
        mm,
        px(8.),
        hsla(theme.popover),
        px(1.),
        hsla(theme.border_strong),
        gpui::BorderStyle::Solid,
    ));
    let inner = point(mm.left() + px(6.), mm.top() + px(6.));
    for r in &layout.nodes {
        window.paint_quad(gpui::quad(
            Bounds::new(
                point(inner.x + px(r.x * scale), inner.y + px(r.y * scale)),
                size(px((r.w * scale).max(2.)), px((r.h * scale).max(2.))),
            ),
            px(1.),
            hsla(theme.raised),
            px(0.),
            gpui::transparent_black(),
            gpui::BorderStyle::Solid,
        ));
    }
    // The viewport in graph units.
    let vx = -f32::from(pan.x) / zoom;
    let vy = -f32::from(pan.y) / zoom;
    let vw = f32::from(bounds.size.width) / zoom;
    let vh = f32::from(bounds.size.height) / zoom;
    let view = Bounds::new(
        point(inner.x + px(vx * scale), inner.y + px(vy * scale)),
        size(px(vw * scale), px(vh * scale)),
    );
    window.with_content_mask(Some(gpui::ContentMask { bounds: mm }), |window| {
        window.paint_quad(gpui::quad(
            view,
            px(2.),
            hsla(theme.accent).opacity(0.14),
            px(1.),
            hsla(theme.accent),
            gpui::BorderStyle::Solid,
        ));
    });
    Some((Bounds::new(inner, size(px(mw), px(mh))), scale))
}
