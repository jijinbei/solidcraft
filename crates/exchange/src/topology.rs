//! A body's B-rep edges and corners as snapshot data: names, exact curves, polylines, and the
//! chains of mesh vertices that trace them.

use std::collections::BTreeMap;

use solidcraft_geom::Vec3;

use crate::schema::{Curve, Edge, Vertex};

/// Most edge ends a body's corners are found among (a body with more lists no corners).
const MAX_EDGE_ENDS: usize = 200_000;

/// Mesh vertices by position, for finding the one at a point.
pub(crate) struct PointIndex<'a> {
    positions: &'a [[f64; 3]],
    cell: f64,
    cells: BTreeMap<[i64; 3], Vec<u32>>,
}

impl<'a> PointIndex<'a> {
    /// `positions` with points within `close` of each other found as the same.
    pub(crate) fn new(positions: &'a [[f64; 3]], close: f64) -> Self {
        let cell = close.max(f64::MIN_POSITIVE) * 2.0;
        let mut cells: BTreeMap<[i64; 3], Vec<u32>> = BTreeMap::new();
        for (i, p) in positions.iter().enumerate() {
            if let Ok(i) = u32::try_from(i) {
                cells.entry(key(*p, cell)).or_default().push(i);
            }
        }
        PointIndex { positions, cell, cells }
    }

    /// The mesh vertex nearest `p` within the closeness given, if any.
    pub(crate) fn find(&self, p: [f64; 3]) -> Option<u32> {
        let k = key(p, self.cell);
        let mut best: Option<(f64, u32)> = None;
        for dx in -1..=1 {
            for dy in -1..=1 {
                for dz in -1..=1 {
                    let Some(list) = self.cells.get(&[k[0] + dx, k[1] + dy, k[2] + dz]) else { continue };
                    for &i in list {
                        let Some(q) = self.positions.get(i as usize) else { continue };
                        let d = dist(p, *q);
                        if d <= self.cell * 0.5 && best.is_none_or(|(bd, bi)| d < bd || (d == bd && i < bi)) {
                            best = Some((d, i));
                        }
                    }
                }
            }
        }
        best.map(|(_, i)| i)
    }
}

fn key(p: [f64; 3], cell: f64) -> [i64; 3] {
    p.map(|c| (c / cell).floor() as i64)
}

fn dist(a: [f64; 3], b: [f64; 3]) -> f64 {
    ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)).sqrt()
}

fn v3(v: Vec3) -> [f64; 3] {
    [v.x + 0.0, v.y + 0.0, v.z + 0.0]
}

/// Distance from `p` to the polyline `line`.
fn to_polyline(p: [f64; 3], line: &[[f64; 3]]) -> f64 {
    let seg = |a: [f64; 3], b: [f64; 3]| {
        let d = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
        let l2 = d[0] * d[0] + d[1] * d[1] + d[2] * d[2];
        let t = if l2 > 0.0 { (((p[0] - a[0]) * d[0] + (p[1] - a[1]) * d[1] + (p[2] - a[2]) * d[2]) / l2).clamp(0.0, 1.0) } else { 0.0 };
        dist(p, [a[0] + d[0] * t, a[1] + d[1] * t, a[2] + d[2] * t])
    };
    match line {
        [only] => dist(p, *only),
        _ => line.windows(2).map(|w| seg(w[0], w[1])).fold(f64::INFINITY, f64::min),
    }
}

/// The edges of a body: `polylines` and `edge_faces` from its tessellation (in edge order),
/// `names` its persistent edge names, the welded mesh `positions`/`triangles`/`tri_faces`, and
/// `tol` the chord tolerance.
#[allow(clippy::too_many_arguments)]
pub(crate) fn edges_of(
    body: &solidcraft_kernel::Body,
    polylines: &[Vec<Vec3>],
    edge_faces: &[Vec<u32>],
    names: &[String],
    positions: &[[f64; 3]],
    triangles: &[[u32; 3]],
    tri_faces: &[u32],
    index: &PointIndex,
    tol: f64,
) -> Vec<Edge> {
    // The mesh's sides between two different faces, by that pair of faces.
    let mut sides: BTreeMap<(u32, u32), Vec<(u32, u32)>> = BTreeMap::new();
    {
        let mut half: BTreeMap<(u32, u32), u32> = BTreeMap::new();
        for (t, f) in triangles.iter().zip(tri_faces) {
            for (a, b) in [(t[0], t[1]), (t[1], t[2]), (t[2], t[0])] {
                match half.remove(&(b, a)) {
                    Some(g) if g != *f => {
                        sides.entry((g.min(*f), g.max(*f))).or_default().push((a.min(b), a.max(b)));
                    }
                    Some(_) => {}
                    None => {
                        half.insert((a, b), *f);
                    }
                }
            }
        }
    }
    // The B-rep edges between each pair of faces.
    let mut sharing: BTreeMap<(u32, u32), usize> = BTreeMap::new();
    for fs in edge_faces {
        if let [a, b] = fs[..]
            && a != b
        {
            *sharing.entry((a.min(b), a.max(b))).or_default() += 1;
        }
    }
    polylines
        .iter()
        .enumerate()
        .map(|(i, line)| {
            let points: Vec<[f64; 3]> = line.iter().map(|p| v3(*p)).collect();
            let faces = edge_faces.get(i).cloned().unwrap_or_default();
            let curve = match (solidcraft_kernel::edge_line(body, i), solidcraft_kernel::edge_circle(body, i)) {
                (Some(l), _) => Curve::Line { start: v3(l.start), end: v3(l.end) },
                (None, Some(c)) => Curve::Circle { center: v3(c.center), axis: v3(c.axis.vec()), radius: c.radius, angle: c.angle },
                _ => Curve::Other,
            };
            let path = match faces[..] {
                [a, b] if a != b => {
                    let pair = (a.min(b), a.max(b));
                    let candidates = sides.get(&pair).map(Vec::as_slice).unwrap_or_default();
                    // With other edges between the same faces, only the sides along this one.
                    let own: Vec<(u32, u32)> = if sharing.get(&pair).copied().unwrap_or(0) > 1 {
                        let near = |v: u32| positions.get(v as usize).is_some_and(|p| to_polyline(*p, &points) <= 2.0 * tol + 1e-9);
                        candidates.iter().copied().filter(|(u, v)| near(*u) && near(*v)).collect()
                    } else {
                        candidates.to_vec()
                    };
                    chain(&own, points.first().and_then(|p| index.find(*p)), points.last().and_then(|p| index.find(*p)))
                }
                _ => Vec::new(),
            };
            Edge { index: u32::try_from(i).unwrap_or(u32::MAX), name: names.get(i).cloned().unwrap_or_default(), faces, curve, points, path }
        })
        .collect()
}

/// The sides `sides` as one chain of vertices from `start` (to `end`), or empty when they are
/// not one simple chain.
fn chain(sides: &[(u32, u32)], start: Option<u32>, end: Option<u32>) -> Vec<u32> {
    if sides.is_empty() {
        return Vec::new();
    }
    let mut next: BTreeMap<u32, Vec<u32>> = BTreeMap::new();
    for &(a, b) in sides {
        next.entry(a).or_default().push(b);
        next.entry(b).or_default().push(a);
    }
    if next.values().any(|n| n.len() > 2) {
        return Vec::new();
    }
    let Some(first) = start.filter(|s| next.contains_key(s)) else { return Vec::new() };
    let mut path = vec![first];
    let mut prev = None;
    let mut at = first;
    for _ in 0..sides.len() {
        let Some(n) = next.get(&at).and_then(|n| n.iter().copied().find(|v| Some(*v) != prev)) else { break };
        // A closed edge comes back to where it began.
        if n == first {
            path.push(n);
            break;
        }
        prev = Some(at);
        at = n;
        path.push(n);
    }
    let closed = path.len() > 2 && path.first() == path.last();
    let reached = closed || end.is_none_or(|e| path.last() == Some(&e));
    if path.len() - 1 == sides.len() && reached { path } else { Vec::new() }
}

/// The corners of a body: the ends of its `edges`, those within `close` one corner, named after
/// the faces (`face_names`) of the edges that meet there. A closed edge with no other edge at its
/// ends (a full circle) has no corner.
pub(crate) fn vertices_of(edges: &[Edge], face_names: &[String], index: &PointIndex, close: f64) -> Vec<Vertex> {
    if edges.len().saturating_mul(2) > MAX_EDGE_ENDS {
        return Vec::new();
    }
    let mut found: Vec<([f64; 3], Vec<u32>)> = Vec::new();
    for e in edges {
        for p in [e.points.first(), e.points.last()].into_iter().flatten() {
            match found.iter_mut().find(|(q, _)| dist(*q, *p) <= close) {
                Some((_, at)) => at.push(e.index),
                None => found.push((*p, vec![e.index])),
            }
        }
    }
    found.retain(|(_, at)| at.windows(2).any(|w| w[0] != w[1]) || at.len() > 2);
    let mut vertices: Vec<Vertex> = found
        .into_iter()
        .map(|(point, mut at)| {
            let mut faces: Vec<u32> = at.iter().filter_map(|e| edges.get(*e as usize)).flat_map(|e| e.faces.iter().copied()).collect();
            faces.sort_unstable();
            faces.dedup();
            let mut names: Vec<&str> = faces.iter().filter_map(|f| face_names.get(*f as usize)).map(String::as_str).collect();
            names.sort_unstable();
            at.sort_unstable();
            at.dedup();
            Vertex { name: names.join("|"), point, position: index.find(point), edges: at }
        })
        .collect();
    number_repeats(&mut vertices);
    vertices
}

/// Number the corners that share a name `~0`, `~1`, … by position: along the axis their points
/// spread most along, then the other two, on a grid of a millionth of that spread.
fn number_repeats(vertices: &mut [Vertex]) {
    let mut groups: BTreeMap<String, Vec<usize>> = BTreeMap::new();
    for (i, v) in vertices.iter().enumerate() {
        groups.entry(v.name.clone()).or_default().push(i);
    }
    for (name, mut same) in groups {
        if same.len() < 2 {
            continue;
        }
        let at = |i: usize| vertices.get(i).map_or([0.0; 3], |v| v.point);
        let (mut lo, mut hi) = ([f64::INFINITY; 3], [f64::NEG_INFINITY; 3]);
        for c in same.iter().map(|&i| at(i)) {
            for k in 0..3 {
                lo[k] = lo[k].min(c[k]);
                hi[k] = hi[k].max(c[k]);
            }
        }
        let spread = [hi[0] - lo[0], hi[1] - lo[1], hi[2] - lo[2]];
        let axes = if spread[0] >= spread[1] && spread[0] >= spread[2] {
            [0, 1, 2]
        } else if spread[1] >= spread[2] {
            [1, 2, 0]
        } else {
            [2, 0, 1]
        };
        let grid = spread.iter().fold(1e-9f64, |m, s| m.max(*s)) * 1e-6;
        let key = |i: usize| axes.map(|k| (at(i)[k] / grid).round() as i64);
        same.sort_by_key(|&i| (key(i), i));
        for (k, i) in same.into_iter().enumerate() {
            if let Some(v) = vertices.get_mut(i) {
                v.name = format!("{name}~{k}");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sides_chain_from_the_start() {
        assert_eq!(chain(&[(2, 3), (1, 2)], Some(1), Some(3)), vec![1, 2, 3]);
        // A loop comes back to its start.
        assert_eq!(chain(&[(1, 2), (2, 3), (1, 3)], Some(1), Some(1)), vec![1, 2, 3, 1]);
        // A branch or a gap is no chain.
        assert!(chain(&[(1, 2), (2, 3), (2, 4)], Some(1), Some(3)).is_empty());
        assert!(chain(&[(1, 2), (3, 4)], Some(1), Some(4)).is_empty());
        assert!(chain(&[(1, 2)], None, None).is_empty());
    }
}
