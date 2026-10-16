//! Conservative boundary extraction from non-overlapping rectangular height cells.
//! Shared planar edges and vertical steps are split before triangulation. No
//! XY snapping, interpolation or generation algorithm is involved. Tiny adaptive
//! saddle contacts may be filled upward within a declared export error budget.
use crate::{
    Error, Result, invalid,
    verify::{Cell, Report},
};
use solidcraft_geom::Vec3;
use std::{
    collections::{BTreeMap, BTreeSet},
    future::Future,
};

/// Count elementary topology work, including sorting and manifold validation.
/// A yield callback which is immediately ready retains the synchronous API.
struct Work<C, Y> {
    cancelled: C,
    yield_control: Y,
    remaining: usize,
}
impl<C: Fn() -> bool, Y: FnMut() -> F, F: Future<Output = ()>> Work<C, Y> {
    async fn tick(&mut self) -> Result<()> {
        if self.remaining == 0 {
            if (self.cancelled)() {
                return invalid("stock extraction cancelled");
            }
            (self.yield_control)().await;
            if (self.cancelled)() {
                return invalid("stock extraction cancelled");
            }
            self.remaining = 256;
        }
        self.remaining -= 1;
        Ok(())
    }
    // Sort small runs, then merge with checkpoints. A single standard-library
    // sort of up to two million cell events otherwise monopolizes a browser poll.
    async fn sort<T: Clone>(&mut self, values: &mut Vec<T>, compare: impl Fn(&T, &T) -> std::cmp::Ordering) -> Result<()> {
        for run in values.chunks_mut(256) {
            if run.len() == 256 {
                self.remaining = 0;
            }
            self.tick().await?;
            run.sort_unstable_by(&compare);
        }
        if values.len() <= 256 {
            return Ok(());
        }
        let mut scratch = Vec::with_capacity(values.len());
        let mut width = 256;
        while width < values.len() {
            for begin in (0..values.len()).step_by(width * 2) {
                let middle = (begin + width).min(values.len());
                let end = (middle + width).min(values.len());
                let (mut a, mut b) = (begin, middle);
                while a < middle || b < end {
                    self.tick().await?;
                    let take_a = b == end || (a < middle && compare(&values[a], &values[b]).is_le());
                    let index = if take_a {
                        let i = a;
                        a += 1;
                        i
                    } else {
                        let i = b;
                        b += 1;
                        i
                    };
                    scratch.push(values[index].clone());
                }
            }
            std::mem::swap(values, &mut scratch);
            scratch.clear();
            width *= 2;
        }
        Ok(())
    }
}
fn ready<T>(future: impl Future<Output = Result<T>>) -> Result<T> {
    let mut future = std::pin::pin!(future);
    match future.as_mut().poll(&mut std::task::Context::from_waker(std::task::Waker::noop())) {
        std::task::Poll::Ready(result) => result,
        std::task::Poll::Pending => invalid("synchronous stock extraction unexpectedly yielded"),
    }
}

const MAX_CELLS: usize = 60_000;
const MAX_TRIANGLES: usize = 500_000;
// One percent of the post budget, reserved only for conservative saddle closure.
// This is an artifact approximation; it never changes the verification report.
const MAX_SADDLE_FILL: f64 = crate::STOCK_EXPORT_ERROR;

#[derive(Clone, Copy, Debug, PartialEq)]
struct Coord(f64);
impl Eq for Coord {}
impl PartialOrd for Coord {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}
impl Ord for Coord {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        if self.0 == other.0 { std::cmp::Ordering::Equal } else { self.0.total_cmp(&other.0) }
    }
}
type Point = [Coord; 3];
type XY = [Coord; 2];
fn point(p: Vec3) -> Point {
    [Coord(p.x), Coord(p.y), Coord(p.z)]
}

#[derive(Clone, Copy)]
struct Event {
    at: Coord,
    cell: usize,
    side: usize,
    start: bool,
}
struct Span {
    axis: usize,
    fixed: f64,
    a: f64,
    b: f64,
    cells: [Option<usize>; 2],
}
impl Span {
    fn xy(&self, t: f64) -> XY {
        if self.axis == 0 { [Coord(self.fixed), Coord(t)] } else { [Coord(t), Coord(self.fixed)] }
    }
    fn vertex(&self, t: f64, z: f64) -> Vec3 {
        let xy = self.xy(t);
        Vec3::new(xy[0].0, xy[1].0, z)
    }
}

async fn cells<C: Fn() -> bool, Y: FnMut() -> F, F: Future<Output = ()>>(report: &Report, bottom: f64, work: &mut Work<C, Y>) -> Result<Vec<Cell>> {
    work.tick().await?;
    if !bottom.is_finite() || report.cells.is_empty() || report.cells.len() > 1_000_000 {
        return invalid("invalid stock export bounds");
    }
    if !report.stock_complete || report.findings.iter().any(|f| f.kind == "budget_exhausted") {
        return invalid("stock verification is incomplete; cannot export remaining stock");
    }
    for c in &report.cells {
        work.tick().await?;
        for p in [Vec3::new(c.min[0], c.min[1], bottom), Vec3::new(c.max[0], c.max[1], c.high)] {
            crate::model::point(p)?;
        }
        if c.min[0] >= c.max[0] || c.min[1] >= c.max[1] || !c.low.is_finite() || c.low > c.high || c.high < bottom {
            return invalid("invalid stock cell");
        }
    }
    // Reject overlap in O(n log n). End events precede starts at the same X.
    let mut events = Vec::with_capacity(report.cells.len() * 2);
    for (i, c) in report.cells.iter().enumerate() {
        work.tick().await?;
        events.push((Coord(c.min[0]), true, i));
        events.push((Coord(c.max[0]), false, i));
    }
    work.sort(&mut events, Ord::cmp).await?;
    let mut active = BTreeMap::<Coord, f64>::new();
    for (_, start, i) in events {
        work.tick().await?;
        let c = &report.cells[i];
        let y = Coord(c.min[1]);
        if start {
            if active.range(..=y).next_back().is_some_and(|(_, end)| *end > c.min[1])
                || active.range(y..).next().is_some_and(|(start, _)| start.0 < c.max[1])
            {
                return invalid("overlapping stock cells");
            }
            active.insert(y, c.max[1]);
        } else {
            active.remove(&y);
        }
    }
    // Exact coplanar rectangle merging reduces output without changing height.
    let mut cells = Vec::new();
    for c in &report.cells {
        work.tick().await?;
        if c.high > bottom {
            cells.push(c.clone());
        }
    }
    for _ in 0..24 {
        let before = cells.len();
        for axis in 0..2 {
            let other = 1 - axis;
            work.sort(&mut cells, |a, b| {
                a.min[other]
                    .total_cmp(&b.min[other])
                    .then(a.max[other].total_cmp(&b.max[other]))
                    .then(a.high.total_cmp(&b.high))
                    .then(a.min[axis].total_cmp(&b.min[axis]))
            })
            .await?;
            let mut merged: Vec<Cell> = Vec::new();
            for c in cells {
                work.tick().await?;
                if let Some(last) = merged.last_mut()
                    && last.min[other] == c.min[other]
                    && last.max[other] == c.max[other]
                    && last.high == c.high
                    && last.max[axis] == c.min[axis]
                {
                    last.max[axis] = c.max[axis];
                } else {
                    merged.push(c);
                }
            }
            cells = merged;
        }
        if cells.len() == before {
            break;
        }
    }
    if cells.len() > MAX_CELLS {
        return invalid(format!("stock mesh has {} merged cells, exceeding 60000; use validation JSON for this resolution", cells.len()));
    }
    Ok(cells)
}

async fn spans<C: Fn() -> bool, Y: FnMut() -> F, F: Future<Output = ()>>(cells: &[Cell], work: &mut Work<C, Y>) -> Result<Vec<Span>> {
    let mut lines: BTreeMap<(usize, Coord), Vec<Event>> = BTreeMap::new();
    for (i, c) in cells.iter().enumerate() {
        work.tick().await?;
        for axis in 0..2 {
            for (fixed, side) in [(c.min[axis], 1), (c.max[axis], 0)] {
                let events = lines.entry((axis, Coord(fixed))).or_default();
                events.push(Event { at: Coord(c.min[1 - axis]), cell: i, side, start: true });
                events.push(Event { at: Coord(c.max[1 - axis]), cell: i, side, start: false });
            }
        }
    }
    let mut spans = Vec::new();
    for ((axis, fixed), mut events) in lines {
        work.sort(&mut events, |a, b| (a.at, a.start).cmp(&(b.at, b.start))).await?;
        let mut active = [None; 2];
        let mut previous: Option<Coord> = None;
        for event in events {
            work.tick().await?;
            if let Some(a) = previous
                && a < event.at
                && active != [None; 2]
            {
                spans.push(Span { axis, fixed: fixed.0, a: a.0, b: event.at.0, cells: active });
            }
            previous = Some(event.at);
            if event.start {
                if active[event.side].is_some() {
                    return invalid("overlapping stock boundaries");
                }
                active[event.side] = Some(event.cell);
            } else {
                active[event.side] = None;
            }
        }
    }
    Ok(spans)
}

async fn close_tiny_saddles<C: Fn() -> bool, Y: FnMut() -> F, F: Future<Output = ()>>(
    cells: &mut [Cell],
    spans: &[Span],
    bottom: f64,
    work: &mut Work<C, Y>,
) -> Result<()> {
    // NW, NE, SE, SW occupancy at every boundary endpoint, including T-junctions.
    let mut vertices = BTreeMap::<XY, [Option<usize>; 4]>::new();
    for s in spans {
        for (side, id) in s.cells.iter().enumerate() {
            work.tick().await?;
            let Some(id) = id else { continue };
            let (a, b) = match (s.axis, side) {
                (0, 0) => (0, 3),
                (0, 1) => (1, 2),
                (1, 0) => (2, 3),
                _ => (1, 0),
            };
            vertices.entry(s.xy(s.a)).or_insert([None; 4])[a] = Some(*id);
            vertices.entry(s.xy(s.b)).or_insert([None; 4])[b] = Some(*id);
        }
    }
    let mut ceilings = Vec::with_capacity(cells.len());
    for c in cells.iter() {
        work.tick().await?;
        ceilings.push(c.high + MAX_SADDLE_FILL);
    }
    // Monotone bounded closure. Stop rather than silently exceeding either the
    // error or work bound; the final manifold validator rejects unresolved cases.
    for _ in 0..8 {
        let mut changed = false;
        for ids in vertices.values() {
            work.tick().await?;
            let heights = ids.map(|id| id.map_or(bottom, |i| cells[i].high));
            for (upper, lower) in [([0, 2], [1, 3]), ([1, 3], [0, 2])] {
                let threshold = heights[upper[0]].min(heights[upper[1]]);
                let pick = if heights[lower[0]] >= heights[lower[1]] { lower[0] } else { lower[1] };
                if threshold > heights[pick]
                    && let Some(id) = ids[pick]
                    && threshold <= ceilings[id]
                {
                    cells[id].high = threshold;
                    changed = true;
                }
            }
        }
        if !changed {
            break;
        }
    }
    for _ in vertices {
        work.tick().await?;
    }
    Ok(())
}

pub(crate) fn validate_triangles(triangles: &[[Vec3; 3]]) -> Result<()> {
    ready(validate(triangles, &mut Work { cancelled: || false, yield_control: || std::future::ready(()), remaining: 0 }))
}
async fn validate<C: Fn() -> bool, Y: FnMut() -> F, F: Future<Output = ()>>(triangles: &[[Vec3; 3]], work: &mut Work<C, Y>) -> Result<()> {
    if triangles.len() > MAX_TRIANGLES {
        return invalid("stock triangle budget exceeded");
    }
    for t in triangles {
        work.tick().await?;
        for p in t {
            crate::model::point(*p)?;
        }
        if (t[1] - t[0]).cross(t[2] - t[0]).len2() == 0. {
            return invalid("degenerate stock triangle");
        }
    }

    // Edge incidence plus connected vertex links reject cracks, T-junctions,
    // touching components and non-manifold saddle edges explicitly.
    let mut edges = BTreeMap::<(Point, Point), (usize, i32)>::new();
    let mut links = BTreeMap::<Point, Vec<(Point, Point)>>::new();
    for t in triangles {
        work.tick().await?;
        let p = t.map(point);
        for (a, b, c) in [(p[0], p[1], p[2]), (p[1], p[2], p[0]), (p[2], p[0], p[1])] {
            let (key, sign) = if a < b { ((a, b), 1) } else { ((b, a), -1) };
            let edge = edges.entry(key).or_default();
            edge.0 += 1;
            edge.1 += sign;
            if edge.0 > 2 {
                return invalid(format!("stock boundary is non-manifold at {key:?}; use validation JSON"));
            }
            links.entry(a).or_default().push((b, c));
        }
    }
    for (_, edge) in edges {
        work.tick().await?;
        if edge != (2, 0) {
            return invalid("stock boundary has an unstitched or misoriented edge");
        }
    }
    // Consume the maps incrementally too: dropping a large tree all at once
    // can be as visible as constructing it on the browser's event loop.
    for (_, link) in links {
        let mut next = BTreeMap::new();
        for (a, b) in &link {
            work.tick().await?;
            if next.insert(*a, *b).is_some() {
                return invalid("non-manifold stock vertex");
            }
        }
        let Some((&start, _)) = next.first_key_value() else {
            continue;
        };
        let mut at = start;
        for _ in 0..link.len() {
            work.tick().await?;
            let Some(b) = next.remove(&at) else {
                return invalid("disconnected stock vertex surface; use validation JSON");
            };
            at = b;
        }
        if at != start || !next.is_empty() {
            return invalid("open stock vertex surface");
        }
    }
    Ok(())
}

struct Mesh {
    triangles: Vec<[Vec3; 3]>,
}
impl Mesh {
    fn triangle(&mut self, a: Vec3, b: Vec3, c: Vec3) -> Result<()> {
        if self.triangles.len() >= MAX_TRIANGLES {
            return invalid("stock mesh exceeds 500000 triangles");
        }
        if (b - a).cross(c - a).len2() == 0.0 {
            return invalid("degenerate stock boundary triangle");
        }
        self.triangles.push([a, b, c]);
        Ok(())
    }
    async fn convex<C: Fn() -> bool, Y: FnMut() -> F, F: Future<Output = ()>>(
        &mut self,
        boundary: &[Vec3],
        reverse: bool,
        work: &mut Work<C, Y>,
    ) -> Result<()> {
        // All faces are rectangles with collinear boundary subdivisions. Clip
        // the smallest nonzero corner first, preserving every boundary vertex.
        // An ordered corner set bounds this to O(n log n), including long edges.
        let n = boundary.len();
        if !(3..=MAX_TRIANGLES).contains(&n) {
            return invalid("invalid stock polygon size");
        }
        let mut prev: Vec<_> = (0..n).map(|i| (i + n - 1) % n).collect();
        let mut next: Vec<_> = (0..n).map(|i| (i + 1) % n).collect();
        let area = |a: usize, b: usize, c: usize| Coord((boundary[b] - boundary[a]).cross(boundary[c] - boundary[b]).len2());
        let mut corners = BTreeSet::new();
        for i in 0..n {
            work.tick().await?;
            let a = area(prev[i], i, next[i]);
            if a.0 > 0.0 {
                corners.insert((a, i));
            }
        }
        for _ in 0..n - 2 {
            work.tick().await?;
            let (_, i) = corners.pop_first().ok_or_else(|| Error("degenerate stock polygon".into()))?;
            let a = prev[i];
            let b = next[i];
            if reverse {
                self.triangle(boundary[a], boundary[b], boundary[i])?;
            } else {
                self.triangle(boundary[a], boundary[i], boundary[b])?;
            }
            corners.remove(&(area(prev[a], a, next[a]), a));
            corners.remove(&(area(prev[b], b, next[b]), b));
            next[a] = b;
            prev[b] = a;
            for k in [a, b] {
                let v = area(prev[k], k, next[k]);
                if v.0 > 0.0 {
                    corners.insert((v, k));
                }
            }
        }
        Ok(())
    }
}

async fn boundary<C: Fn() -> bool, Y: FnMut() -> F, F: Future<Output = ()>>(report: &Report, bottom: f64, work: &mut Work<C, Y>) -> Result<Mesh> {
    let mut cells = cells(report, bottom, work).await?;
    let spans = spans(&cells, work).await?;
    close_tiny_saddles(&mut cells, &spans, bottom, work).await?;
    let mut levels = BTreeMap::<XY, BTreeSet<Coord>>::new();
    for s in &spans {
        work.tick().await?;
        for xy in [s.xy(s.a), s.xy(s.b)] {
            let z = levels.entry(xy).or_default();
            z.insert(Coord(bottom));
            for id in s.cells.into_iter().flatten() {
                z.insert(Coord(cells[id].high));
            }
        }
    }
    let mut mesh = Mesh { triangles: Vec::new() };
    let mut tops: Vec<Vec<(usize, Coord, Vec3)>> = vec![Vec::new(); cells.len()];
    for s in &spans {
        work.tick().await?;
        for (side, id) in s.cells.iter().enumerate() {
            let Some(id) = id else {
                continue;
            };
            let c = &cells[*id];
            let edge = match (s.axis, side) {
                (1, 1) => 0,
                (0, 0) => 1,
                (1, 0) => 2,
                _ => 3,
            };
            let a = if edge < 2 { s.a } else { s.b };
            tops[*id].push((edge, Coord(if edge < 2 { a } else { -a }), s.vertex(a, c.high)));
        }
        let heights = s.cells.map(|id| id.map_or(bottom, |id| cells[id].high));
        if heights[0] == heights[1] {
            continue;
        }
        let lo = heights[0].min(heights[1]);
        let hi = heights[0].max(heights[1]);
        let mut polygon = vec![s.vertex(s.a, lo), s.vertex(s.b, lo)];
        let right = levels.get(&s.xy(s.b)).ok_or_else(|| Error("missing stock edge levels".into()))?;
        for z in right.range((std::ops::Bound::Excluded(Coord(lo)), std::ops::Bound::Excluded(Coord(hi)))) {
            work.tick().await?;
            polygon.push(s.vertex(s.b, z.0));
        }
        polygon.push(s.vertex(s.b, hi));
        polygon.push(s.vertex(s.a, hi));
        let left = levels.get(&s.xy(s.a)).ok_or_else(|| Error("missing stock edge levels".into()))?;
        for z in left.range((std::ops::Bound::Excluded(Coord(lo)), std::ops::Bound::Excluded(Coord(hi)))).rev() {
            work.tick().await?;
            polygon.push(s.vertex(s.a, z.0));
        }
        mesh.convex(&polygon, (s.axis == 0) != (heights[0] > heights[1]), work).await?;
    }
    for mut top in tops {
        work.sort(&mut top, |a, b| (a.0, a.1).cmp(&(b.0, b.1))).await?;
        let mut polygon: Vec<_> = top.into_iter().map(|p| p.2).collect();
        mesh.convex(&polygon, false, work).await?;
        for p in &mut polygon {
            p.z = bottom;
        }
        mesh.convex(&polygon, true, work).await?;
    }
    for _ in levels {
        work.tick().await?;
    }
    validate(&mesh.triangles, work).await?;
    Ok(mesh)
}

pub(crate) fn triangles(report: &Report, bottom: f64) -> Result<Vec<[Vec3; 3]>> {
    ready(triangles_cooperative(report, bottom, || false, || std::future::ready(())))
}
pub(crate) async fn triangles_cooperative<F: Future<Output = ()>>(
    report: &Report,
    bottom: f64,
    cancelled: impl Fn() -> bool,
    yield_control: impl FnMut() -> F,
) -> Result<Vec<[Vec3; 3]>> {
    let mut work = Work { cancelled, yield_control, remaining: 0 };
    Ok(boundary(report, bottom, &mut work).await?.triangles)
}

pub(crate) fn stl(report: &Report, bottom: f64) -> Result<String> {
    ready(stl_cooperative(report, bottom, || false, || std::future::ready(())))
}
pub(crate) async fn stl_cooperative<F: Future<Output = ()>>(
    report: &Report,
    bottom: f64,
    cancelled: impl Fn() -> bool,
    yield_control: impl FnMut() -> F,
) -> Result<String> {
    let mut work = Work { cancelled, yield_control, remaining: 0 };
    let mesh = boundary(report, bottom, &mut work).await?.triangles;
    let mut out = String::from("solid cam_remaining_stock\n");
    for triangle in mesh {
        work.tick().await?;
        out.push_str("facet normal 0 0 0\nouter loop\n");
        for p in triangle {
            out.push_str(&format!("vertex {} {} {}\n", p.x, p.y, p.z));
        }
        out.push_str("endloop\nendfacet\n");
        if out.len() > 128 * 1024 * 1024 {
            return invalid("stock mesh exceeds 128 MiB");
        }
    }
    out.push_str("endsolid cam_remaining_stock\n");
    work.remaining = 0;
    work.tick().await?;
    Ok(out)
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::verify::Status;
    fn cell(min: [f64; 2], max: [f64; 2], high: f64) -> Cell {
        Cell { min, max, low: high, high, target_low: high, target_high: high }
    }
    fn report(cells: Vec<Cell>) -> Report {
        Report {
            version: crate::VERSION.into(),
            fingerprint: "analytic".into(),
            status: Status::Pass,
            finished_dimensions: Status::Pass,
            post_version: "analytic".into(),
            tolerance: 0.01,
            uncertainty: 0.0025,
            maximum_height_interval: 0.0,
            stock_complete: true,
            stock: None,
            cells,
            findings: vec![],
            cutting_distance: 0.0,
            rapid_distance: 0.0,
            cutting_minutes: 0.0,
            remaining_volume_low: 0.0,
            remaining_volume_high: 0.0,
            work: 0,
        }
    }
    fn check(cells: Vec<Cell>, volume: f64, area: f64) -> Vec<[Vec3; 3]> {
        let text = stl(&report(cells), 0.0).unwrap();
        let mesh = crate::compare::read_stl(text.as_bytes()).unwrap();
        let v: f64 = mesh.iter().map(|t| t[0].dot(t[1].cross(t[2])) / 6.0).sum();
        let a: f64 = mesh.iter().map(|t| (t[1] - t[0]).cross(t[2] - t[0]).len() / 2.0).sum();
        assert!((v - volume).abs() < 1e-9, "volume {v} expected {volume}");
        assert!((a - area).abs() < 1e-9, "area {a} expected {area}");
        mesh
    }
    #[test]
    fn box_and_step_have_only_exterior_faces() {
        check(vec![cell([0., 0.], [1., 1.], 1.)], 1., 6.);
        let mesh = check(vec![cell([0., 0.], [1., 1.], 1.), cell([1., 0.], [2., 1.], 2.)], 3., 14.);
        for t in mesh {
            if t.iter().all(|p| p.x == 1.) {
                assert!(t.iter().all(|p| p.z >= 1.));
            }
        }
    }
    #[test]
    fn adaptive_t_junctions_and_three_height_vertices_are_stitched() {
        check(vec![cell([0., 0.], [1., 2.], 1.), cell([1., 0.], [2., 1.], 2.), cell([1., 1.], [2., 2.], 3.)], 7., 26.);
    }
    #[test]
    fn hole_and_disconnected_stock_are_watertight() {
        let mut cells = Vec::new();
        for x in 0..3 {
            for y in 0..3 {
                cells.push(cell([x as f64, y as f64], [(x + 1) as f64, (y + 1) as f64], if x == 1 && y == 1 { 0. } else { 1. }));
            }
        }
        check(cells, 8., 32.);
        check(vec![cell([0., 0.], [1., 1.], 1.), cell([2., 0.], [3., 1.], 1.)], 2., 12.);
    }
    #[test]
    fn nonmanifold_touching_stock_is_explicitly_rejected() {
        assert!(stl(&report(vec![cell([0., 0.], [1., 1.], 1.), cell([1., 1.], [2., 2.], 1.)]), 0.).is_err());
        assert!(
            stl(
                &report(vec![cell([0., 0.], [1., 1.], 2.), cell([1., 0.], [2., 1.], 1.), cell([0., 1.], [1., 2.], 1.), cell([1., 1.], [2., 2.], 2.)]),
                0.
            )
            .is_err()
        );
    }
    #[test]
    fn tiny_adaptive_saddle_closure_is_conservative_and_bounded() {
        let cells =
            vec![cell([0., 0.], [1., 1.], 2.), cell([1., 0.], [2., 1.], 2. - 4e-6), cell([0., 1.], [1., 2.], 1.), cell([1., 1.], [2., 2.], 2.)];
        let mut original = report(cells);
        original.stock = Some(crate::Stock { min: Vec3::new(0., 0., 0.), max: Vec3::new(2., 2., 2.) });
        let mesh = triangles(&original, 0.).unwrap();
        let volume: f64 = mesh.iter().map(|t| t[0].dot(t[1].cross(t[2])) / 6.).sum();
        assert!((volume - 7.).abs() < 1e-12);
        assert_eq!(original.cells[1].high, 2. - 4e-6);
        assert_eq!(crate::compare::stock_mesh(&original, &mesh, 0., crate::TOLERANCE).unwrap().status, Status::Pass);
        let mut too_large = original;
        too_large.cells[1].high = 2. - 2. * MAX_SADDLE_FILL;
        assert!(triangles(&too_large, 0.).is_err());
    }
    #[test]
    fn hostile_and_overlapping_cells_are_rejected() {
        for bad in [cell([0., 0.], [0., 1.], 1.), cell([0., 0.], [1., 1.], f64::NAN), cell([0., 0.], [1., 1.], -1.)] {
            assert!(stl(&report(vec![bad]), 0.).is_err());
        }
        assert!(stl(&report(vec![cell([0., 0.], [2., 2.], 1.), cell([1., 1.], [3., 3.], 2.)]), 0.).is_err());
        assert!(stl(&report(vec![cell([0., 0.], [2., 2.], 1.), cell([0., 0.], [2., 2.], 1.)]), 0.).is_err());
    }
    #[test]
    fn zero_stock_and_translated_base_are_preserved() {
        assert!(stl(&report(vec![cell([0., 0.], [1., 1.], 0.)]), 0.).unwrap().lines().count() == 2);
        let mesh = triangles(&report(vec![cell([-3., 2.], [-2., 3.], 7.)]), 5.).unwrap();
        let v: f64 = mesh.iter().map(|t| t[0].dot(t[1].cross(t[2])) / 6.0).sum();
        assert!((v - 2.).abs() < 1e-9);
    }
    #[test]
    fn incomplete_verification_cannot_export_partial_stock() {
        let input = crate::corpus::fixture("face").unwrap();
        let path = crate::generate::generate(input.clone()).unwrap();
        let nc = crate::nc::round_trip(&path, &input.setup, &input.tools).unwrap();
        for limits in [crate::verify::Limits { cells: 1, ..Default::default() }, crate::verify::Limits { work: 1, ..Default::default() }] {
            let report = crate::verify::simulate(&input, &nc, limits).unwrap();
            assert!(!report.stock_complete);
            assert!(stl(&report, 0.).is_err());
        }
    }
    #[test]
    fn cooperative_mesh_matches_sync_and_cancels_during_topology_work() {
        use std::task::{Context, Poll, Waker};
        let report = report(
            (0..32)
                .rev()
                .flat_map(|x| (0..32).rev().map(move |y| cell([x as f64, y as f64], [(x + 1) as f64, (y + 1) as f64], (x + y + 1) as f64)))
                .collect(),
        );
        let expected = triangles(&report, 0.).unwrap();
        let yield_once = || {
            let mut first = true;
            std::future::poll_fn(move |_| if std::mem::take(&mut first) { Poll::Pending } else { Poll::Ready(()) })
        };
        let mut future = std::pin::pin!(triangles_cooperative(&report, 0., || false, yield_once));
        let mut cx = Context::from_waker(Waker::noop());
        let mut polls = 0;
        loop {
            polls += 1;
            if let Poll::Ready(result) = future.as_mut().poll(&mut cx) {
                assert_eq!(result.unwrap(), expected);
                break;
            }
            assert!(polls < 100_000);
        }
        assert!(polls > 100);
        // Cancel in cell sorting, boundary work and late manifold validation.
        // Each suspended future must fail on its next poll without a partial mesh.
        for cancel_after in [1, polls / 2, polls - 1] {
            let cancelled = std::cell::Cell::new(false);
            let mut future = std::pin::pin!(triangles_cooperative(&report, 0., || cancelled.get(), yield_once));
            for _ in 0..cancel_after {
                assert!(future.as_mut().poll(&mut cx).is_pending());
            }
            cancelled.set(true);
            let result = future.as_mut().poll(&mut cx);
            assert!(matches!(result, Poll::Ready(Err(e)) if e.0.contains("cancelled")));
        }
    }
    #[test]
    fn stepped_grid_preserves_volume_and_surface_area() {
        let n = 16;
        let mut cells = Vec::new();
        for x in 0..n {
            for y in 0..n {
                cells.push(cell([x as f64, y as f64], [(x + 1) as f64, (y + 1) as f64], (x + y + 1) as f64));
            }
        }
        check(cells, (n * n * n) as f64, (6 * n * n + 2 * n * (n - 1)) as f64);
    }
}
