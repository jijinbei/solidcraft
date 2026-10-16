//! Coverage certificates use boundary incidence, never summed overlapping area.
use crate::{spatial::Index, verify::Cell, *};
use solidcraft_geom::Vec3;
use std::collections::BTreeMap;

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
type Point = [Coord; 2];
fn xy(p: Vec3) -> Point {
    [Coord(p.x), Coord(p.y)]
}
fn orient(a: [f64; 2], b: [f64; 2], c: [f64; 2]) -> f64 {
    robust::orient2d(robust::Coord { x: a[0], y: a[1] }, robust::Coord { x: b[0], y: b[1] }, robust::Coord { x: c[0], y: c[1] })
}
fn corners(min: [f64; 2], max: [f64; 2]) -> [[f64; 2]; 4] {
    [min, [max[0], min[1]], max, [min[0], max[1]]]
}
fn predicate_coordinates(values: impl IntoIterator<Item = f64>) -> Result<()> {
    // Avoid underflow in adaptive predicate expansions. Such coordinates are
    // far below the supported machining error budget; reject, never snap.
    if values.into_iter().any(|v| v != 0. && v.abs() < 1e-100) {
        return invalid("comparison coordinates are below the predicate numeric range");
    }
    Ok(())
}

/// Reject overlaps and hostile intervals; certify exact rectangular tiling in
/// O(n log n). Missing strips of any width stay missing, with no area epsilon.
pub(crate) fn cells(cells: &[Cell], stock: Stock) -> Result<bool> {
    stock.validate()?;
    if cells.is_empty() || cells.len() > 1_000_000 {
        return invalid("invalid comparison cell count");
    }
    let mut events = Vec::with_capacity(cells.len() * 2);
    for (id, c) in cells.iter().enumerate() {
        for p in [Vec3::new(c.min[0], c.min[1], c.low), Vec3::new(c.max[0], c.max[1], c.high)] {
            model::point(p)?;
        }
        predicate_coordinates(c.min.into_iter().chain(c.max))?;
        if c.min[0] >= c.max[0]
            || c.min[1] >= c.max[1]
            || c.low > c.high
            || c.low < stock.min.z
            || c.high > stock.max.z
            || c.min[0] < stock.min.x
            || c.min[1] < stock.min.y
            || c.max[0] > stock.max.x
            || c.max[1] > stock.max.y
        {
            return invalid("comparison cell outside declared stock or invalid interval");
        }
        events.push((Coord(c.min[0]), true, id));
        events.push((Coord(c.max[0]), false, id));
    }
    events.sort_unstable();
    let mut active = BTreeMap::<Coord, Coord>::new();
    let mut gaps = 1isize;
    let mut previous = stock.min.x;
    let mut complete = true;
    for (at, start, id) in events {
        if at.0 > previous && gaps != 0 {
            complete = false;
        }
        previous = at.0;
        let c = &cells[id];
        let a = Coord(c.min[1]);
        let b = Coord(c.max[1]);
        if start {
            let left = active.range(..=a).next_back().map_or(Coord(stock.min.y), |(_, end)| *end);
            let right = active.range(a..).next().map_or(Coord(stock.max.y), |(start, _)| *start);
            if left > a || right < b {
                return invalid("overlapping comparison cells");
            }
            gaps += isize::from(left != a) + isize::from(b != right) - isize::from(left != right);
            active.insert(a, b);
        } else {
            if active.remove(&a) != Some(b) {
                return invalid("invalid comparison cell sweep");
            }
            let left = active.range(..a).next_back().map_or(Coord(stock.min.y), |(_, end)| *end);
            let right = active.range(a..).next().map_or(Coord(stock.max.y), |(start, _)| *start);
            gaps += isize::from(left != right) - isize::from(left != a) - isize::from(b != right);
        }
    }
    Ok(complete && previous == stock.max.x)
}

pub(crate) struct MeshField<'a> {
    triangles: &'a [[Vec3; 3]],
    index: Index,
    boundary: Vec<([f64; 2], [f64; 2])>,
    boundary_index: Index,
    stock: Stock,
    guard: f64,
}
impl<'a> MeshField<'a> {
    pub(crate) fn new(triangles: &'a [[Vec3; 3]], stock: Stock) -> Result<Self> {
        stock.validate()?;
        crate::stock_mesh::validate_triangles(triangles)?;
        let mut edges = BTreeMap::<(Point, Point), i32>::new();
        let mut items = Vec::new();
        let extent = stock.min.len().max(stock.max.len()).max(1.);
        for (id, t) in triangles.iter().enumerate() {
            for p in t {
                predicate_coordinates([p.x, p.y])?;
                if p.x < stock.min.x || p.y < stock.min.y || p.z < stock.min.z || p.x > stock.max.x || p.y > stock.max.y || p.z > stock.max.z {
                    return invalid("reference stock mesh exceeds declared stock; check units and frame");
                }
            }
            let p = t.map(|v| [v.x, v.y]);
            let orientation = orient(p[0], p[1], p[2]);
            if orientation < 0. && t.iter().any(|p| p.z != stock.min.z) {
                return invalid("reference stock is not a bottom-supported vertical height field");
            }
            if orientation <= 0. {
                continue;
            }
            let min = t.iter().copied().fold(t[0], Vec3::min);
            let max = t.iter().copied().fold(t[0], Vec3::max);
            items.push((id, [min.x, min.y], [max.x, max.y]));
            for (a, b) in [(xy(t[0]), xy(t[1])), (xy(t[1]), xy(t[2])), (xy(t[2]), xy(t[0]))] {
                let (key, sign) = if a < b { ((a, b), 1) } else { ((b, a), -1) };
                *edges.entry(key).or_default() += sign;
            }
        }
        let boundary: Vec<_> = edges.into_iter().filter(|(_, n)| *n != 0).map(|((a, b), _)| (a.map(|v| v.0), b.map(|v| v.0))).collect();
        let boundary_index = Index::new(
            boundary.iter().enumerate().map(|(id, (a, b))| (id, [a[0].min(b[0]), a[1].min(b[1])], [a[0].max(b[0]), a[1].max(b[1])])).collect(),
        );
        Ok(Self { triangles, index: Index::new(items), boundary, boundary_index, stock, guard: 128. * f64::EPSILON * extent })
    }
    /// Bounds the maximum upward surface everywhere in a rectangle, including
    /// holes (height = stock bottom). Shared projected edges cancel exactly;
    /// unmatched edges crossing the interior forbid a full-coverage certificate.
    pub(crate) fn bounds(&self, min: [f64; 2], max: [f64; 2], work: &mut usize, budget: usize) -> Option<[f64; 2]> {
        let rectangle = corners(min, max);
        let center = [(min[0] + max[0]) / 2., (min[1] + max[1]) / 2.];
        let mut ids = Vec::new();
        self.boundary_index.query(min, max, &mut ids);
        let mut crossed = false;
        for id in ids {
            *work += 1;
            if *work > budget {
                return None;
            }
            let (a, b) = self.boundary[id];
            if (0..2).any(|k| a[k].max(b[k]) <= min[k] || a[k].min(b[k]) >= max[k]) {
                continue;
            }
            let signs = rectangle.map(|c| orient(a, b, c));
            if signs.iter().any(|v| *v < 0.) && signs.iter().any(|v| *v > 0.) {
                crossed = true;
                break;
            }
        }
        let mut ids = Vec::new();
        self.index.query(min, max, &mut ids);
        let mut low = f64::INFINITY;
        let mut high = self.stock.min.z;
        let mut inside = false;
        for id in ids {
            *work += 1;
            if *work > budget {
                return None;
            }
            let t = &self.triangles[id];
            let p = t.map(|p| [p.x, p.y]);
            // These cells form a tiling whose closed columns are united later.
            // A triangle touching only a cell edge belongs to the adjacent cell;
            // including its height here creates false interior discontinuities.
            // Positive-area overlap is decided with exact orientation predicates.
            if (0..2).any(|k| p.iter().all(|v| v[k] <= min[k]) || p.iter().all(|v| v[k] >= max[k])) {
                continue;
            }
            let edges = [(p[0], p[1]), (p[1], p[2]), (p[2], p[0])];
            if edges.iter().any(|(a, b)| rectangle.iter().all(|c| orient(*a, *b, *c) <= 0.)) {
                continue;
            }
            inside |= edges.iter().all(|(a, b)| orient(*a, *b, center) >= 0.);
            let mut lo = t.iter().map(|v| v.z).fold(f64::INFINITY, f64::min);
            let mut hi = t.iter().map(|v| v.z).fold(f64::NEG_INFINITY, f64::max);
            let n = (t[1] - t[0]).cross(t[2] - t[0]);
            let normal_error = 4. * self.guard * (1. + (t[1] - t[0]).len() + (t[2] - t[0]).len());
            if n.z > normal_error {
                let slope = [n.x / n.z, n.y / n.z];
                let slope_error = slope.map(|s| normal_error * (1. + s.abs()) / (n.z - normal_error));
                let error = self.guard * (1. + slope[0].abs() + slope[1].abs())
                    + slope_error[0] * (t[0].x - min[0]).abs().max((t[0].x - max[0]).abs())
                    + slope_error[1] * (t[0].y - min[1]).abs().max((t[0].y - max[1]).abs());
                let z = rectangle.map(|p| t[0].z + slope[0] * (t[0].x - p[0]) + slope[1] * (t[0].y - p[1]));
                if error.is_finite() && z.iter().all(|v| v.is_finite()) {
                    lo = lo.max(z.iter().copied().fold(f64::INFINITY, f64::min) - error);
                    hi = hi.min(z.iter().copied().fold(f64::NEG_INFINITY, f64::max) + error);
                }
            }
            low = low.min(lo);
            high = high.max(hi);
        }
        if !inside || crossed {
            low = self.stock.min.z;
        }
        Some([(low - self.guard).max(self.stock.min.z), (high + self.guard).min(self.stock.max.z)])
    }
}
