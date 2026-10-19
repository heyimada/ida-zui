// Layered (Sugiyama-style) layout for control-flow graphs. Pure geometry so
// it can be tested without a window.
// Some fields exist for the invariant tests only.
#![cfg_attr(not(test), allow(dead_code))]

pub const H_GAP: f32 = 48.;
pub const V_GAP: f32 = 64.;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl Rect {
    pub fn right(&self) -> f32 {
        self.x + self.w
    }
    pub fn bottom(&self) -> f32 {
        self.y + self.h
    }
    pub fn overlaps(&self, o: &Rect) -> bool {
        self.x < o.right() && o.x < self.right() && self.y < o.bottom() && o.y < self.bottom()
    }
}

#[derive(Clone, Debug)]
pub struct EdgePath {
    pub from: usize,
    pub to: usize,
    /// True for edges that go against the layering (loops).
    pub back: bool,
    pub points: Vec<(f32, f32)>,
}

#[derive(Clone, Debug, Default)]
pub struct Layout {
    pub nodes: Vec<Rect>,
    pub layer: Vec<usize>,
    pub edges: Vec<EdgePath>,
    pub width: f32,
    pub height: f32,
}

/// Horizontal room kept around the vertical run of a long edge.
const ROUTE_GAP: f32 = 18.;
/// Vertical room for each horizontal channel in a gap between layers.
const TRACK: f32 = 10.;

/// Lays out `sizes.len()` nodes (node 0 is the entry) connected by `edges`.
pub fn layout(sizes: &[(f32, f32)], edges: &[(usize, usize)]) -> Layout {
    build(sizes, edges, None)
}

/// Like `layout`, but the caller chooses each node's layer (the proximity
/// view puts callers above and callees below). An edge that does not go to a
/// deeper layer is drawn as a loop around the side.
pub fn layout_in_layers(
    sizes: &[(f32, f32)],
    edges: &[(usize, usize)],
    layers: &[usize],
) -> Layout {
    build(sizes, edges, Some(layers))
}

/// Edges into a node still on the DFS stack, and the order of discovery.
fn dfs_back_edges(
    n: usize,
    edges: &[(usize, usize)],
    adj: &[Vec<(usize, usize)>],
) -> (Vec<bool>, Vec<usize>) {
    let mut state = vec![0u8; n];
    let mut back = vec![false; edges.len()];
    let mut discovered = vec![0usize; n];
    let mut next_index = 0;
    for root in 0..n {
        if state[root] != 0 {
            continue;
        }
        state[root] = 1;
        discovered[root] = next_index;
        next_index += 1;
        let mut stack = vec![(root, 0usize)];
        while let Some(top) = stack.last_mut() {
            let (v, i) = *top;
            if i < adj[v].len() {
                top.1 += 1;
                let (w, k) = adj[v][i];
                match state[w] {
                    0 => {
                        state[w] = 1;
                        discovered[w] = next_index;
                        next_index += 1;
                        stack.push((w, 0));
                    }
                    1 => back[k] = true,
                    _ => {}
                }
            } else {
                state[v] = 2;
                stack.pop();
            }
        }
    }
    (back, discovered)
}

fn build(sizes: &[(f32, f32)], edges: &[(usize, usize)], fixed: Option<&[usize]>) -> Layout {
    let n = sizes.len();
    if n == 0 {
        return Layout::default();
    }
    let mut adj: Vec<Vec<(usize, usize)>> = vec![Vec::new(); n];
    for (k, &(a, b)) in edges.iter().enumerate() {
        adj[a].push((b, k));
    }

    // 1. Back edges: against the given layers, or found by a DFS from the
    //    entry (then any unvisited node); self-loops are back edges too.
    let (back, discovered): (Vec<bool>, Vec<usize>) = match fixed {
        Some(l) => (
            edges.iter().map(|&(a, b)| l[b] <= l[a]).collect(),
            (0..n).collect(),
        ),
        None => {
            let (mut back, discovered) = dfs_back_edges(n, edges, &adj);
            // Whatever jumps to the entry loops back to it, so it stays on top.
            for (k, &(_, b)) in edges.iter().enumerate() {
                back[k] |= b == 0;
            }
            (back, discovered)
        }
    };

    // 2. Layers: the given ones (made dense), or longest paths over the
    //    forward edges (a DAG).
    let mut by_discovery: Vec<usize> = (0..n).collect();
    by_discovery.sort_by_key(|&v| discovered[v]);
    let layer: Vec<usize> = match fixed {
        Some(l) => {
            let mut used: Vec<usize> = l.to_vec();
            used.sort_unstable();
            used.dedup();
            l.iter().map(|x| used.binary_search(x).unwrap()).collect()
        }
        None => {
            let mut indeg = vec![0usize; n];
            for (k, &(_, b)) in edges.iter().enumerate() {
                if !back[k] {
                    indeg[b] += 1;
                }
            }
            let mut queue: std::collections::VecDeque<usize> = by_discovery
                .iter()
                .copied()
                .filter(|&v| indeg[v] == 0)
                .collect();
            let mut layer = vec![0usize; n];
            while let Some(v) = queue.pop_front() {
                for &(w, k) in &adj[v] {
                    if back[k] {
                        continue;
                    }
                    layer[w] = layer[w].max(layer[v] + 1);
                    indeg[w] -= 1;
                    if indeg[w] == 0 {
                        queue.push_back(w);
                    }
                }
            }
            layer
        }
    };
    let layers_n = layer.iter().max().unwrap() + 1;

    // 3. Waypoints: every forward edge is cut into one-layer pieces by
    //    waypoint items (indices from `n`), one per layer it crosses, so
    //    ordering and placement treat a long edge like a thin block and
    //    keep it out of the blocks' way.
    let mut item_layer = layer.clone();
    let mut preds: Vec<Vec<usize>> = vec![Vec::new(); n];
    let mut succs: Vec<Vec<usize>> = vec![Vec::new(); n];
    let mut chains: Vec<Vec<usize>> = vec![Vec::new(); edges.len()];
    // Long edges into one block share their waypoints (one per layer and
    // target), so they run down one trunk instead of side by side.
    let mut trunk_item: std::collections::HashMap<(usize, usize), usize> =
        std::collections::HashMap::new();
    for (k, &(a, b)) in edges.iter().enumerate() {
        if back[k] || a == b {
            continue;
        }
        let mut prev = a;
        for l in layer[a] + 1..layer[b] {
            let d = *trunk_item.entry((b, l)).or_insert_with(|| {
                item_layer.push(l);
                preds.push(Vec::new());
                succs.push(Vec::new());
                item_layer.len() - 1
            });
            chains[k].push(d);
            if !succs[prev].contains(&d) {
                succs[prev].push(d);
                preds[d].push(prev);
            }
            prev = d;
        }
        if !succs[prev].contains(&b) {
            succs[prev].push(b);
            preds[b].push(prev);
        }
    }
    let items = item_layer.len();
    let width = |v: usize| if v < n { sizes[v].0 } else { 0. };
    let gap = |a: usize, b: usize| if a >= n || b >= n { ROUTE_GAP } else { H_GAP };

    // 4. Order items within layers by barycentre sweeps.
    let mut layers: Vec<Vec<usize>> = vec![Vec::new(); layers_n];
    for v in by_discovery.iter().copied().chain(n..items) {
        layers[item_layer[v]].push(v);
    }
    let mut pos = vec![0f32; items];
    for l in &layers {
        for (i, &v) in l.iter().enumerate() {
            pos[v] = i as f32;
        }
    }
    for _ in 0..8 {
        for layer in layers.iter_mut().take(layers_n).skip(1) {
            sort_by_bary(layer, &preds, &mut pos);
        }
        for l in (0..layers_n.saturating_sub(1)).rev() {
            sort_by_bary(&mut layers[l], &succs, &mut pos);
        }
    }

    // 5. Horizontal placement. Within a layer, items aim for the mean centre
    //    of their predecessors and keep their distance.
    let mut cx = vec![0f32; items]; // left edge of a block, x of a waypoint
    for (li, l) in layers.iter().enumerate() {
        let desired: Vec<f32> = l
            .iter()
            .map(|&v| {
                let ps: Vec<f32> = preds[v].iter().map(|&p| cx[p] + width(p) / 2.).collect();
                if ps.is_empty() {
                    f32::NAN
                } else {
                    ps.iter().sum::<f32>() / ps.len() as f32
                }
            })
            .collect();
        // Least squares towards the desired positions with the order and the
        // distances kept (pool-adjacent-violators). Blocks weigh far more than
        // waypoints, so a crowded layer moves the waypoints, not the blocks.
        let mut offset = Vec::with_capacity(l.len());
        let mut run = 0f32;
        for (i, &v) in l.iter().enumerate() {
            if i > 0 {
                run += width(l[i - 1]) + gap(l[i - 1], v);
            }
            offset.push(run);
        }
        let mut targets: Vec<f32> = Vec::with_capacity(l.len());
        let mut weights: Vec<f32> = Vec::with_capacity(l.len());
        for (i, &v) in l.iter().enumerate() {
            if desired[i].is_nan() {
                targets.push(targets.last().copied().unwrap_or(0.));
                weights.push(1e-3);
            } else {
                targets.push(desired[i] - width(v) / 2. - offset[i]);
                weights.push(if v < n { 1000. } else { 1. });
            }
        }
        let fitted = isotonic(&targets, &weights);
        for (i, &v) in l.iter().enumerate() {
            cx[v] = fitted[i] + offset[i];
        }
        // A waypoint is nudged off the line of another edge's waypoint in the
        // layer above, so two long edges never merge into one line.
        if li > 0 {
            for &v in l.iter().filter(|&&v| v >= n) {
                for _ in 0..4 {
                    let clash = layers[li - 1]
                        .iter()
                        .any(|&p| p >= n && !preds[v].contains(&p) && (cx[p] - cx[v]).abs() < 3.);
                    if !clash {
                        break;
                    }
                    cx[v] += 4.;
                }
            }
        }
    }
    let min_x = cx.iter().copied().fold(f32::INFINITY, f32::min);
    for x in &mut cx {
        *x -= min_x;
    }
    let max_right = (0..items).map(|v| cx[v] + width(v)).fold(0., f32::max);

    // 6. Ports: each block spreads its outgoing and incoming edges along its
    //    bottom and top, ordered by where they go so they cross less. Edges
    //    that loop around the side come last.
    let mid = |v: usize| cx[v] + width(v) / 2.;
    let mut outs: Vec<Vec<(f32, usize)>> = vec![Vec::new(); n];
    let mut ins: Vec<Vec<(f32, usize)>> = vec![Vec::new(); n];
    // Edges that arrive on one trunk (loops into a block, or long edges
    // whose last waypoint is shared) end at one port: the first of them owns
    // the port and the arrival channel, the rest follow it.
    let mut arrival: std::collections::HashMap<(usize, usize), usize> =
        std::collections::HashMap::new();
    let mut owner = vec![0usize; edges.len()];
    for (k, &(a, b)) in edges.iter().enumerate() {
        let via = if back[k] || a == b {
            usize::MAX
        } else {
            chains[k].last().copied().unwrap_or(usize::MAX - 1 - k)
        };
        owner[k] = *arrival.entry((b, via)).or_insert(k);
        if back[k] || a == b {
            outs[a].push((f32::INFINITY, k));
            if owner[k] == k {
                ins[b].push((f32::INFINITY, k));
            }
        } else {
            outs[a].push((chains[k].first().map_or(mid(b), |&d| cx[d]), k));
            if owner[k] == k {
                ins[b].push((chains[k].last().map_or(mid(a), |&d| cx[d]), k));
            }
        }
    }
    let mut sx = vec![0f32; edges.len()];
    let mut tx = vec![0f32; edges.len()];
    // A port is nudged off the line of a waypoint in the layer next to it, so
    // two edges never run on top of each other there.
    let waypoint_x: Vec<Vec<f32>> = layers
        .iter()
        .map(|l| l.iter().filter(|&&v| v >= n).map(|&v| cx[v]).collect())
        .collect();
    for v in 0..n {
        for (is_out, list, ports) in [(true, &mut outs[v], &mut sx), (false, &mut ins[v], &mut tx)]
        {
            list.sort_by(|x, y| x.partial_cmp(y).unwrap_or(std::cmp::Ordering::Equal));
            let count = list.len();
            let near = if is_out {
                layer[v] + 1
            } else {
                layer[v].wrapping_sub(1)
            };
            for (i, &(_, k)) in list.iter().enumerate() {
                let mut x = cx[v] + sizes[v].0 * (i + 1) as f32 / (count + 1) as f32;
                for _ in 0..8 {
                    if !waypoint_x
                        .get(near)
                        .is_some_and(|w| w.iter().any(|&p| (p - x).abs() < 2.))
                    {
                        break;
                    }
                    x += 3.;
                }
                ports[k] = x.min(cx[v] + sizes[v].0 - 1.);
            }
        }
    }

    for k in 0..edges.len() {
        if owner[k] != k {
            tx[k] = tx[owner[k]];
        }
    }

    // 7. Channels. Gap g lies above layer g (gap `layers_n` below the last).
    //    Every sideways run of an edge sits in a gap, on a track of its own
    //    among the runs that overlap it.
    let lane_x = |lane: usize| max_right + 16. + lane as f32 * 10.;
    let mut lane = 0;
    let mut lane_of = vec![0usize; edges.len()];
    let mut lane_of_target: Vec<Option<usize>> = vec![None; n];
    // (gap, lo, hi, edge, which run of the edge)
    let mut runs: Vec<(usize, f32, f32, usize, usize)> = Vec::new();
    let mut piece_of: std::collections::HashMap<(usize, usize, usize), (usize, usize)> =
        std::collections::HashMap::new();
    let mut alias: std::collections::HashMap<(usize, usize), (usize, usize)> =
        std::collections::HashMap::new();
    for (k, &(a, b)) in edges.iter().enumerate() {
        let (la, lb) = (layer[a], layer[b]);
        if back[k] || a == b {
            lane_of[k] = *lane_of_target[b].get_or_insert_with(|| {
                lane += 1;
                lane
            });
            let lx = lane_x(lane_of[k]);
            runs.push((la + 1, sx[k], lx, k, 0));
            if owner[k] == k {
                runs.push((lb, tx[k], lx, k, 1));
            }
        } else {
            let mut x = sx[k];
            for (i, l) in (la + 1..=lb).enumerate() {
                let next = hop(x, l == lb, tx[k], || cx[chains[k][l - la - 1]]);
                if (next - x).abs() > 0.05 {
                    // A piece between the same two waypoints (or into the same
                    // arrival) as an earlier edge's is that edge's run.
                    let from = if i == 0 {
                        usize::MAX - k
                    } else {
                        chains[k][i - 1]
                    };
                    let to = if l == lb {
                        usize::MAX / 2 + owner[k]
                    } else {
                        chains[k][l - la - 1]
                    };
                    match piece_of.get(&(l, from, to)) {
                        Some(&rep) => {
                            alias.insert((k, i), rep);
                        }
                        None => {
                            piece_of.insert((l, from, to), (k, i));
                            runs.push((l, x.min(next), x.max(next), k, i));
                        }
                    }
                }
                x = next;
            }
        }
    }
    runs.sort_by(|p, q| {
        (p.0, p.1, p.2)
            .partial_cmp(&(q.0, q.1, q.2))
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    let mut track_of = std::collections::HashMap::new();
    let mut tracks = vec![0usize; layers_n + 1];
    let mut ends: Vec<Vec<f32>> = vec![Vec::new(); layers_n + 1];
    for &(g, lo, hi, k, which) in &runs {
        let t = match ends[g].iter().position(|&e| e + 6. < lo) {
            Some(t) => t,
            None => {
                ends[g].push(0.);
                ends[g].len() - 1
            }
        };
        ends[g][t] = hi;
        tracks[g] = ends[g].len();
        track_of.insert((k, which), t);
    }

    // 8. Vertical placement: gaps grow to hold their tracks.
    let gap_h: Vec<f32> = (0..=layers_n)
        .map(|g| match (tracks[g], g) {
            (0, 0) => 0.,
            (0, _) => V_GAP,
            (t, _) => V_GAP.max((t + 1) as f32 * TRACK),
        })
        .collect();
    let mut gap_top = vec![0f32; layers_n + 1];
    let mut layer_top = vec![0f32; layers_n];
    let mut y = 0.;
    for l in 0..layers_n {
        gap_top[l] = y;
        layer_top[l] = y + gap_h[l];
        let h = layers[l]
            .iter()
            .filter(|&&v| v < n)
            .map(|&v| sizes[v].1)
            .fold(0., f32::max);
        y = layer_top[l] + h;
    }
    gap_top[layers_n] = y;
    let track_y =
        |g: usize, t: usize| gap_top[g] + gap_h[g] * (t + 1) as f32 / (tracks[g] + 1) as f32;
    let nodes: Vec<Rect> = (0..n)
        .map(|v| Rect {
            x: cx[v],
            y: layer_top[layer[v]],
            w: sizes[v].0,
            h: sizes[v].1,
        })
        .collect();

    // 9. Routes. Forward edges drop to their first channel, hop sideways
    //    from waypoint to waypoint and enter the target from above; loops
    //    leave below the source's layer, run down a lane right of the graph
    //    and come in from above the target's layer.
    let mut paths = Vec::with_capacity(edges.len());
    for (k, &(a, b)) in edges.iter().enumerate() {
        let (ra, rb) = (nodes[a], nodes[b]);
        let (la, lb) = (layer[a], layer[b]);
        let is_back = back[k] || a == b;
        let mut points = vec![(sx[k], ra.bottom())];
        if is_back {
            let lx = lane_x(lane_of[k]);
            let y1 = track_y(la + 1, track_of[&(k, 0)]);
            let y2 = track_y(lb, track_of[&(owner[k], 1)]);
            points.extend([(sx[k], y1), (lx, y1), (lx, y2), (tx[k], y2)]);
        } else {
            let mut x = sx[k];
            for (i, l) in (la + 1..=lb).enumerate() {
                let next = hop(x, l == lb, tx[k], || cx[chains[k][l - la - 1]]);
                if let Some(&t) = track_of.get(&alias.get(&(k, i)).copied().unwrap_or((k, i))) {
                    let yy = track_y(l, t);
                    points.extend([(x, yy), (next, yy)]);
                }
                x = next;
            }
        }
        points.push((tx[k], rb.y));
        // Runs shorter than the jog threshold are straightened.
        for i in 1..points.len() {
            if (points[i].0 - points[i - 1].0).abs() < 0.05 {
                points[i].0 = points[i - 1].0;
            }
        }
        paths.push(EdgePath {
            from: a,
            to: b,
            back: is_back,
            points,
        });
    }

    Layout {
        width: max_right + 16. + (lane + 1) as f32 * 10.,
        height: gap_top[layers_n] + gap_h[layers_n],
        nodes,
        layer,
        edges: paths,
    }
}

/// Where a long edge is after one more layer: at the waypoint, unless that
/// is within a hair of where it is (then it stays on its line, so a trunk
/// whose waypoints drift by fractions of a pixel stays one vertical line).
fn hop(x: f32, last: bool, port: f32, waypoint: impl FnOnce() -> f32) -> f32 {
    if last {
        return port;
    }
    let w = waypoint();
    if (w - x).abs() <= 0.05 { x } else { w }
}

/// The non-decreasing sequence closest to `targets` in weighted least squares.
fn isotonic(targets: &[f32], weights: &[f32]) -> Vec<f32> {
    // Blocks of equal value: (mean, weight, count).
    let mut blocks: Vec<(f32, f32, usize)> = Vec::new();
    for (&t, &w) in targets.iter().zip(weights) {
        blocks.push((t, w, 1));
        while blocks.len() > 1 && blocks[blocks.len() - 2].0 > blocks[blocks.len() - 1].0 {
            let (m2, w2, c2) = blocks.pop().unwrap();
            let (m1, w1, c1) = blocks.pop().unwrap();
            let w = w1 + w2;
            blocks.push(((m1 * w1 + m2 * w2) / w, w, c1 + c2));
        }
    }
    blocks
        .into_iter()
        .flat_map(|(m, _, c)| std::iter::repeat_n(m, c))
        .collect()
}

fn sort_by_bary(layer: &mut [usize], neighbours: &[Vec<usize>], pos: &mut [f32]) {
    let bary: Vec<(usize, f32)> = layer
        .iter()
        .map(|&v| {
            let ns = &neighbours[v];
            let b = if ns.is_empty() {
                pos[v]
            } else {
                ns.iter().map(|&u| pos[u]).sum::<f32>() / ns.len() as f32
            };
            (v, b)
        })
        .collect();
    let mut sorted = bary;
    sorted.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal));
    for (i, (v, _)) in sorted.into_iter().enumerate() {
        layer[i] = v;
        pos[v] = i as f32;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn check_invariants(sizes: &[(f32, f32)], edges: &[(usize, usize)]) -> Layout {
        let l = layout(sizes, edges);
        assert_eq!(l.nodes.len(), sizes.len(), "every block is placed");
        for (i, r) in l.nodes.iter().enumerate() {
            assert!(
                r.x.is_finite() && r.y.is_finite(),
                "node {i} has finite position"
            );
            assert_eq!((r.w, r.h), sizes[i], "node {i} keeps its size");
            assert!(r.x >= 0. && r.y >= 0., "node {i} is inside the canvas");
            assert!(r.right() <= l.width + 0.01 && r.bottom() <= l.height + 0.01);
        }
        for i in 0..l.nodes.len() {
            for j in i + 1..l.nodes.len() {
                assert!(
                    !l.nodes[i].overlaps(&l.nodes[j]),
                    "nodes {i} and {j} overlap"
                );
            }
        }
        if !sizes.is_empty() {
            let top = l.nodes.iter().map(|r| r.y).fold(f32::INFINITY, f32::min);
            assert_eq!(l.nodes[0].y, top, "entry is in the top layer");
        }
        assert_eq!(l.edges.len(), edges.len(), "every edge is routed");
        for e in &l.edges {
            let (a, b) = (l.nodes[e.from], l.nodes[e.to]);
            assert!(e.points.len() >= 2);
            let first = e.points[0];
            let last = *e.points.last().unwrap();
            assert!(
                (first.1 - a.bottom()).abs() < 0.01,
                "edge starts at the bottom of its source"
            );
            assert!(first.0 >= a.x && first.0 <= a.right());
            assert!(
                (last.1 - b.y).abs() < 0.01,
                "edge ends at the top of its target"
            );
            assert!(last.0 >= b.x && last.0 <= b.right());
            if !e.back {
                assert!(b.y > a.y, "forward edge {}->{} points down", e.from, e.to);
            }
        }
        check_routes(&l);
        l
    }

    // Failure modes of the edge routing, listed before the router was written:
    //  1. A long edge runs straight down through a block in an intermediate layer.
    //  2. A jog (horizontal run) happens inside a layer's row, not in the gap
    //     between layers, so it cuts blocks beside the source or target.
    //  3. A back edge leaves a short block and runs sideways through a taller
    //     neighbour (the exit channel must sit below the layer's tallest block).
    //  4. A back edge returns to a block in the top layer through nothing but
    //     blocks (needs a channel above the first layer).
    //  5. A self-loop cuts its own block.
    //  6. Routing points are not orthogonal (a diagonal) or the last segment
    //     is not vertical into the target (the arrowhead assumes it).
    //  7. Long-edge waypoints land on a block of their layer.
    //  8. Edges that share a channel lie on top of each other.
    //  9. Waypoint bookkeeping leaks into `nodes` / `layer`.
    const CLEAR: f32 = 3.;
    /// Two waypoints of different layers can fall on one x by chance; such
    /// edges (or a port and a waypoint) may then run together for less than one gap.
    const MERGE: f32 = V_GAP;

    fn segs(e: &EdgePath) -> Vec<((f32, f32), (f32, f32))> {
        e.points.windows(2).map(|w| (w[0], w[1])).collect()
    }

    /// True if the axis-aligned segment enters `r` inflated by `pad`.
    fn hits(s: ((f32, f32), (f32, f32)), r: &Rect, pad: f32) -> bool {
        let (x0, x1) = (s.0.0.min(s.1.0), s.0.0.max(s.1.0));
        let (y0, y1) = (s.0.1.min(s.1.1), s.0.1.max(s.1.1));
        x1 > r.x - pad && x0 < r.right() + pad && y1 > r.y - pad && y0 < r.bottom() + pad
    }

    fn check_routes(l: &Layout) {
        for (k, e) in l.edges.iter().enumerate() {
            let last = e.points.last().unwrap();
            let before = e.points[e.points.len() - 2];
            assert!(
                (last.0 - before.0).abs() < 0.1 && last.1 > before.1,
                "edge {k} ends with a downward vertical"
            );
            for s in segs(e) {
                assert!(
                    (s.0.0 - s.1.0).abs() < 0.1 || (s.0.1 - s.1.1).abs() < 0.1,
                    "edge {k} segment {s:?} is diagonal"
                );
                for (i, r) in l.nodes.iter().enumerate() {
                    let own = i == e.from || i == e.to;
                    let pad = if own { -0.5 } else { CLEAR };
                    assert!(
                        !hits(s, r, pad),
                        "edge {}->{} segment {s:?} passes through block {i} {r:?}",
                        e.from,
                        e.to
                    );
                }
            }
        }
        // Edges between different pairs of blocks never share a stretch of
        // line. (Two edges of one block may run together for a few pixels
        // where a waypoint happens to sit under one of its ports.)
        for (i, a) in l.edges.iter().enumerate() {
            for b in &l.edges[i + 1..] {
                if a.from == b.from || a.to == b.to || a.from == b.to || a.to == b.from {
                    continue;
                }
                for sa in segs(a) {
                    for sb in segs(b) {
                        let vert = |s: ((f32, f32), (f32, f32))| (s.0.0 - s.1.0).abs() < 0.1;
                        if vert(sa) && vert(sb) && (sa.0.0 - sb.0.0).abs() < 0.5 {
                            let lo = sa.0.1.min(sa.1.1).max(sb.0.1.min(sb.1.1));
                            let hi = sa.0.1.max(sa.1.1).min(sb.0.1.max(sb.1.1));
                            assert!(
                                hi - lo < MERGE,
                                "edges {}->{} and {}->{} overlap vertically {:?} {:?}",
                                a.from,
                                a.to,
                                b.from,
                                b.to,
                                a.points,
                                b.points
                            );
                        }
                        let horiz = |s: ((f32, f32), (f32, f32))| (s.0.1 - s.1.1).abs() < 0.1;
                        if horiz(sa) && horiz(sb) && (sa.0.1 - sb.0.1).abs() < 0.5 {
                            let lo = sa.0.0.min(sa.1.0).max(sb.0.0.min(sb.1.0));
                            let hi = sa.0.0.max(sa.1.0).min(sb.0.0.max(sb.1.0));
                            assert!(
                                hi - lo < 1.,
                                "edges {}->{} and {}->{} overlap horizontally",
                                a.from,
                                a.to,
                                b.from,
                                b.to
                            );
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn skip_edge_clears_wide_intermediate_blocks() {
        // 0 -> 1 -> 2 -> 3 with wide middle blocks and a skip edge 0 -> 3.
        check_invariants(
            &[(80., 30.), (300., 40.), (300., 40.), (80., 30.)],
            &[(0, 1), (1, 2), (2, 3), (0, 3)],
        );
    }

    #[test]
    fn every_block_skips_to_the_last() {
        let n = 8;
        let sizes = vec![(120., 40.); n];
        let mut edges: Vec<(usize, usize)> = (0..n - 1).map(|i| (i, i + 1)).collect();
        edges.extend((0..n - 2).map(|i| (i, n - 1)));
        check_invariants(&sizes, &edges);
    }

    #[test]
    fn back_edge_from_a_short_block_clears_the_tall_neighbour() {
        // 1 is short and 2 is tall in the same layer; 1 loops back to 0.
        check_invariants(
            &[(100., 40.), (100., 20.), (100., 220.), (100., 40.)],
            &[(0, 1), (0, 2), (1, 3), (2, 3), (1, 0)],
        );
        // The same with the neighbours swapped.
        check_invariants(
            &[(100., 40.), (100., 220.), (100., 20.), (100., 40.)],
            &[(0, 1), (0, 2), (1, 3), (2, 3), (2, 0)],
        );
    }

    #[test]
    fn loop_in_the_top_layer_and_self_loop_on_tall_block() {
        check_invariants(
            &[(100., 40.), (100., 100.), (100., 20.)],
            &[(0, 1), (0, 2), (1, 1), (2, 0), (1, 2)],
        );
    }

    #[test]
    fn long_edge_waypoints_stay_out_of_crowded_layers() {
        // A wide fan with one edge that has to cross it.
        let n = 12;
        let mut sizes = vec![(100., 30.)];
        let mut edges = Vec::new();
        for i in 1..=n {
            sizes.push((140., 30. + (i % 4) as f32 * 20.));
            edges.push((0, i));
        }
        for i in 1..n {
            edges.push((i, i + 1));
        }
        sizes.push((100., 30.));
        edges.push((n, n + 1));
        edges.push((0, n + 1));
        check_invariants(&sizes, &edges);
    }

    #[test]
    fn random_graphs_route_cleanly() {
        // Deterministic pseudo-random graphs: forward edges, back edges,
        // self-loops, duplicates, unreachable blocks, mixed sizes.
        let mut state = 0x2545_F491_4F6C_DD1Du64;
        let mut rnd = move |m: usize| {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            (state % m as u64) as usize
        };
        for _ in 0..300 {
            let n = 2 + rnd(26);
            let sizes: Vec<(f32, f32)> = (0..n)
                .map(|_| (60. + rnd(300) as f32, 20. + rnd(160) as f32))
                .collect();
            let mut edges = Vec::new();
            for v in 1..n {
                if rnd(10) != 0 {
                    edges.push((rnd(v), v));
                }
            }
            for _ in 0..rnd(n * 2) {
                edges.push((rnd(n), rnd(n)));
            }
            check_invariants(&sizes, &edges);
        }
    }

    // Failure modes of the loop bundling, listed before it was written:
    //  1. Every loop edge still takes a lane of its own, so n loops into one
    //     block draw n parallel lines down the side channel.
    //  2. Loops into different blocks are merged into one trunk, which makes
    //     two unrelated edges look like one.
    //  3. Sharing the trunk lets an edge leave its own channel and cut a block.
    //  4. Edges that share a trunk end at different ports, so the arrival is a
    //     comb of separate lines instead of one arrow.
    //  5. A self-loop on the target loses its lane or its port.

    /// The x of every vertical run right of all blocks, one entry per line.
    fn lane_xs(l: &Layout) -> Vec<f32> {
        let right = l.nodes.iter().map(Rect::right).fold(0., f32::max);
        let mut xs: Vec<f32> = l
            .edges
            .iter()
            .filter(|e| e.back)
            .flat_map(segs)
            .filter(|s| (s.0.0 - s.1.0).abs() < 0.1 && s.0.0 > right + 1.)
            .map(|s| s.0.0)
            .collect();
        xs.sort_by(|a, b| a.partial_cmp(b).unwrap());
        xs.dedup_by(|a, b| (*a - *b).abs() < 0.5);
        xs
    }

    fn back_targets(l: &Layout) -> usize {
        let mut t: Vec<usize> = l.edges.iter().filter(|e| e.back).map(|e| e.to).collect();
        t.sort_unstable();
        t.dedup();
        t.len()
    }

    #[test]
    fn many_loops_into_one_block_share_one_lane_and_one_arrival() {
        // A chain 0 -> 1 -> ... -> 11 where every block jumps back to 1.
        let n = 12;
        let sizes = vec![(120., 40.); n];
        let mut edges: Vec<(usize, usize)> = (0..n - 1).map(|i| (i, i + 1)).collect();
        edges.extend((2..n).map(|i| (i, 1)));
        let l = check_invariants(&sizes, &edges);
        assert_eq!(l.edges.iter().filter(|e| e.back).count(), n - 2);
        assert_eq!(
            lane_xs(&l).len(),
            1,
            "one lane for one target: {:?}",
            lane_xs(&l)
        );
        let ends: Vec<(f32, f32)> = l
            .edges
            .iter()
            .filter(|e| e.back)
            .map(|e| *e.points.last().unwrap())
            .collect();
        assert!(
            ends.iter().all(|p| (p.0 - ends[0].0).abs() < 0.01),
            "one arrival port: {ends:?}"
        );
    }

    #[test]
    fn loops_into_different_blocks_keep_separate_lanes() {
        let n = 8;
        let sizes = vec![(120., 40.); n];
        let mut edges: Vec<(usize, usize)> = (0..n - 1).map(|i| (i, i + 1)).collect();
        edges.extend([(4, 1), (5, 1), (6, 2), (7, 2), (7, 3)]);
        let l = check_invariants(&sizes, &edges);
        assert_eq!(back_targets(&l), 3);
        assert_eq!(lane_xs(&l).len(), 3, "{:?}", lane_xs(&l));
    }

    #[test]
    fn a_self_loop_joins_the_lane_of_its_block() {
        let sizes = vec![(100., 40.); 4];
        let l = check_invariants(&sizes, &[(0, 1), (1, 2), (2, 3), (1, 1), (3, 1), (2, 1)]);
        assert_eq!(lane_xs(&l).len(), 1);
    }

    #[test]
    fn lanes_never_outnumber_the_targets_of_loops() {
        let mut state = 0x9E37_79B9_7F4A_7C15u64;
        let mut rnd = move |m: usize| {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            (state % m as u64) as usize
        };
        for _ in 0..200 {
            let n = 3 + rnd(20);
            let sizes: Vec<(f32, f32)> = (0..n)
                .map(|_| (60. + rnd(200) as f32, 20. + rnd(100) as f32))
                .collect();
            let mut edges: Vec<(usize, usize)> = (1..n).map(|v| (rnd(v), v)).collect();
            for _ in 0..rnd(n * 3) {
                edges.push((rnd(n), rnd(n)));
            }
            let l = check_invariants(&sizes, &edges);
            assert!(
                lane_xs(&l).len() <= back_targets(&l),
                "{} lanes for {} targets",
                lane_xs(&l).len(),
                back_targets(&l)
            );
        }
    }

    // Failure modes of the trunks for long forward edges, listed first:
    //  1. Every long edge into one block still runs its own vertical line.
    //  2. Edges into different blocks are merged into one trunk.
    //  3. Edges that share a trunk end at different ports.
    //  4. A trunk drifts into a block of the layers it crosses.
    //  5. Only the barycentre order changes: duplicates and self-joins of a
    //     shared waypoint leave a source without its edge (every edge keeps
    //     its endpoints; `check_invariants` covers that).

    /// The x of every vertical run that crosses at least one whole block.
    fn long_vertical_xs(l: &Layout, min_len: f32) -> Vec<f32> {
        let mut xs: Vec<f32> = l
            .edges
            .iter()
            .flat_map(segs)
            .filter(|s| (s.0.0 - s.1.0).abs() < 0.1 && (s.0.1 - s.1.1).abs() >= min_len)
            .map(|s| s.0.0)
            .collect();
        xs.sort_by(|a, b| a.partial_cmp(b).unwrap());
        xs.dedup_by(|a, b| (*a - *b).abs() < 0.5);
        xs
    }

    fn skip_chain(n: usize, targets: &[(usize, usize)]) -> (Vec<(f32, f32)>, Vec<(usize, usize)>) {
        let sizes = vec![(120., 300.); n];
        let mut edges: Vec<(usize, usize)> = (0..n - 1).map(|i| (i, i + 1)).collect();
        for &(from_to, to) in targets {
            edges.extend((0..from_to).map(|i| (i, to)));
        }
        (sizes, edges)
    }

    #[test]
    fn long_edges_into_one_block_share_a_trunk_and_a_port() {
        let (sizes, edges) = skip_chain(11, &[(8, 10)]);
        let l = check_invariants(&sizes, &edges);
        let long = long_vertical_xs(&l, 250.);
        // The chain itself is straight; the skips add at most the trunk.
        assert!(long.len() <= 3, "8 skips into one block: {long:?}");
        let ends: Vec<f32> = l
            .edges
            .iter()
            .filter(|e| e.to == 10 && l.layer[e.to] - l.layer[e.from] > 1)
            .map(|e| e.points.last().unwrap().0)
            .collect();
        assert!(
            ends.len() >= 8 && ends.iter().all(|x| (x - ends[0]).abs() < 0.01),
            "one arrival port: {ends:?}"
        );
    }

    #[test]
    fn trunks_into_different_blocks_stay_apart() {
        let (sizes, edges) = skip_chain(12, &[(5, 8), (7, 11)]);
        let l = check_invariants(&sizes, &edges);
        let ends8: Vec<f32> = l
            .edges
            .iter()
            .filter(|e| e.to == 8 && e.from + 1 < 8)
            .map(|e| e.points.last().unwrap().0)
            .collect();
        let ends11: Vec<f32> = l
            .edges
            .iter()
            .filter(|e| e.to == 11 && e.from + 1 < 11)
            .map(|e| e.points.last().unwrap().0)
            .collect();
        assert!(!ends8.is_empty() && !ends11.is_empty());
        let lo = long_vertical_xs(&l, 250.).len();
        assert!(lo >= 2, "two targets keep two trunks: {lo}");
    }

    #[test]
    fn trunk_edges_ending_in_one_block_keep_their_sources() {
        // Same shape, fanning in from blocks of different widths and heights.
        let sizes: Vec<(f32, f32)> = (0..9)
            .map(|i| (80. + 30. * (i % 4) as f32, 40. + 25. * (i % 3) as f32))
            .collect();
        let mut edges: Vec<(usize, usize)> = (0..8).map(|i| (i, i + 1)).collect();
        edges.extend((0..7).map(|i| (i, 8)));
        edges.push((0, 8)); // a duplicate of a skip
        let l = check_invariants(&sizes, &edges);
        for (k, &(a, b)) in edges.iter().enumerate() {
            assert_eq!((l.edges[k].from, l.edges[k].to), (a, b));
        }
    }

    #[test]
    fn blocks_with_given_layers_keep_them() {
        // Callers above, the centre in the middle, callees below; a callee that
        // calls back up is a back edge.
        let sizes = vec![(120., 30.); 5];
        let l = layout_in_layers(
            &sizes,
            &[(0, 2), (1, 2), (2, 3), (2, 4), (4, 2), (3, 4)],
            &[0, 0, 1, 2, 2],
        );
        assert_eq!(l.layer, vec![0, 0, 1, 2, 2]);
        assert!(l.nodes[0].y < l.nodes[2].y && l.nodes[2].y < l.nodes[3].y);
        assert_eq!(l.nodes[3].y, l.nodes[4].y);
        check_routes(&l);
        assert!(l.edges.iter().any(|e| e.from == 4 && e.to == 2 && e.back));
        assert!(
            l.edges.iter().any(|e| e.from == 3 && e.to == 4 && e.back),
            "an edge inside a layer loops around"
        );
    }

    #[test]
    fn single_block() {
        check_invariants(&[(120., 40.)], &[]);
    }

    #[test]
    fn diamond() {
        let l = check_invariants(
            &[(100., 40.), (80., 30.), (80., 60.), (100., 40.)],
            &[(0, 1), (0, 2), (1, 3), (2, 3)],
        );
        assert!(
            l.nodes[3].y > l.nodes[1].y.max(l.nodes[2].y),
            "join below both arms"
        );
    }

    #[test]
    fn loop_back_edge() {
        let l = check_invariants(
            &[(100., 40.), (100., 40.), (100., 40.), (100., 40.)],
            &[(0, 1), (1, 2), (2, 1), (2, 3)],
        );
        assert!(l.edges.iter().any(|e| e.from == 2 && e.to == 1 && e.back));
    }

    #[test]
    fn self_loop() {
        let l = check_invariants(&[(100., 40.), (100., 40.)], &[(0, 1), (1, 1)]);
        assert!(l.edges.iter().any(|e| e.from == 1 && e.to == 1 && e.back));
    }

    #[test]
    fn edge_back_into_entry() {
        check_invariants(&[(100., 40.), (100., 40.)], &[(0, 1), (1, 0)]);
    }

    #[test]
    fn duplicate_edges() {
        check_invariants(&[(100., 40.), (100., 40.)], &[(0, 1), (0, 1)]);
    }

    #[test]
    fn unreachable_blocks() {
        check_invariants(
            &[(100., 40.), (100., 40.), (90., 20.), (90., 20.)],
            &[(0, 1), (2, 3), (3, 1)],
        );
    }

    #[test]
    fn wide_fan_out() {
        let n = 40;
        let mut sizes = vec![(200., 50.)];
        let mut edges = Vec::new();
        for i in 1..=n {
            sizes.push((60. + (i % 7) as f32 * 30., 20. + (i % 3) as f32 * 15.));
            edges.push((0, i));
            edges.push((i, n + 1));
        }
        sizes.push((150., 30.));
        check_invariants(&sizes, &edges);
    }

    #[test]
    fn long_chain_with_skip() {
        // 0 -> 1 -> 2 -> 3 and a skip edge 0 -> 3.
        check_invariants(&[(80., 30.); 4], &[(0, 1), (1, 2), (2, 3), (0, 3)]);
    }

    #[test]
    fn no_nodes() {
        let l = layout(&[], &[]);
        assert!(l.nodes.is_empty());
    }
}
