//! Local no-gouge certificates for the vertical height-field target solid.
//!
//! To bound distance to the solid's complement, choose an XY point q with a
//! conservative upper target height H(q). Every point p in a candidate removed
//! column has an exterior point (q, max(p.z,H(q))) at distance no more than
//! hypot(max_xy_distance(p,q), max(H(q)-column_low,0)). This covers whole cells,
//! not sampled surface points, and remains valid for overlapping target bodies.
use crate::{
    spatial::Index,
    verify::{Limits, Status},
    *,
};
use solidcraft_geom::Vec3;

pub(crate) struct Surface<'a> {
    triangles: &'a [[Vec3; 3]],
    index: Index,
    bottom: f64,
    guard: f64,
}
pub(crate) struct Region {
    pub center: [f64; 2],
    pub half: f64,
    pub low: f64,
    pub high: f64,
    pub target_high: f64,
    pub tolerance: f64,
}
impl<'a> Surface<'a> {
    pub(crate) fn new(triangles: &'a [[Vec3; 3]], bottom: f64) -> Result<Self> {
        let extent = triangles.iter().flatten().map(|p| p.x.abs().max(p.y.abs()).max(p.z.abs())).fold(1., f64::max);
        let guard = 128. * f64::EPSILON * extent;
        let items: Vec<_> = triangles
            .iter()
            .enumerate()
            .filter(|(_, t)| (t[1] - t[0]).cross(t[2] - t[0]).z > 0.)
            .map(|(id, t)| {
                let min = t.iter().copied().fold(t[0], Vec3::min);
                let max = t.iter().copied().fold(t[0], Vec3::max);
                (id, [min.x - guard, min.y - guard], [max.x + guard, max.y + guard])
            })
            .collect();
        if items.is_empty() {
            return invalid("protected target has no upward height-field surface");
        }
        Ok(Self { triangles, index: Index::new(items), bottom, guard })
    }
    pub(crate) fn bounds(&self, p: [f64; 2], half: f64, work: &mut usize, budget: usize) -> (f64, f64, bool) {
        let mut ids = Vec::new();
        self.index.query([p[0] - half, p[1] - half], [p[0] + half, p[1] + half], &mut ids);
        let local: Vec<_> = ids.into_iter().map(|i| self.triangles[i]).collect();
        crate::verify::target_height(p, half, &local, self.bottom, work, budget)
    }
    async fn height<F: std::future::Future<Output = ()>>(
        &self,
        p: [f64; 2],
        work: &mut usize,
        limits: Limits,
        cancelled: &impl Fn() -> bool,
        yield_control: &mut impl FnMut() -> F,
    ) -> Result<Option<f64>> {
        let mut ids = Vec::new();
        self.index.query([p[0] - self.guard, p[1] - self.guard], [p[0] + self.guard, p[1] + self.guard], &mut ids);
        let mut height = self.bottom;
        for (count, id) in ids.into_iter().enumerate() {
            if count % 4096 == 4095 {
                yield_control().await;
                if cancelled() {
                    return invalid("surface verification cancelled");
                }
            }
            *work += 1;
            if *work > limits.work {
                return Ok(None);
            }
            let t = &self.triangles[id];
            if let Some((_, upper)) = height_interval(t, p, self.guard) {
                height = height.max(upper);
            }
        }
        Ok(Some(height + self.guard))
    }
    pub(crate) async fn protect<F: std::future::Future<Output = ()>>(
        &self,
        r: Region,
        work: &mut usize,
        limits: Limits,
        cancelled: &impl Fn() -> bool,
        yield_control: &mut impl FnMut() -> F,
    ) -> Result<Status> {
        let tolerance = r.tolerance - POST_ERROR - VERIFY_ERROR - self.guard;
        if tolerance > 0. && r.target_high + self.guard - r.low <= tolerance {
            return Ok(Status::Pass);
        }
        if r.half < tolerance {
            // The search locates a certificate. Failure to find one never proves
            // a gouge and never becomes a clearance pass.
            let reach = tolerance - r.half;
            for scale in [0., 0.5, 0.95] {
                for direction in 0..8 {
                    if scale == 0. && direction != 0 {
                        continue;
                    }
                    let angle = direction as f64 * std::f64::consts::FRAC_PI_4;
                    let offset = scale * reach;
                    let q = [r.center[0] + offset * angle.cos(), r.center[1] + offset * angle.sin()];
                    let Some(height) = self.height(q, work, limits, cancelled, yield_control).await? else {
                        return Ok(Status::Inconclusive);
                    };
                    // Include coordinate rounding in the actual computed offset.
                    let lateral = r.half + (q[0] - r.center[0]).hypot(q[1] - r.center[1]);
                    if lateral.hypot((height - r.low).max(0.)) + self.guard <= tolerance {
                        return Ok(Status::Pass);
                    }
                }
            }
        }
        // A positive failure certificate: a ball larger than the tolerance is
        // contained in one target triangle's downward extrusion, while its center
        // is definitely removed. No coverage sums or assumed mesh watertightness.
        let radius = r.tolerance + POST_ERROR + VERIFY_ERROR + self.guard;
        let z = (r.high + self.guard).max(self.bottom + 2. * radius);
        if z - radius > self.bottom {
            let mut ids = Vec::new();
            self.index.query(r.center, r.center, &mut ids);
            for (count, id) in ids.into_iter().enumerate() {
                if count % 4096 == 4095 {
                    yield_control().await;
                    if cancelled() {
                        return invalid("surface verification cancelled");
                    }
                }
                *work += 1;
                if *work > limits.work {
                    return Ok(Status::Inconclusive);
                }
                let t = &self.triangles[id];
                let covered = [[-radius, -radius], [radius, -radius], [radius, radius], [-radius, radius]]
                    .iter()
                    .all(|d| height_interval(t, [r.center[0] + d[0], r.center[1] + d[1]], self.guard).is_some_and(|(lower, _)| lower > z + radius));
                if covered {
                    return Ok(Status::Fail);
                }
            }
        }
        Ok(Status::Inconclusive)
    }
}

// Upper is conservative even on a projected triangle edge. Lower is -infinity
// near edges or degenerate projections so it cannot certify an interior ball.
fn height_interval(t: &[Vec3; 3], p: [f64; 2], guard: f64) -> Option<(f64, f64)> {
    let n = (t[1] - t[0]).cross(t[2] - t[0]);
    if n.z <= 0. {
        return None;
    }
    let mut near = false;
    for (a, b) in [(t[0], t[1]), (t[1], t[2]), (t[2], t[0])] {
        let edge = (b.x - a.x) * (p[1] - a.y) - (b.y - a.y) * (p[0] - a.x);
        let error = guard * (1. + (b.x - a.x).abs() + (b.y - a.y).abs());
        if edge < -error {
            return None;
        }
        near |= edge <= error;
    }
    let hi = t.iter().map(|p| p.z).fold(f64::NEG_INFINITY, f64::max);
    if near || n.z <= guard * guard {
        return Some((f64::NEG_INFINITY, hi + guard));
    }
    let z = t[0].z + (n.x * (t[0].x - p[0]) + n.y * (t[0].y - p[1])) / n.z;
    let error = guard * (1. + (n.x / n.z).abs() + (n.y / n.z).abs());
    if !z.is_finite() || !error.is_finite() {
        return Some((f64::NEG_INFINITY, hi + guard));
    }
    Some((z - error, (z + error).min(hi + guard)))
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    fn plate(lo: f64, hi: f64, z: f64) -> Vec<[Vec3; 3]> {
        let a = Vec3::new(lo, lo, z);
        let b = Vec3::new(hi, lo, z);
        let c = Vec3::new(hi, hi, z);
        let d = Vec3::new(lo, hi, z);
        vec![[a, b, c], [a, c, d]]
    }
    fn protect(s: &Surface, r: Region, budget: usize) -> Status {
        use std::future::Future;
        let mut ready = || std::future::ready(());
        let mut work = 0;
        let mut future = std::pin::pin!(s.protect(r, &mut work, Limits { work: budget, ..Default::default() }, &|| false, &mut ready));
        match future.as_mut().poll(&mut std::task::Context::from_waker(std::task::Waker::noop())) {
            std::task::Poll::Ready(r) => r.unwrap(),
            _ => Status::Inconclusive,
        }
    }
    fn region(x: f64, half: f64, low: f64, high: f64) -> Region {
        Region { center: [x, 1.], half, low, high, target_high: 5., tolerance: 0.01 }
    }
    #[test]
    fn vertical_boundary_is_certified_but_interior_gouge_fails() {
        let tris = plate(0., 2., 5.);
        let s = Surface::new(&tris, 0.).unwrap();
        assert_eq!(protect(&s, region(0.001, 0.001, 1., 5.), 1000), Status::Pass);
        assert_eq!(protect(&s, region(0.1, 0.001, 4.9, 4.9), 1000), Status::Fail);
        assert_eq!(protect(&s, region(0.001, 0.1, 1., 5.), 1000), Status::Inconclusive);
    }
    #[test]
    fn overlapping_top_faces_do_not_create_false_exterior_points() {
        let mut tris = plate(0., 2., 5.);
        tris.extend(plate(-1., 3., 6.));
        let s = Surface::new(&tris, 0.).unwrap();
        let mut r = region(0.001, 0.001, 4.9, 4.9);
        r.target_high = 6.;
        assert_eq!(protect(&s, r, 1000), Status::Fail);
    }
    #[test]
    fn budget_exhaustion_is_inconclusive() {
        let tris = plate(0., 2., 5.);
        let s = Surface::new(&tris, 0.).unwrap();
        assert_eq!(protect(&s, region(0.001, 0.001, 1., 5.), 1), Status::Inconclusive);
    }
    #[test]
    fn missing_upward_faces_are_rejected() {
        assert!(Surface::new(&[[Vec3::ZERO, Vec3::X, Vec3::X]], 0.).is_err());
    }
    #[test]
    fn shifted_coordinates_preserve_height_bounds() {
        let tris = plate(999999., 1000001., 5.);
        let s = Surface::new(&tris, 0.).unwrap();
        let mut work = 0;
        let (lo, hi, uncertain) = s.bounds([1000000., 1000000.], 0.001, &mut work, 1000);
        assert!(!uncertain);
        assert_eq!(lo, 5.);
        assert_eq!(hi, 5.);
    }
    fn drilled_plate() -> JobInput {
        corpus::fixture("drill").unwrap()
    }

    #[test]
    fn protected_drilled_plate_certifies_with_its_wall_and_cone() {
        let input = drilled_plate();
        let path = generate::generate(input.clone()).unwrap();
        let parsed = nc::round_trip(&path, &input.setup, &input.tools).unwrap();
        let report = verify::simulate(&input, &parsed, Default::default()).unwrap();
        assert_eq!(report.status, Status::Pass, "{:?}", report.findings);
        assert_eq!(report.finished_dimensions, Status::Pass);
    }
    #[test]
    fn wrong_hole_against_protected_cad_surface_is_a_gouge() {
        let input = drilled_plate();
        let path = generate::generate(input.clone()).unwrap();
        let code = nc::post(&path, &input.setup).unwrap().replace("X10.000000", "X10.100000");
        let parsed = nc::parse(&code, &input.tools).unwrap();
        let report = verify::simulate(&input, &parsed, Default::default()).unwrap();
        assert_eq!(report.status, Status::Fail, "{:?}", report.findings);
        assert!(report.findings.iter().any(|f| f.kind == "gouge"));
    }
    #[test]
    fn through_gouges_and_sloping_surfaces_use_spatial_distance() {
        let mut tris = plate(0., 2., 5.);
        let s = Surface::new(&tris, 0.).unwrap();
        assert_eq!(protect(&s, region(0.1, 0.001, 0., 0.), 1000), Status::Fail);
        for t in &mut tris {
            for p in t {
                p.z = 5. + 10. * p.x;
            }
        }
        let s = Surface::new(&tris, 0.).unwrap();
        let mut r = region(0.1, 0.001, 5.97, 5.97);
        r.target_high = 6.02;
        assert_eq!(protect(&s, r, 1000), Status::Pass);
        let mut r = region(0.1, 0.001, 5.8, 5.8);
        r.target_high = 6.02;
        assert_eq!(protect(&s, r, 1000), Status::Fail);
    }
    #[test]
    fn surface_queries_yield_and_cancel() {
        use std::{
            future::Future,
            task::{Context, Poll, Waker},
        };
        let tris: Vec<_> = (0..5000).flat_map(|_| plate(0., 2., 5.)).collect();
        let s = Surface::new(&tris, 0.).unwrap();
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
        let mut future = std::pin::pin!(s.protect(region(0.1, 0.001, 4.9, 4.9), &mut work, Default::default(), &cancelled, &mut yielding));
        let mut context = Context::from_waker(Waker::noop());
        assert!(future.as_mut().poll(&mut context).is_pending());
        cancel.set(true);
        assert!(matches!(future.as_mut().poll(&mut context), Poll::Ready(Err(_))));
    }
}
