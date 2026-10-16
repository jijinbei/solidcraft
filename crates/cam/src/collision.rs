//! Independent fixture collision adapter. No toolpath generation code is used.
//! Surface casts and solid containment are both needed: a surface-only query can
//! miss a tool that starts wholly inside a closed fixture.
use crate::{
    JobInput, POST_ERROR, Result, Tool, ToolKind, invalid,
    nc::Segment,
    verify::{Finding, Limits, Report, Status},
};
use parry3d_f64::{
    math::{Pose, Vector},
    query::{ShapeCastOptions, ShapeCastStatus, cast_shapes, distance},
    shape::{Cone, Cylinder, Shape, Triangle},
};
use solidcraft_geom::Vec3;
use std::collections::BTreeMap;

#[derive(Clone, Copy)]
struct Bounds {
    min: Vec3,
    max: Vec3,
}
impl Bounds {
    fn point(p: Vec3) -> Self {
        Self { min: p, max: p }
    }
    fn include(&mut self, p: Vec3) {
        self.min = Vec3::new(self.min.x.min(p.x), self.min.y.min(p.y), self.min.z.min(p.z));
        self.max = Vec3::new(self.max.x.max(p.x), self.max.y.max(p.y), self.max.z.max(p.z));
    }
    fn overlaps(self, b: Self) -> bool {
        self.min.x <= b.max.x
            && self.max.x >= b.min.x
            && self.min.y <= b.max.y
            && self.max.y >= b.min.y
            && self.min.z <= b.max.z
            && self.max.z >= b.min.z
    }
}

struct Component {
    body: usize,
    triangles: Vec<usize>,
    bounds: Bounds,
    closed: bool,
}
struct Edge {
    first: usize,
    forward: bool,
    count: usize,
    consistent: bool,
}
type Vertex = [u64; 3];
fn vertex(p: Vec3) -> Vertex {
    [p.x, p.y, p.z].map(|v| if v == 0.0 { 0 } else { v.to_bits() })
}
fn root(parents: &mut [usize], mut i: usize) -> usize {
    while let Some(&p) = parents.get(i) {
        if p == i {
            break;
        }
        let next = parents.get(p).copied().unwrap_or(p);
        if let Some(slot) = parents.get_mut(i) {
            *slot = next;
        }
        i = next;
    }
    i
}

/// Parry's primitives have a Y axis. This orthogonal mapping puts SolidCraft's
/// downward drill tip on +Y without introducing a rounded rotation transform.
fn vector(p: Vec3) -> Vector {
    Vector::new(p.x, -p.z, p.y)
}
struct Part {
    radius: f64,
    low: f64,
    high: f64,
    cone: bool,
    label: &'static str,
}
fn parts(t: &Tool) -> Vec<Part> {
    let tip = t.tip_length();
    let mut out = Vec::with_capacity(4);
    if t.kind == ToolKind::TwistDrill {
        out.push(Part { radius: t.diameter / 2.0, low: 0.0, high: tip, cone: true, label: "cutter" });
    }
    if t.cutting_length > tip {
        out.push(Part { radius: t.diameter / 2.0, low: tip, high: t.cutting_length, cone: false, label: "cutter" });
    }
    if t.overall_length > t.cutting_length {
        out.push(Part { radius: t.shank_diameter / 2.0, low: t.cutting_length, high: t.overall_length, cone: false, label: "shank" });
    }
    out.push(Part { radius: t.holder_diameter / 2.0, low: t.overall_length, high: t.overall_length + t.holder_length, cone: false, label: "holder" });
    out
}
impl Part {
    fn center(&self, p: Vec3) -> Vec3 {
        p + Vec3::new(0.0, 0.0, (self.low + self.high) / 2.0)
    }
    fn bounds(&self, s: &Segment, margin: f64) -> Bounds {
        let radius = self.radius + margin;
        Bounds {
            min: Vec3::new(s.from.x.min(s.to.x) - radius, s.from.y.min(s.to.y) - radius, s.from.z.min(s.to.z) + self.low - margin),
            max: Vec3::new(s.from.x.max(s.to.x) + radius, s.from.y.max(s.to.y) + radius, s.from.z.max(s.to.z) + self.high + margin),
        }
    }
    fn cast(&self, s: &Segment, tri: &[Vec3; 3], margin: f64) -> Status {
        let center = vector(self.center(s.from));
        let position = Pose::translation(center.x, center.y, center.z);
        let triangle = Triangle::new(vector(tri[0]), vector(tri[1]), vector(tri[2]));
        let cylinder = Cylinder::new((self.high - self.low) / 2.0, self.radius);
        let cone = Cone::new((self.high - self.low) / 2.0, self.radius);
        let shape: &dyn Shape = if self.cone { &cone } else { &cylinder };
        let query = |distance| {
            cast_shapes(
                &position,
                vector(s.to - s.from),
                shape,
                &Pose::IDENTITY,
                Vector::ZERO,
                &triangle,
                ShapeCastOptions {
                    max_time_of_impact: 1.0,
                    target_distance: distance,
                    stop_at_penetration: true,
                    compute_impact_geometry_on_penetration: true,
                },
            )
        };
        // Third-party numerical errors or panics never certify clearance.
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            // A zero-velocity shape cast may report no impact even for initial
            // penetration. Check the initial pose independently before sweeping.
            let initial = match distance(&position, shape, &Pose::IDENTITY, &triangle).map(|d| d.distance) {
                Ok(d) if d.is_finite() && d <= 1e-10 => return Status::Fail,
                Ok(d) if d.is_finite() && d > margin => Status::Pass,
                _ => Status::Inconclusive,
            };
            match query(margin) {
                Ok(None) => initial,
                Ok(Some(_)) => match query(0.0) {
                    Ok(Some(hit))
                        if matches!(hit.status, ShapeCastStatus::Converged | ShapeCastStatus::PenetratingOrWithinTargetDist)
                            && hit.time_of_impact.is_finite()
                            && (0.0..=1.0).contains(&hit.time_of_impact) =>
                    {
                        Status::Fail
                    }
                    _ => Status::Inconclusive,
                },
                Err(_) => Status::Inconclusive,
            }
        }))
        .unwrap_or(Status::Inconclusive)
    }
}

fn finding(report: &mut Report, status: Status, s: &Segment, part: &Part, index: usize, detail: &str) {
    if status == Status::Fail || report.status == Status::Pass {
        report.status = status;
    }
    report.finding(Finding {
        severity: if status == Status::Fail { crate::verify::FindingSeverity::Error } else { crate::verify::FindingSeverity::Unresolved },
        record: None,
        source: None,
        kind: if status == Status::Fail { format!("fixture_{}_collision", part.label) } else { "fixture_clearance_unresolved".into() },
        line: Some(s.line),
        at: s.from,
        detail: format!("{} of tool {} at motion {}: {detail}", part.label, s.tool, index + 1),
    });
}

pub(crate) async fn fixtures<F: std::future::Future<Output = ()>>(
    input: &JobInput,
    segments: &[Segment],
    limits: Limits,
    cancelled: &impl Fn() -> bool,
    yield_control: &mut impl FnMut() -> F,
    report: &mut Report,
) -> Result<()> {
    let tris = &input.geometry.fixtures;
    if tris.is_empty() {
        return Ok(());
    }
    let mut parents: Vec<_> = (0..tris.len()).collect();
    let mut edges: BTreeMap<(usize, [Vertex; 2]), Edge> = BTreeMap::new();
    let bodies: Vec<usize> = if input.geometry.fixture_groups.is_empty() {
        vec![0; tris.len()]
    } else {
        input.geometry.fixture_groups.iter().enumerate().flat_map(|(body, &count)| std::iter::repeat_n(body, count)).collect()
    };
    let mut degenerate = Vec::new();
    for (i, tri) in tris.iter().enumerate() {
        if i.is_multiple_of(1024) {
            yield_control().await;
            if cancelled() {
                return invalid("simulation cancelled");
            }
        }
        report.work += 3;
        if report.work > limits.work {
            return Ok(());
        }
        if (tri[1] - tri[0]).cross(tri[2] - tri[0]).len() < 1e-12 {
            degenerate.push(i);
        }
        for (a, b) in [(tri[0], tri[1]), (tri[1], tri[2]), (tri[2], tri[0])] {
            let a = vertex(a);
            let b = vertex(b);
            let forward = a < b;
            let key = (bodies.get(i).copied().unwrap_or(0), if forward { [a, b] } else { [b, a] });
            if let Some(e) = edges.get_mut(&key) {
                let a = root(&mut parents, e.first);
                let b = root(&mut parents, i);
                if let Some(slot) = parents.get_mut(b) {
                    *slot = a;
                }
                e.count += 1;
                e.consistent &= e.forward != forward;
            } else {
                edges.insert(key, Edge { first: i, forward, count: 1, consistent: true });
            }
        }
    }
    let mut components: BTreeMap<usize, Component> = BTreeMap::new();
    for (i, tri) in tris.iter().enumerate() {
        let key = root(&mut parents, i);
        let c = components.entry(key).or_insert_with(|| Component {
            body: bodies.get(i).copied().unwrap_or(0),
            triangles: Vec::new(),
            bounds: Bounds::point(tri[0]),
            closed: true,
        });
        c.triangles.push(i);
        for p in tri {
            c.bounds.include(*p);
        }
        if i.is_multiple_of(1024) {
            yield_control().await;
            if cancelled() {
                return invalid("simulation cancelled");
            }
        }
    }
    for (index, e) in edges.values().enumerate() {
        if index.is_multiple_of(1024) {
            yield_control().await;
            if cancelled() {
                return invalid("simulation cancelled");
            }
        }
        if (e.count != 2 || !e.consistent)
            && let Some(c) = components.get_mut(&root(&mut parents, e.first))
        {
            c.closed = false;
        }
    }
    for i in degenerate {
        if let Some(c) = components.get_mut(&root(&mut parents, i)) {
            c.closed = false;
        }
    }
    let mut body_closed = BTreeMap::new();
    for c in components.values() {
        let closed = body_closed.entry(c.body).or_insert(true);
        *closed &= c.closed;
    }
    let margin = POST_ERROR + limits.uncertainty;
    for (index, s) in segments.iter().enumerate() {
        yield_control().await;
        if cancelled() {
            return invalid("simulation cancelled");
        }
        let tool = input.tools.iter().find(|t| t.id == s.tool).ok_or_else(|| crate::Error("unknown simulation tool".into()))?;
        for part in parts(tool) {
            let swept = part.bounds(s, margin);
            let mut windings: BTreeMap<usize, f64> = BTreeMap::new();
            for c in components.values() {
                report.work += 1;
                if report.work.is_multiple_of(1024) {
                    yield_control().await;
                    if cancelled() {
                        return invalid("simulation cancelled");
                    }
                }
                if report.work > limits.work {
                    return Ok(());
                }
                if !swept.overlaps(c.bounds) {
                    continue;
                }
                let center = part.center(s.from);
                let containment = Bounds::point(center).overlaps(c.bounds);
                let mut solid_angle = 0.0;
                let mut status = if c.closed { Status::Pass } else { Status::Inconclusive };
                for &i in &c.triangles {
                    report.work += 1;
                    if report.work.is_multiple_of(1024) {
                        yield_control().await;
                        if cancelled() {
                            return invalid("simulation cancelled");
                        }
                    }
                    if report.work > limits.work {
                        return Ok(());
                    }
                    let Some(tri) = tris.get(i) else {
                        return invalid("invalid fixture triangle index");
                    };
                    let mut bounds = Bounds::point(tri[0]);
                    bounds.include(tri[1]);
                    bounds.include(tri[2]);
                    if swept.overlaps(bounds) {
                        let cast = part.cast(s, tri, margin);
                        if cast == Status::Fail {
                            status = Status::Fail;
                            break;
                        }
                        if cast == Status::Inconclusive {
                            status = Status::Inconclusive;
                        }
                    }
                    if c.closed && containment {
                        let [a, b, d] = tri.map(|v| v - center);
                        let denominator = a.len() * b.len() * d.len() + a.dot(b) * d.len() + b.dot(d) * a.len() + d.dot(a) * b.len();
                        solid_angle += 2.0 * a.dot(b.cross(d)).atan2(denominator);
                    }
                }
                if status != Status::Fail && c.closed && containment {
                    *windings.entry(c.body).or_default() += solid_angle / (4.0 * std::f64::consts::PI);
                }
                if status != Status::Pass {
                    finding(
                        report,
                        status,
                        s,
                        &part,
                        index,
                        if status == Status::Fail {
                            "swept shape contacts the fixture surface"
                        } else {
                            "clearance is within the error margin, the query did not converge, or fixture topology is open/degenerate"
                        },
                    );
                }
            }
            for (body, winding) in &windings {
                let status = if !body_closed.get(body).copied().unwrap_or(false) {
                    Status::Inconclusive
                } else if winding.is_finite() && winding.abs() > 1.0 - 1e-6 {
                    Status::Fail
                } else if !winding.is_finite() || winding.abs() > 1e-6 {
                    Status::Inconclusive
                } else {
                    Status::Pass
                };
                if status != Status::Pass {
                    finding(report, status, s, &part, index, "initial tool volume is inside a fixture or its solid winding is unresolved");
                }
            }
        }
    }
    Ok(())
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::{corpus, nc, verify};

    fn box_mesh(min: Vec3, max: Vec3) -> Vec<[Vec3; 3]> {
        let p = [
            min,
            Vec3::new(max.x, min.y, min.z),
            Vec3::new(max.x, max.y, min.z),
            Vec3::new(min.x, max.y, min.z),
            Vec3::new(min.x, min.y, max.z),
            Vec3::new(max.x, min.y, max.z),
            max,
            Vec3::new(min.x, max.y, max.z),
        ];
        [[0, 3, 2, 1], [4, 5, 6, 7], [0, 1, 5, 4], [1, 2, 6, 5], [2, 3, 7, 6], [3, 0, 4, 7]]
            .into_iter()
            .flat_map(|q| [[p[q[0]], p[q[1]], p[q[2]]], [p[q[0]], p[q[2]], p[q[3]]]])
            .collect()
    }
    fn run(mesh: Vec<[Vec3; 3]>, motion: &str) -> Report {
        run_groups(mesh, vec![], motion)
    }
    fn run_groups(mesh: Vec<[Vec3; 3]>, groups: Vec<usize>, motion: &str) -> Report {
        let mut input = corpus::fixture("face").unwrap();
        input.geometry.fixtures = mesh;
        input.geometry.fixture_groups = groups;
        let src = format!("G21 G90 G17 G94 G54\nG0 X0 Y0 Z15\nT1 M6\n{motion}\nM30");
        let parsed = nc::parse(&src, &input.tools).unwrap();
        verify::simulate(&input, &parsed, Limits::default()).unwrap()
    }
    #[test]
    fn near_fixture_is_clear_below_wide_holder() {
        // Old whole-tool AABB overlaps this fixture. Cutter/shank radius is 3;
        // the 10-radius holder starts at Z65 and cannot touch this low block.
        let r = run(box_mesh(Vec3::new(4.5, 0.0, 16.0), Vec3::new(5.0, 1.0, 17.0)), "G0 X1");
        assert_eq!(r.status, Status::Pass, "{:?}", r.findings);
    }
    #[test]
    fn thin_fixture_between_endpoints_is_a_collision() {
        let r = run(box_mesh(Vec3::new(5.0, -1.0, 16.0), Vec3::new(5.01, 1.0, 17.0)), "G0 X10");
        assert_eq!(r.status, Status::Fail);
        assert!(r.findings.iter().any(|f| f.kind == "fixture_cutter_collision" && f.line == Some(4)));
    }
    #[test]
    fn holder_and_shank_collisions_are_distinguished() {
        for (z, kind) in [(40.0, "shank"), (70.0, "holder")] {
            let r = run(box_mesh(Vec3::new(5.0, -1.0, z), Vec3::new(5.1, 1.0, z + 1.0)), "G0 X10");
            assert_eq!(r.status, Status::Fail);
            assert!(r.findings.iter().any(|f| f.kind == format!("fixture_{kind}_collision")));
        }
    }
    #[test]
    fn contained_tool_is_not_missed_and_winding_is_irrelevant() {
        let cube = box_mesh(Vec3::new(-20.0, -20.0, 10.0), Vec3::new(20.0, 20.0, 100.0));
        for reverse in [false, true] {
            let mesh = cube.iter().map(|t| if reverse { [t[2], t[1], t[0]] } else { *t }).collect();
            let r = run(mesh, "G0 X1");
            assert_eq!(r.status, Status::Fail, "{:?}", r.findings);
        }
    }
    #[test]
    fn overlapping_components_with_opposite_winding_do_not_cancel() {
        let mut a = box_mesh(Vec3::new(-20.0, -20.0, 10.0), Vec3::new(20.0, 20.0, 100.0));
        a.extend(box_mesh(Vec3::new(-21.0, -21.0, 9.0), Vec3::new(21.0, 21.0, 101.0)).into_iter().map(|t| [t[2], t[1], t[0]]));
        assert_eq!(run_groups(a, vec![12, 12], "G0 X1").status, Status::Fail);
    }
    #[test]
    fn cavity_in_one_fixture_body_remains_empty() {
        let mut a = box_mesh(Vec3::new(-20.0, -20.0, 10.0), Vec3::new(20.0, 20.0, 100.0));
        a.extend(box_mesh(Vec3::new(-15.0, -15.0, 11.0), Vec3::new(15.0, 15.0, 99.0)).into_iter().map(|t| [t[2], t[1], t[0]]));
        assert_eq!(run_groups(a, vec![24], "G0 X1").status, Status::Pass);
    }
    #[test]
    fn malformed_fixture_group_counts_are_rejected() {
        let mut input = corpus::fixture("face").unwrap();
        input.geometry.fixture_groups = vec![usize::MAX, 1];
        assert!(input.validate().is_err());
        input.geometry.fixture_groups = vec![0];
        assert!(input.validate().is_err());
    }
    #[test]
    fn open_containing_shell_is_inconclusive() {
        let mut cube = box_mesh(Vec3::new(-20.0, -20.0, 10.0), Vec3::new(20.0, 20.0, 100.0));
        cube.pop();
        assert_eq!(run(cube, "G0 X1").status, Status::Inconclusive);
    }
    #[test]
    fn cylinder_corners_and_margin_are_resolved() {
        let part = Part { radius: 1.0, low: 0.0, high: 2.0, cone: false, label: "cutter" };
        let s = Segment { from: Vec3::ZERO, to: Vec3::ZERO, rapid: true, feed: 0.0, tool: 1, line: 4 };
        let triangle = |x, y| [Vec3::new(x, y, 0.5), Vec3::new(x + 0.05, y, 1.5), Vec3::new(x, y + 0.05, 1.5)];
        assert_eq!(part.cast(&s, &triangle(0.8, 0.8), 0.0035), Status::Pass);
        assert_eq!(part.cast(&s, &triangle(0.5, 0.5), 0.0035), Status::Fail);
        assert_ne!(part.cast(&s, &triangle(1.001, 0.0), 0.0035), Status::Pass);
    }
    #[test]
    fn drill_point_shape_has_clearance_outside_its_cone() {
        let part = Part { radius: 1.0, low: 0.0, high: 1.0, cone: true, label: "cutter" };
        let s = Segment { from: Vec3::ZERO, to: Vec3::ZERO, rapid: false, feed: 100.0, tool: 1, line: 4 };
        let clear = [Vec3::new(0.8, 0.0, 0.1), Vec3::new(0.9, 0.0, 0.1), Vec3::new(0.8, 0.1, 0.2)];
        assert_eq!(part.cast(&s, &clear, 0.0035), Status::Pass);
        let contact = clear.map(|p| p + Vec3::new(0.0, 0.0, 0.8));
        assert_eq!(part.cast(&s, &contact, 0.0035), Status::Fail);
    }
    #[test]
    fn fixture_budget_and_cancellation_never_pass() {
        let mut input = corpus::fixture("face").unwrap();
        input.geometry.fixtures = box_mesh(Vec3::new(4.5, 0.0, 16.0), Vec3::new(5.0, 1.0, 17.0));
        let parsed = nc::parse("G21 G90 G17 G94 G54\nG0 X0 Y0 Z15\nT1 M6\nG0 X1\nM30", &input.tools).unwrap();
        let r = verify::simulate(&input, &parsed, Limits { work: 1, ..Limits::default() }).unwrap();
        assert_eq!(r.status, Status::Inconclusive);
        assert!(verify::simulate_cancellable(&input, &parsed, Limits::default(), || true).is_err());
    }
}
