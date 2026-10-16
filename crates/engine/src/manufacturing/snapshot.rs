//! Cheap immutable capture on the session thread; curve sampling, tessellation,
//! transforms and hashing happen during preparation on the job's worker.
use crate::{
    EngineError, Result, Session, cam,
    geom::{Plane, Region2, Seg2, Vec2, Vec3},
};
use solidcraft_doc::{Mat, apply_point};

pub(super) struct Snapshot {
    setup: cam::Setup,
    tools: Vec<cam::Tool>,
    operations: Vec<cam::Operation>,
    profiles: Vec<(usize, Profile)>,
    holes: Vec<(usize, Vec<super::holes::Hole>)>,
    models: Vec<PlacedBody>,
    fixtures: Vec<PlacedBody>,
}
pub(super) struct PlacedBody {
    pub(super) body: solidcraft_doc::ModelBody,
    pub(super) placement: Mat,
}
pub(super) struct Profile {
    pub(super) region: Region2,
    pub(super) plane: Plane,
    tolerance: f64,
}
fn error(message: &str) -> EngineError {
    EngineError::Other(message.into())
}
fn cancelled(check: &impl Fn() -> bool) -> Result<()> {
    if check() { Err(error("CAM preparation cancelled")) } else { Ok(()) }
}

impl Snapshot {
    pub(super) fn capture(s: &Session, program: u64) -> Result<Self> {
        s.doc.manufacturing.validate().map_err(super::error)?;
        let m = &s.doc.manufacturing;
        let p = m.programs.iter().find(|p| p.id == program).ok_or_else(|| error("CAM program not found"))?;
        let setup = m.setups.iter().find(|x| x.id == p.setup).cloned().ok_or_else(|| error("CAM setup missing"))?;
        let operations = p
            .operations
            .iter()
            .map(|id| m.operations.iter().find(|o| o.id == *id).cloned().ok_or_else(|| error("CAM operation missing")))
            .collect::<Result<Vec<_>>>()?;
        let mut profiles = Vec::new();
        for (i, o) in operations.iter().enumerate() {
            if let Some(source) = &o.source {
                profiles.push((i, Profile::capture(s, &setup, source, o.tolerance)?));
            }
        }
        let mut holes = Vec::new();
        for (index, operation) in operations.iter().enumerate() {
            if !operation.hole_sources.is_empty() {
                holes.push((
                    index,
                    operation
                        .hole_sources
                        .iter()
                        .map(|source| super::holes::Hole::capture(s, &setup, source, operation.tolerance))
                        .collect::<Result<_>>()?,
                ));
            }
        }
        let tools = m.tools.iter().filter(|t| operations.iter().any(|o| o.tool == t.id)).cloned().collect::<Vec<_>>();
        if tools.iter().map(|t| t.number).collect::<std::collections::BTreeSet<_>>().len() != tools.len() {
            return Err(error("CAM tools must have unique tool numbers within a program"));
        }
        let models = setup.models.iter().map(|r| PlacedBody::capture(s, r)).collect::<Result<_>>()?;
        let fixtures = setup.fixtures.iter().map(|r| PlacedBody::capture(s, r)).collect::<Result<_>>()?;
        Ok(Self { setup, tools, operations, profiles, holes, models, fixtures })
    }
    pub(super) async fn build<F: std::future::Future<Output = ()>>(
        mut self,
        check: &impl Fn() -> bool,
        progress: &impl Fn(usize),
        yield_control: &mut impl FnMut() -> F,
    ) -> Result<cam::JobInput> {
        let total =
            self.profiles.len() + self.holes.iter().map(|(_, holes)| holes.len()).sum::<usize>() + self.models.len() + self.fixtures.len() + 1;
        let mut done = 0;
        for (index, p) in self.profiles {
            cancelled(check)?;
            yield_control().await;
            let boundary = p.sample(check, yield_control).await?;
            let op = self.operations.get_mut(index).ok_or_else(|| error("CAM operation missing from snapshot"))?;
            match &mut op.geometry {
                cam::Strategy::Contour { boundary: b, .. } | cam::Strategy::Pocket { boundary: b } => *b = boundary,
                _ => return Err(error("profile bound to unsupported operation")),
            }
            done += 1;
            progress(done * 20 / total);
        }
        for (index, holes) in self.holes {
            let operation = self.operations.get_mut(index).ok_or_else(|| error("CAM hole operation missing"))?;
            let tool = self.tools.iter().find(|t| t.id == operation.tool).ok_or_else(|| error("CAM drill missing"))?;
            let mut centers = Vec::new();
            for hole in holes {
                cancelled(check)?;
                yield_control().await;
                let selected = hole.resolve(self.setup.frame, operation.tolerance)?;
                if (selected.diameter - tool.diameter).abs() > operation.tolerance {
                    return Err(error("selected hole diameter differs from the drill; choose a matching tool or unlink the hole"));
                }
                if centers.iter().any(|p: &[f64; 2]| (p[0] - selected.center[0]).hypot(p[1] - selected.center[1]) <= operation.tolerance) {
                    return Err(error("selected hole axes coincide; remove duplicate selections"));
                }
                centers.push(selected.center);
                done += 1;
                progress(done * 20 / total);
            }
            let cam::Strategy::Drill { holes, .. } = &mut operation.geometry else {
                return Err(error("hole references require drilling"));
            };
            *holes = centers;
        }
        let mut target = Vec::new();
        let mut fixtures = Vec::new();
        let mut fixture_groups = Vec::new();
        for body in self.models {
            body.append(self.setup.frame, &mut target, check, yield_control).await?;
            done += 1;
            progress(done * 20 / total);
        }
        for body in self.fixtures {
            let before = fixtures.len();
            body.append(self.setup.frame, &mut fixtures, check, yield_control).await?;
            fixture_groups.push(fixtures.len() - before);
            done += 1;
            progress(done * 20 / total);
        }
        if target.len() + fixtures.len() > 200_000 {
            return Err(error("CAM mesh exceeds verification budget"));
        }
        cancelled(check)?;
        yield_control().await;
        let mut input = cam::JobInput {
            setup: self.setup,
            tools: self.tools,
            operations: self.operations,
            geometry: cam::Geometry { target, fixtures, fixture_groups },
            fingerprint: String::new(),
        };
        input.validate().map_err(super::error)?;
        input.fingerprint = cam::fingerprint_job_cooperative(&input, check, yield_control).await.map_err(super::error)?;
        cancelled(check)?;
        progress(20);
        Ok(input)
    }
}
impl PlacedBody {
    pub(super) fn capture(s: &Session, r: &cam::BodyRef) -> Result<Self> {
        let state = s.model.state();
        let candidates: Vec<_> = state.bodies.iter().filter(|b| b.name == r.body).collect();
        if candidates.len() != 1 {
            return Err(error(&format!("CAM body `{}` missing or ambiguous; reselect it", r.body)));
        }
        let b = candidates[0];
        let component = s.doc.model_body_component(b);
        let placement = if r.occurrence.is_empty() {
            if component != 0 {
                return Err(error("CAM component body requires an occurrence path"));
            }
            solidcraft_doc::IDENTITY
        } else {
            let p = s.occurrence_placement(&r.occurrence)?;
            if p.component != component {
                return Err(error("CAM body no longer belongs to selected occurrence"));
            }
            p.world
        };
        let placement = s.doc.model_body_offset(b).map(|offset| solidcraft_doc::mat_mul(&placement, offset)).unwrap_or(placement);
        Ok(Self { body: b.clone(), placement })
    }
    async fn append<F: std::future::Future<Output = ()>>(
        self,
        frame: cam::Frame,
        out: &mut Vec<[Vec3; 3]>,
        check: &impl Fn() -> bool,
        yield_control: &mut impl FnMut() -> F,
    ) -> Result<()> {
        cancelled(check)?;
        yield_control().await;
        cancelled(check)?;
        let mesh = self.body.body.tessellate_for_verification(cam::VERIFY_ERROR / 2., 200_000usize.saturating_sub(out.len()))?;
        if out.len() + mesh.triangles.len() > 200_000 {
            return Err(error("CAM mesh exceeds verification budget"));
        }
        for (i, indices) in mesh.triangles.iter().enumerate() {
            if i.is_multiple_of(256) {
                cancelled(check)?;
                yield_control().await;
            }
            let triangle = mesh.tri(indices).ok_or_else(|| error("CAM mesh contains invalid indices"))?;
            out.push(triangle.map(|v| frame.local(apply_point(&self.placement, v))));
        }
        Ok(())
    }
}
impl Profile {
    pub(super) fn capture(s: &Session, setup: &cam::Setup, source: &cam::ProfileRef, tolerance: f64) -> Result<Self> {
        let state = s.model.state();
        let sketch = state.sketches.iter().find(|x| x.feature == source.sketch).ok_or_else(|| error("CAM sketch missing; reselect the profile"))?;
        let mut wanted = source.curves.clone();
        wanted.sort();
        let candidates: Vec<_> = sketch
            .profiles
            .iter()
            .filter(|p| {
                let mut ids = p.curve_ids();
                ids.sort();
                ids == wanted
            })
            .collect();
        if candidates.len() != 1 {
            return Err(error("CAM profile missing or ambiguous; reselect it"));
        }
        let component = s.doc.feature(source.sketch).map(|f| f.component).ok_or_else(|| error("CAM sketch feature missing"))?;
        let transform = if source.occurrence.is_empty() {
            if component != 0 {
                return Err(error("CAM sketch needs an explicit occurrence path"));
            }
            solidcraft_doc::IDENTITY
        } else {
            let p = s.occurrence_placement(&source.occurrence)?;
            if p.component != component {
                return Err(error("CAM sketch occurrence mismatch"));
            }
            p.world
        };
        let point = |p| setup.frame.local(apply_point(&transform, p));
        let origin = point(sketch.plane.origin);
        let plane =
            Plane { origin, x: point(sketch.plane.origin + sketch.plane.x) - origin, y: point(sketch.plane.origin + sketch.plane.y) - origin };
        Ok(Self { region: candidates[0].region.clone(), plane, tolerance: cam::profile_tolerance(tolerance) })
    }
    fn segments(&self, s: &Seg2) -> Result<usize> {
        let v = |p: Vec2| self.plane.x * p.x + self.plane.y * p.y;
        let m = match s {
            Seg2::Line { .. } => 0.,
            Seg2::Arc { radius, sweep, .. } => radius.abs() * sweep.powi(2) * (self.plane.x.len2() + self.plane.y.len2()).sqrt(),
            Seg2::Cubic { p0, p1, p2, p3 } => 6. * v(*p0 - *p1 * 2. + *p2).len().max(v(*p1 - *p2 * 2. + *p3).len()),
            Seg2::Bezier { n, p } => {
                let points = p.get(..=*n as usize).filter(|p| p.len() >= 2).ok_or_else(|| error("invalid CAM Bezier degree"))?;
                *n as f64 * (*n as f64 - 1.) * points.windows(3).map(|p| v(p[0] - p[1] * 2. + p[2]).len()).fold(0., f64::max)
            }
            Seg2::Conic { a, apex, b, w } => {
                if !w.is_finite() || *w <= -1. {
                    return Err(error("singular CAM conic profile"));
                }
                let u = v((*apex - *a) * *w);
                let b = v(*b - *a);
                let d = 1_f64.min((1. + w) / 2.);
                let n = u.len().max(b.len());
                let n1 = 2. * u.len().max((b - u).len());
                let n2 = 2. * (b - u * 2.).len();
                let d1 = 2. * (w - 1.).abs();
                let d2 = 4. * (w - 1.).abs();
                n2 / d + 2. * n1 * d1 / d.powi(2) + n * d2 / d.powi(2) + 2. * n * d1.powi(2) / d.powi(3)
            }
        };
        // ||curve''||/(8 n²) bounds the chord error on every parameter interval.
        // The reserve covers evaluation/placement rounding, rather than silently
        // clamping the segment count or increasing the requested tolerance.
        let size = |p: Vec2| self.plane.x.len() * p.x.abs() + self.plane.y.len() * p.y.abs();
        let magnitude = match s {
            Seg2::Line { a, b } => size(*a) + size(*b),
            Seg2::Arc { center, radius, start, sweep } => {
                size(*center) + radius.abs() * (1. + start.abs() + sweep.abs()) * (self.plane.x.len() + self.plane.y.len())
            }
            Seg2::Cubic { p0, p1, p2, p3 } => size(*p0) + size(*p1) + size(*p2) + size(*p3),
            Seg2::Bezier { n, p } => p.get(..=*n as usize).ok_or_else(|| error("invalid CAM Bezier degree"))?.iter().map(|p| size(*p)).sum(),
            Seg2::Conic { a, apex, b, w } => (size(*a) + w.abs() * size(*apex) + size(*b)) / 1_f64.min((1. + w) / 2.),
        };
        let guard = 256. * f64::EPSILON * (1. + self.plane.origin.len() + magnitude + m.abs());
        let budget = self.tolerance * 15. / 16. - guard;
        if !m.is_finite() || !budget.is_finite() || budget <= 0. {
            return Err(error("CAM profile precision cannot meet the requested tolerance"));
        }
        let count = ((m * (1. + 256. * f64::EPSILON) / (8. * budget)).sqrt().ceil()).max(1.);
        if !count.is_finite() || count > 10_000. {
            return Err(error("CAM profile exceeds 10000 vertices at the required tolerance"));
        }
        Ok(count as usize)
    }
    pub(super) async fn sample<F: std::future::Future<Output = ()>>(
        &self,
        check: &impl Fn() -> bool,
        yield_control: &mut impl FnMut() -> F,
    ) -> Result<cam::Boundary> {
        if !self.plane.origin.is_finite() || !self.plane.x.is_finite() || !self.plane.y.is_finite() || self.plane.normal().len() < 1e-12 {
            return Err(error("degenerate CAM profile placement"));
        }
        if self.plane.x.z.abs() > 1e-10 * self.plane.x.len() || self.plane.y.z.abs() > 1e-10 * self.plane.y.len() {
            return Err(error("CAM profile must be perpendicular to setup Z"));
        }
        let mut rings = Vec::new();
        let mut total = 0;
        for ring in std::iter::once(&self.region.outer).chain(&self.region.holes) {
            let mut points = Vec::new();
            for (a, b) in ring.segs.iter().zip(ring.segs.iter().cycle().skip(1)).take(ring.segs.len()) {
                let gap = self.plane.dir_to_world(a.end() - b.start()).len();
                if !gap.is_finite() || gap > self.tolerance / 16. {
                    return Err(error("disconnected CAM profile segments"));
                }
            }
            for curve in &ring.segs {
                cancelled(check)?;
                yield_control().await;
                let count = self.segments(curve)?;
                total += count;
                if total > 10_000 {
                    return Err(error("CAM profile exceeds 10000 vertices at the required tolerance"));
                }
                for i in 0..count {
                    if i.is_multiple_of(256) {
                        cancelled(check)?;
                        yield_control().await;
                    }
                    let p = self.plane.to_world(curve.point_at(i as f64 / count as f64));
                    if !p.is_finite() {
                        return Err(error("non-finite CAM profile point"));
                    }
                    points.push([p.x, p.y]);
                }
            }
            if points.first() == points.last() {
                points.pop();
            }
            rings.push(points);
        }
        let mut rings = rings.into_iter();
        let boundary = cam::Boundary { outer: rings.next().ok_or_else(|| error("empty CAM profile"))?, islands: rings.collect() };
        boundary.validate().map_err(super::error)?;
        Ok(boundary)
    }
}

pub(super) fn ready<T>(future: impl std::future::Future<Output = Result<T>>) -> Result<T> {
    use std::task::{Context, Poll, Waker};
    let mut future = std::pin::pin!(future);
    match future.as_mut().poll(&mut Context::from_waker(Waker::noop())) {
        Poll::Ready(result) => result,
        Poll::Pending => Err(error("synchronous CAM preparation unexpectedly yielded")),
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::geom::Loop2;
    fn profile() -> Profile {
        Profile { region: Region2 { outer: Loop2::circle(Vec2::ZERO, 8.), holes: vec![] }, plane: Plane::XY, tolerance: cam::profile_tolerance(0.01) }
    }
    #[test]
    fn bounded_sampler_meets_transformed_curve_chord_budget() {
        let mut p = profile();
        p.plane.x = Vec3::new(3., 1., 0.);
        p.plane.y = Vec3::new(-1., 0.5, 0.);
        p.plane.origin = Vec3::new(100., -120., 2.);
        let a = Vec2::new(2., -1.);
        let b = Vec2::new(-2., 3.);
        let c = Vec2::new(4., 8.);
        let mut curves = vec![
            Seg2::Line { a, b },
            Seg2::Arc { center: a, radius: 8., start: 0.2, sweep: -5. },
            Seg2::Cubic { p0: a, p1: c, p2: b, p3: Vec2::ZERO },
            Seg2::Bezier { n: 7, p: [a, b, c, a, b, c, a, b, c, a] },
            Seg2::Bezier { n: 9, p: [a, b, c, a, b, c, a, b, c, a] },
        ];
        for w in [-0.5, 0., 0.7, 1., 5.] {
            curves.push(Seg2::Conic { a, apex: c, b, w });
        }
        for curve in curves {
            let n = p.segments(&curve).unwrap();
            for i in 0..n {
                let a = p.plane.to_world(curve.point_at(i as f64 / n as f64));
                let b = p.plane.to_world(curve.point_at((i + 1) as f64 / n as f64));
                for k in 1..8 {
                    let fraction = k as f64 / 8.;
                    let exact = p.plane.to_world(curve.point_at((i as f64 + fraction) / n as f64));
                    assert!(exact.dist(a + (b - a) * fraction) <= p.tolerance, "{curve:?}");
                }
            }
        }
    }
    #[test]
    fn precision_and_vertex_limits_fail_without_relaxing_tolerance() {
        let mut p = profile();
        let circle = Seg2::Arc { center: Vec2::ZERO, radius: 1e6, start: 0., sweep: std::f64::consts::TAU };
        assert!(p.segments(&circle).is_err());
        p.plane.origin = Vec3::new(1e15, 0., 0.);
        assert!(p.segments(&Seg2::Line { a: Vec2::ZERO, b: Vec2::X }).is_err());
        let p = profile();
        assert!(p.segments(&Seg2::Bezier { n: 255, p: Default::default() }).is_err());
        assert!(p.segments(&Seg2::Conic { a: Vec2::ZERO, apex: Vec2::X, b: Vec2::Y, w: -1. }).is_err());
        assert!(p.segments(&Seg2::Line { a: Vec2::new(f64::NAN, 0.), b: Vec2::ZERO }).is_err());
    }
    #[test]
    fn profile_sampling_yields_and_honours_cancellation_and_planarity() {
        use std::{
            future::Future,
            sync::atomic::{AtomicBool, Ordering},
            task::{Context, Poll, Waker},
        };
        let p = profile();
        let cancel = AtomicBool::new(false);
        let check = || cancel.load(Ordering::Relaxed);
        let mut yields = super::super::YieldOnce::default;
        let mut future = std::pin::pin!(p.sample(&check, &mut yields));
        let mut cx = Context::from_waker(Waker::noop());
        assert!(future.as_mut().poll(&mut cx).is_pending());
        cancel.store(true, Ordering::Relaxed);
        assert!(matches!(future.as_mut().poll(&mut cx), Poll::Ready(Err(_))));
        let mut p = profile();
        p.plane = Plane::XZ;
        assert!(ready(p.sample(&|| false, &mut || std::future::ready(()))).is_err());
        p.plane = Plane::XY;
        p.region.outer.segs.pop();
        assert!(ready(p.sample(&|| false, &mut || std::future::ready(()))).is_err());
    }
}
