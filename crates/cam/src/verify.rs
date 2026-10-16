//! Independent bounded stock verifier. Adaptive cells carry height intervals;
//! unresolved intervals and exhausted work budgets never become a pass.
use crate::{
    nc::{Parsed, Segment},
    *,
};
use serde::{Deserialize, Serialize};
use solidcraft_geom::Vec3;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    Pass,
    Fail,
    Inconclusive,
}
#[derive(Clone, Copy, Default, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FindingSeverity {
    Information,
    #[default]
    Unresolved,
    Error,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Finding {
    #[serde(default)]
    pub severity: FindingSeverity,
    pub kind: String,
    pub line: Option<usize>,
    /// One-based interpreted NC record, not a tessellated arc segment.
    #[serde(default)]
    pub record: Option<usize>,
    /// Present only for an independently checked SolidCraft post round trip.
    #[serde(default)]
    pub source: Option<nc::SourceLocation>,
    pub at: Vec3,
    pub detail: String,
}
impl Finding {
    fn priority(&self) -> (FindingSeverity, bool) {
        (self.severity, self.line.is_none())
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Cell {
    pub min: [f64; 2],
    pub max: [f64; 2],
    pub low: f64,
    pub high: f64,
    pub target_low: f64,
    pub target_high: f64,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Report {
    pub version: String,
    pub fingerprint: String,
    pub status: Status,
    /// Separate from path clearance. Unsupported finished surfaces stay inconclusive.
    pub finished_dimensions: Status,
    pub post_version: String,
    pub tolerance: f64,
    /// Requested spatial refinement budget, not a claim about height uncertainty.
    pub uncertainty: f64,
    pub maximum_height_interval: f64,
    /// False when resource limits prevented evaluating all stock cells/motions.
    #[serde(default)]
    pub stock_complete: bool,
    /// Comparison domain. Legacy reports without this cannot certify coverage.
    #[serde(default)]
    pub stock: Option<Stock>,
    pub cells: Vec<Cell>,
    pub findings: Vec<Finding>,
    pub cutting_distance: f64,
    pub rapid_distance: f64,
    pub cutting_minutes: f64,
    pub remaining_volume_low: f64,
    pub remaining_volume_high: f64,
    pub work: usize,
}
impl Report {
    /// Bounded representative witnesses. Repeated cells on one NC line must not
    /// crowd out a definite failure found later in the verification traversal.
    pub(crate) fn finding(&mut self, finding: Finding) {
        if self.findings.iter().any(|old| old.kind == finding.kind && old.line == finding.line && old.severity == finding.severity) {
            return;
        }
        if self.findings.len() < 200 {
            self.findings.push(finding);
        } else if let Some((index, old)) = self.findings.iter().enumerate().min_by_key(|(_, old)| old.priority())
            && old.priority() < finding.priority()
            && let Some(slot) = self.findings.get_mut(index)
        {
            *slot = finding;
        }
    }
}
#[derive(Clone, Copy, Debug)]
pub struct Limits {
    pub cells: usize,
    pub work: usize,
    pub uncertainty: f64,
}
impl Default for Limits {
    fn default() -> Self {
        Self { cells: 250_000, work: 30_000_000, uncertainty: VERIFY_ERROR }
    }
}

pub fn simulate(input: &JobInput, nc: &Parsed, limits: Limits) -> Result<Report> {
    simulate_cancellable(input, nc, limits, || false)
}
pub fn simulate_cancellable(input: &JobInput, nc: &Parsed, limits: Limits, cancelled: impl Fn() -> bool) -> Result<Report> {
    use std::future::Future;
    let mut future = std::pin::pin!(simulate_cooperative(input, nc, limits, cancelled, || std::future::ready(())));
    match future.as_mut().poll(&mut std::task::Context::from_waker(std::task::Waker::noop())) {
        std::task::Poll::Ready(result) => result,
        std::task::Poll::Pending => invalid("synchronous verifier unexpectedly yielded"),
    }
}
/// Portable host-driven verification. A host supplies a yield future to bound a poll's work.
/// This contains no executor, OS thread, browser or generator dependency.
pub async fn simulate_cooperative<F: std::future::Future<Output = ()>>(
    input: &JobInput,
    nc: &Parsed,
    limits: Limits,
    cancelled: impl Fn() -> bool,
    mut yield_control: impl FnMut() -> F,
) -> Result<Report> {
    input.validate()?;
    if limits.cells == 0
        || limits.cells > 1_000_000
        || limits.work == 0
        || limits.work > 100_000_000
        || !limits.uncertainty.is_finite()
        || limits.uncertainty <= 0.0
        || limits.uncertainty > VERIFY_ERROR
    {
        return invalid("invalid verifier limits");
    }
    if nc.work_offset != input.setup.work_offset {
        return invalid("NC/setup work-offset mismatch");
    }
    if input.geometry.target.is_empty() {
        return invalid("no target geometry to verify");
    }
    if input.geometry.target.len() + input.geometry.fixtures.len() > 200_000 {
        return invalid("verification mesh limit");
    }
    for t in input.geometry.target.iter().chain(&input.geometry.fixtures) {
        for p in t {
            crate::model::point(*p)?;
        }
    }
    let segments = nc::segments(nc, &input.setup)?;
    let tol = input.operations.iter().map(|o| o.tolerance).fold(TOLERANCE, f64::min);
    let stock = input.setup.stock;
    // Only motions whose cutter or non-cutting envelopes can reach stock need
    // cell queries. Keep original sequence IDs for machine-state/withdrawal checks.
    let motion_index = crate::spatial::Index::new(
        segments
            .iter()
            .enumerate()
            .filter_map(|(id, s)| {
                let tool = input.tools.iter().find(|t| t.id == s.tool)?;
                let z = s.from.z.min(s.to.z);
                let mut radius = if z <= stock.max.z + POST_ERROR { tool.diameter / 2. + POST_ERROR } else { 0. };
                for (r, lift) in [(tool.shank_diameter / 2., tool.cutting_length), (tool.holder_diameter / 2., tool.overall_length)] {
                    if z + lift <= stock.max.z + POST_ERROR {
                        radius = radius.max(r + POST_ERROR);
                    }
                }
                if radius == 0. {
                    return None;
                }
                Some((
                    id,
                    [s.from.x.min(s.to.x) - radius, s.from.y.min(s.to.y) - radius],
                    [s.from.x.max(s.to.x) + radius, s.from.y.max(s.to.y) + radius],
                ))
            })
            .collect(),
    );
    let surface = crate::surface::Surface::new(&input.geometry.target, stock.min.z)?;
    let finished = crate::dimensions::Finished::new(input, tol)?;
    let dimension_resolution = finished.as_ref().map_or(limits.uncertainty, |f| limits.uncertainty.min(f.refinement));
    let mut r = Report {
        version: VERSION.into(),
        fingerprint: input.fingerprint.clone(),
        status: Status::Pass,
        finished_dimensions: if finished.is_some() { Status::Pass } else { Status::Inconclusive },
        post_version: "reference-three-axis/1".into(),
        tolerance: tol,
        uncertainty: limits.uncertainty,
        maximum_height_interval: 0.0,
        stock_complete: true,
        stock: Some(stock),
        cells: Vec::new(),
        findings: Vec::new(),
        cutting_distance: 0.0,
        rapid_distance: 0.0,
        cutting_minutes: 0.0,
        remaining_volume_low: 0.0,
        remaining_volume_high: 0.0,
        work: 0,
    };
    if finished.is_none() {
        r.finding(Finding {
            record: None,
            severity: FindingSeverity::Unresolved,
            source: None,
            kind: "finished_region_unsupported".into(),
            line: None,
            at: stock.min,
            detail: "Finished-region certification needs a tolerance larger than the post/interpolation error budget".into(),
        });
    }
    for s in &segments {
        let d = s.from.dist(s.to);
        if s.rapid {
            r.rapid_distance += d;
        } else {
            r.cutting_distance += d;
            r.cutting_minutes += d / s.feed;
        }
    }
    crate::collision::fixtures(input, &segments, limits, &cancelled, &mut yield_control, &mut r).await?;
    let mut pending = vec![([stock.min.x, stock.min.y], [stock.max.x, stock.max.y], 0u8)];
    let mut next_yield = 0;
    while let Some((min, max, depth)) = pending.pop() {
        if r.work >= next_yield {
            next_yield = r.work.saturating_add(4096);
            yield_control().await;
        }
        if cancelled() {
            return invalid("simulation cancelled");
        }
        if r.work > limits.work || r.cells.len() + pending.len() >= limits.cells {
            r.stock_complete = false;
            r.status = if r.status == Status::Fail { Status::Fail } else { Status::Inconclusive };
            r.finding(Finding {
                record: None,
                severity: FindingSeverity::Unresolved,
                source: None,
                kind: "budget_exhausted".into(),
                line: None,
                at: stock.min,
                detail: "Adaptive verification incomplete; increase resource budget or simplify the job".into(),
            });
            break;
        }
        let center = [(min[0] + max[0]) / 2.0, (min[1] + max[1]) / 2.0];
        let half = ((max[0] - min[0]).hypot(max[1] - min[1])) / 2.0;
        let mut low = stock.max.z;
        let mut high = stock.max.z;
        let mut collision = None;
        let mut removal_line = None;
        let mut candidates = Vec::new();
        motion_index.query(min, max, &mut candidates);
        candidates.sort_unstable();
        for (query_index, segment_index) in candidates.into_iter().enumerate() {
            let seg = &segments[segment_index];
            r.work += 1;
            if query_index % 4096 == 4095 {
                yield_control().await;
                if cancelled() {
                    return invalid("simulation cancelled");
                }
            }
            if r.work > limits.work {
                break;
            }
            let Some(tool) = input.tools.iter().find(|t| t.id == seg.tool) else {
                continue;
            };
            let radius = tool.diameter / 2.0;
            let (distance, zlo, zhi) = sweep(center, half, seg, radius);
            // A vertical drill has exact radial extrema on a rectangular cell.
            // Circumscribed disks are unnecessarily loose here and can create
            // artificial saddles where coarse and fine cone cells meet.
            let (near, far) = if tool.kind == ToolKind::TwistDrill && seg.from.x == seg.to.x && seg.from.y == seg.to.y {
                radial_bounds(min, max, [seg.from.x, seg.from.y])
            } else if seg.from.z == seg.to.z || (seg.from.x == seg.to.x && seg.from.y == seg.to.y) {
                lateral_bounds(min, max, seg)
            } else {
                // Sloping motion still uses a common covering disk: its upper
                // height bound must cover the entire cell at the same pose.
                ((distance - half).max(0.0), distance + half)
            };
            let cone_low = if tool.kind == ToolKind::TwistDrill { near / (tool.tip_angle / 2.0).tan() } else { 0.0 };
            let cone_high = if tool.kind == ToolKind::TwistDrill { far / (tool.tip_angle / 2.0).tan() } else { 0.0 };
            let overlaps = near <= radius + POST_ERROR;
            // A vertical withdrawal starts at the endpoint of the preceding feed
            // with the same tool; this exact geometric fact needs no raster estimate.
            let withdrawal = segment_index.checked_sub(1).and_then(|i| segments.get(i)).is_some_and(|previous| {
                !previous.rapid
                    && previous.tool == seg.tool
                    && previous.to == seg.from
                    && seg.from.x == seg.to.x
                    && seg.from.y == seg.to.y
                    && seg.to.z >= seg.from.z
            });
            if seg.rapid && overlaps && !withdrawal {
                if zhi + cone_high < low - tol && far <= radius {
                    collision = Some(("rapid_through_stock", seg.line));
                } else if zlo + cone_low < high - tol {
                    collision = Some(("rapid_clearance_unresolved", seg.line));
                }
            } else if !seg.rapid && overlaps {
                low = low.min(zlo + cone_low).max(stock.min.z);
                if far <= radius {
                    let next = high.min(zhi + cone_high).max(stock.min.z);
                    if next < high {
                        removal_line = Some(seg.line);
                    }
                    high = next;
                }
            }
            // Shank and holder sweeps against remaining stock, separate from cutter removal.
            for (rad, lift, label) in [
                (tool.shank_diameter / 2.0, tool.cutting_length, "shank_collision"),
                (tool.holder_diameter / 2.0, tool.overall_length, "holder_collision"),
            ] {
                let (d, z, _) = sweep(center, half, seg, rad);
                if d + half < rad && z + lift < low - tol {
                    collision = Some((label, seg.line));
                } else if d < rad + half && z + lift < high - tol {
                    collision = Some(("holder_clearance_unresolved", seg.line));
                }
            }
        }
        let (target_low, target_high, _) = surface.bounds(center, (max[0] - min[0]).max(max[1] - min[1]) / 2.0, &mut r.work, limits.work);
        let protection = surface
            .protect(
                crate::surface::Region { center, half, low, high, target_high, tolerance: tol },
                &mut r.work,
                limits,
                &cancelled,
                &mut yield_control,
            )
            .await?;
        let rest = low - target_high > tol;
        let expected =
            if let Some(f) = &finished { f.envelope((center, half), &mut r.work, limits.work, &cancelled, &mut yield_control).await? } else { None };
        let dimension = expected.map(|e| e.classify(low, high));
        // Refine both the machined result and the requested finished features:
        // an omitted hole must not hide inside one large untouched stock cell.
        let unresolved = ((high - low > tol / 2.0 || protection == Status::Inconclusive || collision.is_some()) && half > limits.uncertainty)
            || (dimension == Some(Status::Inconclusive) && half > dimension_resolution);
        if unresolved && depth < 24 && r.work <= limits.work {
            pending.extend([
                (min, center, depth + 1),
                ([center[0], min[1]], [max[0], center[1]], depth + 1),
                ([min[0], center[1]], [center[0], max[1]], depth + 1),
                (center, max, depth + 1),
            ]);
            continue;
        }
        let at = Vec3::new(center[0], center[1], low);
        if let Some(dimension) = dimension {
            if dimension == Status::Fail {
                r.finished_dimensions = Status::Fail;
                r.finding(Finding {
                    record: None,
                    severity: FindingSeverity::Error,
                    source: None,
                    kind: "finished_region_error".into(),
                    line: removal_line,
                    at,
                    detail: expected.map_or_else(String::new, |e| e.description()),
                });
            } else if dimension == Status::Inconclusive && r.finished_dimensions == Status::Pass {
                r.finished_dimensions = Status::Inconclusive;
            }
        }
        if let Some((kind, line)) = collision {
            if !kind.ends_with("unresolved") {
                r.status = Status::Fail;
            } else if r.status == Status::Pass {
                r.status = Status::Inconclusive;
            }
            r.finding(Finding {
                record: None,
                severity: if kind.ends_with("unresolved") { FindingSeverity::Unresolved } else { FindingSeverity::Error },
                source: None,
                kind: kind.into(),
                line: Some(line),
                at,
                detail: "Swept cutter or non-cutting tool envelope intersects remaining stock".into(),
            });
        }
        if protection == Status::Fail {
            r.status = Status::Fail;
            r.finding(Finding {
                record: None,
                severity: FindingSeverity::Error,
                source: None,
                kind: "gouge".into(),
                line: removal_line,
                at,
                detail: format!("A definitely removed point lies more than {:.6} mm inside the protected height-field solid", tol),
            });
        }
        // Rest is reported separately: intermediate roughing is not required to finish a part.
        if rest && !r.findings.iter().any(|f| f.kind == "remaining_material") {
            r.finding(Finding {
                record: None,
                severity: FindingSeverity::Information,
                source: None,
                kind: "remaining_material".into(),
                line: None,
                at,
                detail: format!("At least {:.6} mm above target; may be intentional outside selected finished regions", low - target_high),
            });
        }
        if protection == Status::Inconclusive {
            if r.status == Status::Pass {
                r.status = Status::Inconclusive;
            }
            r.finding(Finding {
                record: None,
                severity: FindingSeverity::Unresolved,
                source: None,
                kind: "protected_surface_unresolved".into(),
                line: removal_line,
                at,
                detail: "Local surface-distance bounds cannot yet certify the protected CAD boundary at this resolution or work budget".into(),
            });
        }
        let area = (max[0] - min[0]) * (max[1] - min[1]);
        r.remaining_volume_low += area * (low - stock.min.z);
        r.remaining_volume_high += area * (high - stock.min.z);
        r.maximum_height_interval = r.maximum_height_interval.max(high - low);
        r.cells.push(Cell { min, max, low, high, target_low, target_high });
    }
    if r.work > limits.work {
        r.stock_complete = false;
        if r.status == Status::Pass {
            r.status = Status::Inconclusive;
        }
    }
    if (!r.stock_complete || r.status != Status::Pass) && r.finished_dimensions == Status::Pass {
        r.finished_dimensions = Status::Inconclusive;
    }
    r.findings.sort_by_key(|finding| std::cmp::Reverse(finding.priority()));
    for finding in &mut r.findings {
        if let Some(line) = finding.line {
            finding.record = nc.record_for_line(line).map(|i| i + 1);
            finding.source = nc.source_for_line(line);
        }
    }
    Ok(r)
}

/// Distance to XY sweep and conservative height range over the part of a segment
/// intersecting a disk expanded by the cell radius.
fn sweep(p: [f64; 2], half: f64, s: &Segment, radius: f64) -> (f64, f64, f64) {
    let dx = s.to.x - s.from.x;
    let dy = s.to.y - s.from.y;
    let l2 = dx * dx + dy * dy;
    let t = if l2 > 1e-20 { ((p[0] - s.from.x) * dx + (p[1] - s.from.y) * dy) / l2 } else { 0.0 };
    let c = t.clamp(0.0, 1.0);
    let d = (p[0] - s.from.x - c * dx).hypot(p[1] - s.from.y - c * dy);
    let minimum = |r: f64| {
        if l2 <= 1e-20 {
            return s.from.z.min(s.to.z);
        }
        let perpendicular = ((p[0] - s.from.x).powi(2) + (p[1] - s.from.y).powi(2) - t * t * l2).max(0.0);
        let dt = ((r * r - perpendicular).max(0.0) / l2).sqrt();
        let z0 = s.from.z + (s.to.z - s.from.z) * (t - dt).clamp(0.0, 1.0);
        let z1 = s.from.z + (s.to.z - s.from.z) * (t + dt).clamp(0.0, 1.0);
        z0.min(z1)
    };
    (d, minimum(radius + half), minimum((radius - half).max(0.0)))
}
fn radial_bounds(min: [f64; 2], max: [f64; 2], center: [f64; 2]) -> (f64, f64) {
    let axis = |i: usize| {
        let near = (min[i] - center[i]).max(center[i] - max[i]).max(0.0);
        let far = (min[i] - center[i]).abs().max((max[i] - center[i]).abs());
        (near, far)
    };
    let (x, y) = (axis(0), axis(1));
    (x.0.hypot(y.0), x.1.hypot(y.1))
}

/// Conservative rectangle-to-segment distance bounds. The segment AABB gives
/// an additional lower bound, exact for axis-aligned sweeps. Distance to a
/// convex segment is convex, so its maximum over a rectangle is attained at a
/// corner. Circumscribed disks alone invent overlap along long straight walls
/// and force unnecessary refinement of already resolved stock.
fn lateral_bounds(min: [f64; 2], max: [f64; 2], s: &Segment) -> (f64, f64) {
    let a = [s.from.x, s.from.y];
    let b = [s.to.x, s.to.y];
    let delta = [b[0] - a[0], b[1] - a[1]];
    let length2 = delta[0] * delta[0] + delta[1] * delta[1];
    let gap = |k: usize| (min[k] - a[k].max(b[k])).max(a[k].min(b[k]) - max[k]).max(0.);
    let distance = |p: [f64; 2]| {
        let t = if length2 > 0. { (((p[0] - a[0]) * delta[0] + (p[1] - a[1]) * delta[1]) / length2).clamp(0., 1.) } else { 0. };
        (p[0] - a[0] - t * delta[0]).hypot(p[1] - a[1] - t * delta[1])
    };
    let mut far = 0_f64;
    for p in [min, [min[0], max[1]], max, [max[0], min[1]]] {
        far = far.max(distance(p));
    }
    let center_distance = distance([(min[0] + max[0]) / 2., (min[1] + max[1]) / 2.]);
    let half = (max[0] - min[0]).hypot(max[1] - min[1]) / 2.;
    let extent = min.into_iter().chain(max).chain(a).chain(b).map(f64::abs).fold(1_f64, f64::max);
    let guard = 128. * f64::EPSILON * extent;
    (((center_distance - half).max(gap(0).hypot(gap(1))) - guard).max(0.), far + guard)
}

pub(crate) fn target_height(p: [f64; 2], half: f64, tris: &[[Vec3; 3]], bottom: f64, work: &mut usize, budget: usize) -> (f64, f64, bool) {
    let mut lo = f64::INFINITY;
    let mut hi = bottom;
    let mut area = 0.0;
    for tri in tris {
        *work += 1;
        if *work > budget {
            return (bottom, hi, true);
        }
        let [a, b, c] = *tri;
        // Positive-Z faces form the top envelope of supported height-field solids.
        if (b - a).cross(c - a).z <= 0.0 {
            continue;
        }
        let mut poly = tri.to_vec();
        for (axis, bound, greater) in [(0, p[0] - half, true), (0, p[0] + half, false), (1, p[1] - half, true), (1, p[1] + half, false)] {
            let old = std::mem::take(&mut poly);
            if old.is_empty() {
                break;
            }
            let value = |v: Vec3| if axis == 0 { v.x } else { v.y };
            let inside = |v: Vec3| if greater { value(v) >= bound } else { value(v) <= bound };
            for (a, b) in old.iter().zip(old.iter().cycle().skip(1)).take(old.len()) {
                if inside(*a) {
                    poly.push(*a);
                }
                if inside(*a) != inside(*b) {
                    let t = (bound - value(*a)) / (value(*b) - value(*a));
                    poly.push(*a + (*b - *a) * t);
                }
            }
        }
        if poly.len() < 3 {
            continue;
        }
        let origin = poly[0];
        let covered =
            poly.iter().zip(poly.iter().cycle().skip(1)).take(poly.len()).map(|(a, b)| (*a - origin).cross(*b - origin).z).sum::<f64>().abs() / 2.0;
        if covered <= 0.0 {
            continue;
        }
        area += covered;
        for v in poly {
            lo = lo.min(v.z);
            hi = hi.max(v.z);
        }
    }
    if area == 0.0 {
        return (bottom, bottom, false);
    }
    let complete = area >= 4.0 * half * half * (1.0 - 1e-8);
    (if complete { lo } else { bottom }, hi, !complete)
}
/// Export a conforming conservative stock boundary. Tiny saddle closure may add
/// at most `STOCK_EXPORT_ERROR` to a cell height; it never removes stock.
pub fn stock_stl(report: &Report, bottom: f64) -> Result<String> {
    crate::stock_mesh::stl(report, bottom)
}

/// Incremental extraction and ASCII serialization. Cancellation returns no partial artifact.
pub async fn stock_stl_cooperative<F: std::future::Future<Output = ()>>(
    report: &Report,
    bottom: f64,
    cancelled: impl Fn() -> bool,
    yield_control: impl FnMut() -> F,
) -> Result<String> {
    crate::stock_mesh::stl_cooperative(report, bottom, cancelled, yield_control).await
}

/// The same conforming conservative boundary as `stock_stl`, without serialization.
/// Coordinates are in the setup frame. An empty mesh represents fully removed stock.
pub fn stock_triangles(report: &Report, bottom: f64) -> Result<Vec<[Vec3; 3]>> {
    crate::stock_mesh::triangles(report, bottom)
}

/// Cancellation-aware boundary extraction, yielding during sorting, stitching,
/// triangulation and manifold checks. No partial mesh is returned.
pub async fn stock_triangles_cooperative<F: std::future::Future<Output = ()>>(
    report: &Report,
    bottom: f64,
    cancelled: impl Fn() -> bool,
    yield_control: impl FnMut() -> F,
) -> Result<Vec<[Vec3; 3]>> {
    crate::stock_mesh::triangles_cooperative(report, bottom, cancelled, yield_control).await
}

/// Independently evaluate a point on the upper stock surface. Used for analytic
/// acceptance probes, never as a substitute for whole-region verification.
pub fn probe_height(input: &JobInput, nc: &Parsed, p: [f64; 2]) -> Result<f64> {
    let mut z = input.setup.stock.max.z;
    for s in nc::segments(nc, &input.setup)? {
        if s.rapid {
            continue;
        }
        let t = input.tools.iter().find(|t| t.id == s.tool).ok_or_else(|| Error("unknown probe tool".into()))?;
        let (d, low, _) = sweep(p, 0.0, &s, t.diameter / 2.0);
        if d <= t.diameter / 2.0 {
            let point = if t.kind == ToolKind::TwistDrill { d / (t.tip_angle / 2.0).tan() } else { 0.0 };
            z = z.min(low + point);
        }
    }
    Ok(z.max(input.setup.stock.min.z))
}

#[cfg(test)]
mod radial_tests {
    use super::*;
    #[test]
    fn rectangle_radial_extrema_include_corners_and_interior_axis() {
        assert_eq!(super::radial_bounds([3., 4.], [6., 8.], [0., 0.]), (5., 10.));
        assert_eq!(super::radial_bounds([-3., -4.], [3., 4.], [0., 0.]), (0., 5.));
        assert_eq!(super::radial_bounds([7., -2.], [10., 6.], [10., 2.]), (0., 5.));
    }

    #[test]
    fn rectangle_sweep_bounds_cover_axis_diagonal_endcaps_and_translations() {
        for offset in [0., 1_000_000.] {
            let point = |x, y| Vec3::new(x + offset, y + offset, 0.);
            for (a, b, min, max, expected) in [
                ([0., 0.], [10., 0.], [2., 3.], [4., 5.], (3., 5.)),
                ([0., 0.], [0., 10.], [3., 2.], [5., 4.], (3., 5.)),
                ([0., 0.], [0., 0.], [3., 4.], [6., 8.], (5., 10.)),
                ([0., 0.], [1., 0.], [4., 4.], [7., 8.], (5., 10.)),
                ([0., 0.], [10., 10.], [4., 4.], [6., 6.], (0., 2_f64.sqrt())),
            ] {
                let s = Segment { from: point(a[0], a[1]), to: point(b[0], b[1]), rapid: false, feed: 100., tool: 1, line: 1 };
                let (low, high) = lateral_bounds(min.map(|x| x + offset), max.map(|x| x + offset), &s);
                assert!(low <= expected.0 && high >= expected.1, "{low} {high} {expected:?}");
                assert!(expected.0 - low < 1e-6 && high - expected.1 < 1e-6);
            }
        }
    }
}
