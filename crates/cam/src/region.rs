//! Independent planar-region predicates for finished dimensions. No offsets or
//! toolpath-generation routines are used. Winding ignores input ring direction.
use crate::{spatial::Index, *};

fn orient(a: [f64; 2], b: [f64; 2], p: [f64; 2]) -> f64 {
    robust::orient2d(robust::Coord { x: a[0], y: a[1] }, robust::Coord { x: b[0], y: b[1] }, robust::Coord { x: p[0], y: p[1] })
}
fn contains(ring: &[[f64; 2]], p: [f64; 2]) -> bool {
    ring.iter().zip(ring.iter().cycle().skip(1)).take(ring.len()).fold(false, |inside, (a, b)| inside ^ crossing(*a, *b, p))
}
fn crossing(a: [f64; 2], b: [f64; 2], p: [f64; 2]) -> bool {
    (a[1] > p[1]) != (b[1] > p[1]) && if b[1] > a[1] { orient(a, b, p) > 0. } else { orient(a, b, p) < 0. }
}
struct Edge {
    a: [f64; 2],
    b: [f64; 2],
    ring: usize,
    vertex: usize,
    count: usize,
}
pub(crate) struct Region {
    edges: Vec<Edge>,
    index: Index,
    max_x: f64,
    guard: f64,
}
impl Region {
    pub(crate) fn new(boundary: &Boundary) -> Result<Self> {
        boundary.validate()?;
        let rings: Vec<_> = std::iter::once(&boundary.outer).chain(&boundary.islands).collect();
        let mut edges = Vec::new();
        let mut extent = 1_f64;
        let mut max_x = f64::NEG_INFINITY;
        for (r, ring) in rings.iter().enumerate() {
            let mut area = 0.;
            let origin = ring[0];
            for (vertex, (&a, &b)) in ring.iter().zip(ring.iter().cycle().skip(1)).take(ring.len()).enumerate() {
                if a.into_iter().any(|v| v != 0. && v.abs() < 1e-100) || (a[0] - b[0]).hypot(a[1] - b[1]) < 1e-7 {
                    return invalid("unresolvable dimensional boundary edge");
                }
                extent = extent.max(a[0].abs()).max(a[1].abs());
                max_x = max_x.max(a[0]);
                area += (a[0] - origin[0]) * (b[1] - origin[1]) - (a[1] - origin[1]) * (b[0] - origin[0]);
                edges.push(Edge { a, b, ring: r, vertex, count: ring.len() });
            }
            if area.abs() < 1e-8 {
                return invalid("zero-area dimensional boundary");
            }
        }
        let index = Index::new(
            edges
                .iter()
                .enumerate()
                .map(|(id, e)| (id, [e.a[0].min(e.b[0]), e.a[1].min(e.b[1])], [e.a[0].max(e.b[0]), e.a[1].max(e.b[1])]))
                .collect(),
        );
        let mut work = 0;
        for (id, e) in edges.iter().enumerate() {
            let mut candidates = Vec::new();
            index.query([e.a[0].min(e.b[0]), e.a[1].min(e.b[1])], [e.a[0].max(e.b[0]), e.a[1].max(e.b[1])], &mut candidates);
            for other in candidates.into_iter().filter(|other| *other > id) {
                work += 1;
                if work > 2_000_000 {
                    return invalid("dimensional boundary validation budget exceeded");
                }
                let f = &edges[other];
                if e.ring == f.ring && ((e.vertex + 1) % e.count == f.vertex || (f.vertex + 1) % e.count == e.vertex) {
                    let (a, b, c) = if e.b == f.a { (e.a, e.b, f.b) } else { (f.a, f.b, e.b) };
                    if orient(a, b, c) == 0. && (b[0] - a[0]) * (c[0] - b[0]) + (b[1] - a[1]) * (c[1] - b[1]) <= 0. {
                        return invalid("backtracking dimensional boundary");
                    }
                    continue;
                }
                if (0..2).any(|k| e.a[k].max(e.b[k]) < f.a[k].min(f.b[k]) || f.a[k].max(f.b[k]) < e.a[k].min(e.b[k])) {
                    continue;
                }
                let opposite = |x: f64, y: f64| x == 0. || y == 0. || x.is_sign_positive() != y.is_sign_positive();
                if opposite(orient(e.a, e.b, f.a), orient(e.a, e.b, f.b)) && opposite(orient(f.a, f.b, e.a), orient(f.a, f.b, e.b)) {
                    return invalid("intersecting or touching dimensional boundaries");
                }
            }
        }
        for (id, island) in boundary.islands.iter().enumerate() {
            if !contains(&boundary.outer, island[0]) || boundary.islands.iter().enumerate().any(|(j, r)| id != j && contains(r, island[0])) {
                return invalid("dimensional islands must be disjoint and inside the outer boundary");
            }
        }
        Ok(Self { edges, index, max_x, guard: 128. * f64::EPSILON * extent })
    }
    /// Signed distance is positive in the material-removal region. Distance to
    /// its boundary is 1-Lipschitz, so center distance +/- cell radius bounds the
    /// whole cell. A capped nearest-edge query returns an infinite far endpoint.
    pub(crate) async fn range<F: std::future::Future<Output = ()>>(
        &self,
        query: ([f64; 2], f64, f64),
        work: &mut usize,
        budget: usize,
        cancelled: &impl Fn() -> bool,
        yield_control: &mut impl FnMut() -> F,
    ) -> Result<Option<[f64; 2]>> {
        let (p, half, reach) = query;
        let guard = self.guard.max(128. * f64::EPSILON * p[0].abs().max(p[1].abs()));
        let mut candidates = Vec::new();
        self.index.query(p, [self.max_x.max(p[0]), p[1]], &mut candidates);
        let mut inside = false;
        for (n, id) in candidates.into_iter().enumerate() {
            if n % 4096 == 4095 {
                yield_control().await;
                if cancelled() {
                    return invalid("dimensional verification cancelled");
                }
            }
            *work += 1;
            if *work > budget {
                return Ok(None);
            }
            let e = &self.edges[id];
            inside ^= crossing(e.a, e.b, p);
        }
        let radius = reach + half + guard;
        let mut candidates = Vec::new();
        self.index.query(p.map(|v| v - radius), p.map(|v| v + radius), &mut candidates);
        let mut distance = radius;
        for (n, id) in candidates.into_iter().enumerate() {
            if n % 4096 == 4095 {
                yield_control().await;
                if cancelled() {
                    return invalid("dimensional verification cancelled");
                }
            }
            *work += 1;
            if *work > budget {
                return Ok(None);
            }
            let e = &self.edges[id];
            let dx = e.b[0] - e.a[0];
            let dy = e.b[1] - e.a[1];
            let t = (((p[0] - e.a[0]) * dx + (p[1] - e.a[1]) * dy) / (dx * dx + dy * dy)).clamp(0., 1.);
            distance = distance.min((p[0] - e.a[0] - t * dx).hypot(p[1] - e.a[1] - t * dy));
        }
        let lo = (distance - half - guard).max(0.);
        let hi = if distance == radius { f64::INFINITY } else { distance + half + guard };
        if distance <= half + guard {
            Ok(Some([-hi, hi]))
        } else if inside {
            Ok(Some([lo, hi]))
        } else {
            Ok(Some([-hi, -lo]))
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use std::{
        future::Future,
        task::{Context, Poll, Waker},
    };
    fn square(a: f64, b: f64) -> Vec<[f64; 2]> {
        vec![[a, a], [b, a], [b, b], [a, b]]
    }
    fn range(r: &Region, p: [f64; 2], half: f64, budget: usize) -> Option<[f64; 2]> {
        let mut ready = || std::future::ready(());
        let mut work = 0;
        let mut f = std::pin::pin!(r.range((p, half, 10.), &mut work, budget, &|| false, &mut ready));
        match f.as_mut().poll(&mut Context::from_waker(Waker::noop())) {
            Poll::Ready(result) => result.unwrap(),
            _ => None,
        }
    }
    #[test]
    fn signed_distances_cover_concavity_islands_and_winding() {
        let mut boundary = Boundary { outer: square(0., 10.), islands: vec![square(4., 6.)] };
        for _ in 0..2 {
            let r = Region::new(&boundary).unwrap();
            for (p, expected) in [([3., 5.], 1.), ([5., 5.], -1.), ([-1., 5.], -1.)] {
                let [a, b] = range(&r, p, 0.1, 1000).unwrap();
                assert!((a - (expected - 0.1)).abs() < 1e-10 && (b - (expected + 0.1)).abs() < 1e-10, "{a} {b}");
            }
            boundary.outer.reverse();
            boundary.islands[0].reverse();
        }
        let r = Region::new(&Boundary { outer: vec![[0., 0.], [4., 0.], [4., 1.], [1., 1.], [1., 4.], [0., 4.]], islands: vec![] }).unwrap();
        assert!((range(&r, [2., 2.], 0., 1000).unwrap()[0] + 1.).abs() < 1e-10);
        assert!((range(&r, [0.5, 3.], 0., 1000).unwrap()[0] - 0.5).abs() < 1e-10);
        assert!(range(&r, [0.5, 3.], 0., 0).is_none());
    }
    #[test]
    fn invalid_regions_are_rejected_without_generator_validation() {
        for b in [
            Boundary { outer: vec![[0., 0.], [2., 2.], [0., 2.], [2., 0.]], islands: vec![] },
            Boundary { outer: square(0., 10.), islands: vec![square(0., 2.)] },
            Boundary { outer: square(0., 10.), islands: vec![square(2., 8.), square(3., 4.)] },
            Boundary { outer: vec![[0., 0.], [2., 0.], [1., 0.], [2., 2.], [0., 2.]], islands: vec![] },
        ] {
            assert!(Region::new(&b).is_err());
        }
    }
    #[test]
    fn large_boundary_queries_yield_and_cancel() {
        let b = Boundary {
            outer: (0..10_000)
                .map(|i| {
                    let a = (i as f64 + 0.1) * std::f64::consts::TAU / 10_000.;
                    [a.cos() * 10., a.sin() * 10.]
                })
                .collect(),
            islands: vec![],
        };
        let r = Region::new(&b).unwrap();
        let mut work = 0;
        let cancel = std::cell::Cell::new(false);
        let cancelled = || cancel.get();
        let mut yielding = || {
            let mut first = true;
            std::future::poll_fn(move |_| {
                if first {
                    first = false;
                    Poll::Pending
                } else {
                    Poll::Ready(())
                }
            })
        };
        let mut f = std::pin::pin!(r.range(([0., 0.], 20., 0.), &mut work, 100_000, &cancelled, &mut yielding));
        let mut cx = Context::from_waker(Waker::noop());
        assert!(f.as_mut().poll(&mut cx).is_pending());
        cancel.set(true);
        assert!(matches!(f.as_mut().poll(&mut cx), Poll::Ready(Err(_))));
    }
}
