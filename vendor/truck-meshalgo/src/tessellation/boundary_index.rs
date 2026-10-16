//! SolidCraft: conservative line candidates for the existing trimming winding predicate.
//! This index never classifies a point, changes a segment or chooses a ray direction.
use std::ops::Range;

type Point = [f64; 2];
type Segment = (Point, Point);

#[derive(Clone, Debug)]
struct Node {
    low: Point,
    high: Point,
    range: Range<usize>,
    children: Option<(usize, usize)>,
}

#[derive(Clone, Debug, Default)]
pub(super) struct BoundaryIndex {
    segments: Vec<Segment>,
    nodes: Vec<Node>,
    root: Option<usize>,
}

impl BoundaryIndex {
    pub(super) fn new(segments: impl IntoIterator<Item = Segment>) -> Self {
        let mut index = Self { segments: segments.into_iter().collect(), ..Self::default() };
        // Non-finite or extreme domains retain the full scan. The magnitude cap
        // also keeps midpoint sorting and the conservative rounding reserve finite.
        if index.segments.iter().all(|(a, b)| a.iter().chain(b).all(|v| v.is_finite() && v.abs() <= 1e100)) {
            index.root = index.build(0..index.segments.len());
        }
        index
    }

    fn build(&mut self, range: Range<usize>) -> Option<usize> {
        let segments = self.segments.get_mut(range.clone())?;
        if segments.is_empty() {
            return None;
        }
        let mut low = [f64::INFINITY; 2];
        let mut high = [f64::NEG_INFINITY; 2];
        for (a, b) in segments.iter() {
            for axis in 0..2 {
                low[axis] = low[axis].min(a[axis]).min(b[axis]);
                high[axis] = high[axis].max(a[axis]).max(b[axis]);
            }
        }
        let middle = range.start + segments.len() / 2;
        if segments.len() > 8 {
            let axis = usize::from(high[1] - low[1] > high[0] - low[0]);
            let position = segments.len() / 2;
            segments.select_nth_unstable_by(position, |(a, b), (c, d)| (a[axis] + b[axis]).total_cmp(&(c[axis] + d[axis])));
        }
        let id = self.nodes.len();
        self.nodes.push(Node { low, high, range: range.clone(), children: None });
        if range.len() > 8 {
            let left = self.build(range.start..middle)?;
            let right = self.build(middle..range.end)?;
            self.nodes.get_mut(id)?.children = Some((left, right));
        }
        Some(id)
    }

    /// Visit a conservative superset of segments crossing the infinite query
    /// line. Keeping the backwards half avoids changing near-origin/parallel
    /// behavior of the caller's floating-point winding predicate. False stops.
    pub(super) fn visit(&self, point: Point, ray: Point, mut visitor: impl FnMut(Point, Point) -> bool) -> bool {
        match self.root {
            Some(root) => self.visit_node(root, point, ray, &mut visitor),
            None => self.segments.iter().all(|(a, b)| visitor(*a, *b)),
        }
    }

    fn visit_node(&self, id: usize, point: Point, ray: Point, visitor: &mut impl FnMut(Point, Point) -> bool) -> bool {
        let Some(node) = self.nodes.get(id) else {
            return false;
        };
        let side = |p: Point| ray[0] * (p[1] - point[1]) - ray[1] * (p[0] - point[0]);
        let values = [side(node.low), side(node.high), side([node.low[0], node.high[1]]), side([node.high[0], node.low[1]])];
        let scale = 1. + point[0].abs() + point[1].abs() + node.low[0].abs() + node.low[1].abs() + node.high[0].abs() + node.high[1].abs();
        let guard = 64. * f64::EPSILON * scale * (1. + ray[0].abs() + ray[1].abs());
        // The same subtraction/multiplication order as the original predicate
        // bounds every segment endpoint inside the box. Non-finite arithmetic
        // cannot prune a node; the old predicate remains the authority.
        if guard.is_finite() && values.iter().all(|v| v.is_finite()) && (values.iter().all(|v| *v > guard) || values.iter().all(|v| *v < -guard)) {
            return true;
        }
        match node.children {
            Some((left, right)) => self.visit_node(left, point, ray, visitor) && self.visit_node(right, point, ray, visitor),
            None => self.segments.get(node.range.clone()).is_some_and(|segments| segments.iter().all(|(a, b)| visitor(*a, *b))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn loop_segments(points: &[Point]) -> Vec<Segment> {
        points.iter().copied().zip(points.iter().copied().cycle().skip(1)).take(points.len()).collect()
    }
    fn winding(segments: impl IntoIterator<Item = Segment>, point: Point, ray: Point) -> Option<i32> {
        segments.into_iter().try_fold(0, |counter, (a, b)| winding_edge(counter, a, b, point, ray))
    }
    fn winding_edge(counter: i32, a: Point, b: Point, c: Point, r: Point) -> Option<i32> {
        let a = [a[0] - c[0], a[1] - c[1]];
        let b = [b[0] - c[0], b[1] - c[1]];
        let s0 = r[0] * a[1] - r[1] * a[0];
        let s1 = r[0] * b[1] - r[1] * b[0];
        let s2 = a[0] * b[1] - a[1] * b[0];
        let x = s2 / (s1 - s0);
        if x.abs() < 1e-6 && s0 * s1 < 0. {
            None
        } else if x > 0. && s0 <= 0. && s1 > 0. {
            Some(counter + 1)
        } else if x > 0. && s0 >= 0. && s1 < 0. {
            Some(counter - 1)
        } else {
            Some(counter)
        }
    }

    #[test]
    fn indexed_trimming_matches_full_scan_for_concavity_holes_and_boundary_points() {
        for (center, scale) in [([0., 0.], 1.), ([1e8, -1e8], 1000.), ([0.001, -0.002], 1e-6)] {
            let outer: Vec<_> = (0..1001)
                .map(|i| {
                    let a = i as f64 * std::f64::consts::TAU / 1001.;
                    let r = 8. - (7. * a).cos();
                    [center[0] + scale * r * a.cos(), center[1] + scale * r * a.sin()]
                })
                .collect();
            let hole: Vec<_> = (0..64)
                .rev()
                .map(|i| {
                    let a = i as f64 * std::f64::consts::TAU / 64.;
                    [center[0] + scale * a.cos(), center[1] + scale * a.sin()]
                })
                .collect();
            let mut segments = loop_segments(&outer);
            segments.extend(loop_segments(&hole));
            for reverse in [false, true] {
                if reverse {
                    for (a, b) in &mut segments {
                        std::mem::swap(a, b);
                    }
                }
                let index = BoundaryIndex::new(segments.clone());
                let mut points = outer.iter().step_by(11).copied().collect::<Vec<_>>();
                points.extend(segments.iter().step_by(13).map(|(a, b)| [(a[0] + b[0]) * 0.5, (a[1] + b[1]) * 0.5]));
                points
                    .extend((0..400).map(|i| [center[0] + scale * (i % 20) as f64 - scale * 10., center[1] + scale * (i / 20) as f64 - scale * 10.]));
                for (i, point) in points.into_iter().enumerate() {
                    for ray in [[1., 0.], [1., 1e-15], [0., 1.], [(i as f64).cos(), (i as f64).sin()]] {
                        let expected = winding(segments.iter().copied(), point, ray);
                        let mut actual = Some(0);
                        index.visit(point, ray, |a, b| {
                            actual = actual.and_then(|counter| winding_edge(counter, a, b, point, ray));
                            actual.is_some()
                        });
                        assert_eq!(actual, expected, "point={point:?} ray={ray:?}");
                    }
                }
            }
        }
    }

    #[test]
    fn index_prunes_dense_boundaries_and_preserves_nonfinite_fallback_and_early_stop() {
        let points: Vec<_> = (0..10_000)
            .map(|i| {
                let a = i as f64 * std::f64::consts::TAU / 10_000.;
                [a.cos(), a.sin()]
            })
            .collect();
        let index = BoundaryIndex::new(loop_segments(&points));
        let mut count = 0;
        assert!(index.visit([0., 0.], [0.4, 0.8], |_, _| {
            count += 1;
            true
        }));
        assert!(count > 0 && count < 200, "visited {count} of 10000 segments");
        count = 0;
        assert!(!index.visit([0., 0.], [0.4, 0.8], |_, _| {
            count += 1;
            false
        }));
        assert_eq!(count, 1);
        count = 0;
        index.visit([f64::NAN, 0.], [1., 0.], |_, _| {
            count += 1;
            true
        });
        assert_eq!(count, 10_000);
        for bad in [f64::NAN, f64::INFINITY, 1e200] {
            let segments = [([bad, 0.], [1., 0.]), ([0., 1.], [0., 2.])];
            let index = BoundaryIndex::new(segments);
            count = 0;
            index.visit([0., 0.], [1., 0.], |_, _| {
                count += 1;
                true
            });
            assert_eq!(count, 2);
        }
        assert!(BoundaryIndex::default().visit([0., 0.], [1., 0.], |_, _| false));
    }
}
