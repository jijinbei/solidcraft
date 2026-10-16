//! Conservative artifact comparison, separate from toolpath generation.
use crate::spatial::Index;
use crate::{
    verify::{Report, Status},
    *,
};
use serde::{Deserialize, Serialize};
use solidcraft_geom::Vec3;

#[derive(Debug, Serialize, Deserialize)]
pub struct Comparison {
    pub status: Status,
    pub maximum_possible_error: f64,
    pub maximum_definite_error: f64,
    pub checked_regions: usize,
    pub detail: String,
}
fn gaps(a: [f64; 2], b: [f64; 2]) -> (f64, f64) {
    ((a[0] - b[1]).max(b[0] - a[1]).max(0.0), (a[0] - b[1]).abs().max((a[1] - b[0]).abs()))
}
fn result(detail: &str) -> Comparison {
    Comparison { status: Status::Pass, maximum_possible_error: 0., maximum_definite_error: 0., checked_regions: 0, detail: detail.into() }
}
fn inconclusive(detail: &str) -> Comparison {
    Comparison { status: Status::Inconclusive, ..result(detail) }
}
fn tolerance(value: f64) -> Result<()> {
    if !value.is_finite() || value <= 0. || value > 1e6 {
        return invalid("comparison tolerance must be positive and bounded");
    }
    Ok(())
}
/// Compare both remaining material and removed space, in both directions.
/// Each passing region is a whole-column distance certificate, not a sample.
pub fn stock_reports(a: &Report, b: &Report, tol: f64) -> Result<Comparison> {
    stock_reports_budget(a, b, tol, &mut 0, 30_000_000)
}
fn stock_reports_budget(a: &Report, b: &Report, tol: f64, work: &mut usize, budget: usize) -> Result<Comparison> {
    tolerance(tol)?;
    let (Some(stock), Some(other)) = (a.stock, b.stock) else {
        return Ok(inconclusive("missing declared stock bounds; regenerate legacy reports"));
    };
    if stock != other {
        return invalid("stock bounds differ; verify input units and coordinate frames");
    }
    let complete_a = crate::coverage::cells(&a.cells, stock)?;
    let complete_b = crate::coverage::cells(&b.cells, stock)?;
    if !complete_a || !complete_b || !a.stock_complete || !b.stock_complete || a.status != Status::Pass || b.status != Status::Pass {
        return Ok(inconclusive("stock verification or exact cell coverage is incomplete"));
    }
    let guard = 128. * f64::EPSILON * stock.min.len().max(stock.max.len()).max(1.);
    let mut r = result("Bidirectional whole-column distances for remaining material and removed space in declared setup coordinates");
    for (a, b) in [(a, b), (b, a)] {
        let index = Index::new(b.cells.iter().enumerate().map(|(id, c)| (id, c.min, c.max)).collect());
        for x in &a.cells {
            let mut ids = Vec::new();
            index.query(x.min, x.max, &mut ids);
            for id in ids {
                *work += 1;
                if *work > budget {
                    r.status = Status::Inconclusive;
                    r.detail = "stock comparison work budget exhausted".into();
                    return Ok(r);
                }
                let y = &b.cells[id];
                let min = [x.min[0].max(y.min[0]), x.min[1].max(y.min[1])];
                let max = [x.max[0].min(y.max[0]), x.max[1].min(y.max[1])];
                if (0..2).any(|k| min[k] >= max[k]) {
                    continue;
                }
                r.checked_regions += 1;
                let mut possible = gaps([x.low, x.high], [y.low, y.high]).1 + guard;
                if possible > tol {
                    let mut candidates = Vec::new();
                    index.query(min.map(|v| v - tol - guard), max.map(|v| v + tol + guard), &mut candidates);
                    let mut material = (x.high - y.low).max(0.);
                    // Outside the declared XY stock domain is guaranteed void.
                    let exterior = (max[0] - stock.min.x).min(stock.max.x - min[0]).min(max[1] - stock.min.y).min(stock.max.y - min[1]);
                    let mut removed = (y.high - x.low).max(0.).min(exterior);
                    for id in candidates {
                        *work += 1;
                        if *work > budget {
                            r.status = Status::Inconclusive;
                            r.detail = "stock distance certificate budget exhausted".into();
                            return Ok(r);
                        }
                        let c = &b.cells[id];
                        // The farthest rectangle corner bounds the distance of
                        // every XY point to this guaranteed material/void prism.
                        let xy = [0, 1].map(|k| (c.min[k] - min[k]).max(max[k] - c.max[k]).max(0.));
                        let lateral = xy[0].hypot(xy[1]);
                        material = material.min(lateral.hypot((x.high - c.low).max(0.)));
                        removed = removed.min(lateral.hypot((c.high - x.low).max(0.)));
                    }
                    possible = material.max(removed) + guard;
                    if possible > tol {
                        let center = [(min[0] + max[0]) / 2., (min[1] + max[1]) / 2.];
                        // A positive failure requires a definitely present
                        // material/void point farther than tolerance from ALL
                        // possibly corresponding prisms, not just a Z gap.
                        let radius = 2. * tol + guard;
                        let mut candidates = Vec::new();
                        index.query(center.map(|v| v - radius), center.map(|v| v + radius), &mut candidates);
                        let mut material_distance = radius;
                        let mut removed_distance = radius
                            .min(center[0] - stock.min.x)
                            .min(stock.max.x - center[0])
                            .min(center[1] - stock.min.y)
                            .min(stock.max.y - center[1]);
                        for id in candidates {
                            *work += 1;
                            if *work > budget {
                                r.status = Status::Inconclusive;
                                r.detail = "stock failure certificate budget exhausted".into();
                                return Ok(r);
                            }
                            let c = &b.cells[id];
                            let dx = (c.min[0] - center[0]).max(center[0] - c.max[0]).max(0.);
                            let dy = (c.min[1] - center[1]).max(center[1] - c.max[1]).max(0.);
                            material_distance = material_distance.min(dx.hypot(dy).hypot((x.low - c.high).max(0.)));
                            removed_distance = removed_distance.min(dx.hypot(dy).hypot((c.low - x.high).max(0.)));
                        }
                        let definite = (material_distance.max(removed_distance) - guard).max(0.);
                        r.maximum_definite_error = r.maximum_definite_error.max(definite);
                        if definite > tol {
                            r.status = Status::Fail;
                            r.detail =
                                format!("Certified local stock difference near X{} Y{}; lower distance bound {} mm", center[0], center[1], definite);
                            r.maximum_possible_error = r.maximum_possible_error.max(possible);
                            return Ok(r);
                        }
                        r.status = Status::Inconclusive;
                    }
                }
                r.maximum_possible_error = r.maximum_possible_error.max(possible);
            }
        }
    }
    Ok(r)
}

pub fn stock_mesh(report: &Report, mesh: &[[Vec3; 3]], bottom: f64, tol: f64) -> Result<Comparison> {
    stock_mesh_budget(report, mesh, bottom, tol, 250_000, 30_000_000)
}
fn stock_mesh_budget(report: &Report, mesh: &[[Vec3; 3]], bottom: f64, tol: f64, cell_budget: usize, budget: usize) -> Result<Comparison> {
    tolerance(tol)?;
    if mesh.is_empty() || mesh.len() > 500_000 || !bottom.is_finite() {
        return invalid("reference stock mesh empty or too large, or invalid bottom");
    }
    let Some(stock) = report.stock else {
        return Ok(inconclusive("missing declared stock bounds; regenerate legacy report"));
    };
    if bottom != stock.min.z {
        return invalid("reference stock bottom differs from declared stock");
    }
    let field = crate::coverage::MeshField::new(mesh, stock)?;
    let mut cells = Vec::new();
    if !crate::coverage::cells(&report.cells, stock)? || !report.stock_complete || report.status != Status::Pass {
        return Ok(inconclusive("stock verification or exact cell coverage is incomplete"));
    }
    // Reuse only the spatial partition, never the report's heights as mesh
    // evidence. Every reference interval is calculated from its own triangles.
    let mut pending: Vec<_> = report.cells.iter().map(|c| (c.min, c.max, [c.low, c.high])).collect();
    let mut work = 0;
    while let Some((min, max, source)) = pending.pop() {
        if cells.len() + pending.len() >= cell_budget {
            return Ok(inconclusive("reference stock refinement cell budget exhausted"));
        }
        let Some([low, high]) = field.bounds(min, max, &mut work, budget) else {
            return Ok(inconclusive("reference stock refinement work budget exhausted"));
        };
        let size = (max[0] - min[0]).hypot(max[1] - min[1]);
        if gaps(source, [low, high]).1 > tol && high - low > VERIFY_ERROR.min(tol / 4.) && size > VERIFY_ERROR.min(tol / 4.) {
            let center = [(min[0] + max[0]) / 2., (min[1] + max[1]) / 2.];
            if (0..2).any(|k| center[k] <= min[k] || center[k] >= max[k]) {
                return Ok(inconclusive("reference refinement reached coordinate precision limit"));
            }
            for (a, b) in [(min, center), ([center[0], min[1]], [max[0], center[1]]), (center, max), ([min[0], center[1]], [center[0], max[1]])] {
                pending.push((a, b, source));
            }
        } else {
            cells.push(verify::Cell { min, max, low, high, target_low: low, target_high: high });
        }
    }
    let reference = Report { cells, status: Status::Pass, stock_complete: true, ..report.clone() };
    stock_reports_budget(report, &reference, tol, &mut work, budget)
}
/// Strict STL decoding: malformed triangles are rejected, never silently dropped.
pub fn read_stl(bytes: &[u8]) -> Result<Vec<[Vec3; 3]>> {
    if bytes.len() > 128 * 1024 * 1024 {
        return invalid("STL exceeds 128 MiB");
    }
    let mut triangles = Vec::new();
    let count = bytes.get(80..84).and_then(|v| <[u8; 4]>::try_from(v).ok()).map(u32::from_le_bytes).map(|n| n as usize);
    if let Some(count) = count.filter(|n| 84usize.checked_add(n.saturating_mul(50)) == Some(bytes.len())) {
        if count > 500_000 {
            return invalid("STL triangle limit");
        }
        for chunk in bytes.get(84..).unwrap_or_default().as_chunks::<50>().0 {
            let mut pts = [Vec3::ZERO; 3];
            for (k, p) in pts.iter_mut().enumerate() {
                let mut xyz = [0.0; 3];
                for (j, v) in xyz.iter_mut().enumerate() {
                    let start = 12 + k * 12 + j * 4;
                    let b =
                        chunk.get(start..start + 4).and_then(|b| <[u8; 4]>::try_from(b).ok()).ok_or_else(|| Error("truncated STL vertex".into()))?;
                    *v = f32::from_le_bytes(b) as f64;
                }
                *p = Vec3::new(xyz[0], xyz[1], xyz[2]);
                crate::model::point(*p)?;
            }
            triangles.push(pts);
        }
    } else {
        let text = std::str::from_utf8(bytes).map_err(|_| Error("invalid STL encoding/length".into()))?;
        let mut lines = text.lines().map(|line| line.split_whitespace().collect::<Vec<_>>()).filter(|line| !line.is_empty());
        if lines.next().and_then(|line| line.first().copied()) != Some("solid") {
            return invalid("invalid ASCII STL header");
        }
        loop {
            let header = lines.next().ok_or_else(|| Error("missing STL terminator".into()))?;
            if header.first() == Some(&"endsolid") {
                if lines.next().is_some() {
                    return invalid("data after STL terminator");
                }
                break;
            }
            if header.len() != 5 || header[..2] != ["facet", "normal"] || header[2..].iter().any(|v| !v.parse::<f64>().is_ok_and(f64::is_finite)) {
                return invalid("invalid STL facet header");
            }
            if lines.next().as_deref() != Some(&["outer", "loop"]) {
                return invalid("missing STL outer loop");
            }
            let mut points = [Vec3::ZERO; 3];
            for point in &mut points {
                let words = lines.next().ok_or_else(|| Error("incomplete STL triangle".into()))?;
                if words.len() != 4 || words[0] != "vertex" {
                    return invalid("invalid STL vertex");
                }
                let mut xyz = [0.0; 3];
                for (i, word) in words[1..].iter().enumerate() {
                    xyz[i] = word.parse::<f64>().map_err(|_| Error("invalid STL number".into()))?;
                }
                *point = Vec3::new(xyz[0], xyz[1], xyz[2]);
                crate::model::point(*point)?;
            }
            if lines.next().as_deref() != Some(&["endloop"]) || lines.next().as_deref() != Some(&["endfacet"]) {
                return invalid("incomplete STL facet");
            }
            triangles.push(points);
            if triangles.len() > 500_000 {
                return invalid("STL triangle limit");
            }
        }
    }
    if triangles.is_empty() {
        return invalid("empty STL");
    }
    Ok(triangles)
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    fn cell(x0: f64, x1: f64, low: f64, high: f64) -> verify::Cell {
        verify::Cell { min: [x0, 0.], max: [x1, 2.], low, high, target_low: low, target_high: high }
    }
    fn report(cells: Vec<verify::Cell>) -> Report {
        Report {
            version: VERSION.into(),
            fingerprint: "analytic".into(),
            status: Status::Pass,
            finished_dimensions: Status::Pass,
            post_version: "analytic".into(),
            tolerance: TOLERANCE,
            uncertainty: VERIFY_ERROR,
            maximum_height_interval: 0.,
            stock_complete: true,
            stock: Some(Stock { min: Vec3::ZERO, max: Vec3::new(2., 2., 3.) }),
            cells,
            findings: vec![],
            cutting_distance: 0.,
            rapid_distance: 0.,
            cutting_minutes: 0.,
            remaining_volume_low: 0.,
            remaining_volume_high: 0.,
            work: 0,
        }
    }
    fn mesh(report: &Report) -> Vec<[Vec3; 3]> {
        read_stl(verify::stock_stl(report, 0.).unwrap().as_bytes()).unwrap()
    }
    #[test]
    fn duplicate_open_and_misoriented_reference_triangles_cannot_pass() {
        let r = report(vec![cell(0., 2., 1., 1.)]);
        let tri = [Vec3::new(0., 0., 1.), Vec3::new(2., 0., 1.), Vec3::new(2., 2., 1.)];
        assert!(stock_mesh(&r, &[tri, tri], 0., TOLERANCE).is_err());
        let mut m = mesh(&r);
        m.pop();
        assert!(stock_mesh(&r, &m, 0., TOLERANCE).is_err());
        let mut m = mesh(&r);
        for t in &mut m {
            t.swap(0, 1);
        }
        assert!(stock_mesh(&r, &m, 0., TOLERANCE).is_err());
    }
    #[test]
    fn cell_coverage_has_no_area_epsilon_and_rejects_duplicates() {
        let full = report(vec![cell(0., 2., 1., 1.)]);
        let duplicate = report(vec![cell(0., 1., 1., 1.), cell(0., 1., 1., 1.)]);
        assert!(stock_reports(&full, &duplicate, TOLERANCE).is_err());
        let gap = report(vec![cell(0., 1., 1., 1.), cell(1. + 1e-10, 2., 1., 1.)]);
        assert_eq!(stock_reports(&full, &gap, TOLERANCE).unwrap().status, Status::Inconclusive);
        let mut legacy = full.clone();
        legacy.stock = None;
        assert_eq!(stock_reports(&full, &legacy, TOLERANCE).unwrap().status, Status::Inconclusive);
        let mut shifted = full.clone();
        shifted.stock.as_mut().unwrap().min.x = -1.;
        assert!(stock_reports(&full, &shifted, TOLERANCE).is_err());
        let mut bad = full.clone();
        bad.cells[0].high = f64::NAN;
        assert!(stock_reports(&full, &bad, TOLERANCE).is_err());
        let mut bad = full.clone();
        bad.cells[0].min[0] = 1e-200;
        assert!(stock_reports(&full, &bad, TOLERANCE).is_err());
    }
    #[test]
    fn whole_column_certificates_resolve_wall_uncertainty_and_detect_wall_shift() {
        let exact = report(vec![cell(0., 1., 1., 1.), cell(1., 2., 2., 2.)]);
        let band = report(vec![cell(0., 0.999, 1., 1.), cell(0.999, 1.001, 1., 2.), cell(1.001, 2., 2., 2.)]);
        let r = stock_reports(&exact, &band, TOLERANCE).unwrap();
        assert_eq!(r.status, Status::Pass, "{r:?}");
        assert!(r.maximum_possible_error < 0.003);
        let shifted = report(vec![cell(0., 1.1, 1., 1.), cell(1.1, 2., 2., 2.)]);
        assert_eq!(stock_reports(&exact, &shifted, TOLERANCE).unwrap().status, Status::Fail);
        let wide = report(vec![cell(0., 2., 1., 2.)]);
        assert_eq!(stock_reports(&wide, &wide, TOLERANCE).unwrap().status, Status::Inconclusive);
        assert_eq!(stock_reports_budget(&exact, &exact, TOLERANCE, &mut 0, 1).unwrap().status, Status::Inconclusive);
    }
    #[test]
    fn removed_space_detects_a_narrow_deep_missing_hole() {
        let full = report(vec![cell(0., 2., 2., 2.)]);
        // A solid-only Hausdorff test could hide this narrow cut because all
        // missing material is within tolerance of a neighboring solid column.
        let hole = report(vec![cell(0., 0.999, 2., 2.), cell(0.999, 1.001, 0., 0.), cell(1.001, 2., 2., 2.)]);
        assert_eq!(stock_reports(&full, &hole, TOLERANCE).unwrap().status, Status::Fail);
        let rim = report(vec![cell(0., 0.001, 0., 0.), cell(0.001, 2., 2., 2.)]);
        assert_eq!(stock_reports(&full, &rim, TOLERANCE).unwrap().status, Status::Pass);
        let rim = report(vec![cell(0., 0.1, 0., 0.), cell(0.1, 2., 2., 2.)]);
        assert_eq!(stock_reports(&full, &rim, TOLERANCE).unwrap().status, Status::Fail);
    }
    #[test]
    fn closed_stock_mesh_compares_at_vertical_walls_and_holes() {
        let step = report(vec![cell(0., 1., 1., 1.), cell(1., 2., 2., 2.)]);
        let m = mesh(&step);
        let comparison = stock_mesh(&step, &m, 0., TOLERANCE).unwrap();
        assert_eq!(comparison.status, Status::Pass, "{comparison:?}");
        let hole = report(vec![cell(0., 0.9, 2., 2.), cell(0.9, 1.1, 0., 0.), cell(1.1, 2., 2., 2.)]);
        let comparison = stock_mesh(&hole, &mesh(&hole), 0., TOLERANCE).unwrap();
        assert_eq!(comparison.status, Status::Pass, "{comparison:?}");
        assert_eq!(stock_mesh_budget(&step, &m, 0., TOLERANCE, 1, 100).unwrap().status, Status::Inconclusive);
    }
    #[test]
    fn shared_mesh_edges_do_not_refine_flat_cells_or_hide_thin_overlaps() {
        let step = report(vec![cell(0., 1., 1., 1.), cell(1., 2., 2., 2.)]);
        let m = mesh(&step);
        assert_eq!(stock_mesh_budget(&step, &m, 0., TOLERANCE, 3, 100_000).unwrap().status, Status::Pass);
        let field = crate::coverage::MeshField::new(&m, step.stock.unwrap()).unwrap();
        let flat = field.bounds([0., 0.], [1., 2.], &mut 0, 1000).unwrap();
        assert!((flat[0] - 1.).abs() < 1e-10 && (flat[1] - 1.).abs() < 1e-10);
        // Positive-area overlap has no area epsilon: even this tiny strip must
        // retain both heights, rather than treating it as a shared edge.
        let sliver = field.bounds([0., 0.], [1. + 1e-10, 2.], &mut 0, 1000).unwrap();
        assert!(sliver[0] <= 1. && sliver[1] >= 2.);
    }
    #[test]
    fn overlapping_closed_components_cannot_counterfeit_full_coverage() {
        let expected = report(vec![cell(0., 2., 1., 1.)]);
        let mut m = mesh(&report(vec![cell(0., 1., 1., 1.)]));
        m.extend(mesh(&report(vec![cell(0.1, 1.1, 1., 1.)])));
        let comparison = stock_mesh(&expected, &m, 0., TOLERANCE).unwrap();
        assert_eq!(comparison.status, Status::Fail, "{comparison:?}");
        let mut extra = mesh(&expected);
        for t in &mut extra {
            for p in t {
                p.x += 1.;
            }
        }
        assert!(stock_mesh(&expected, &extra, 0., TOLERANCE).is_err());
    }
    #[test]
    fn generated_drill_stock_round_trip_is_spatially_certified() {
        let input = corpus::fixture("drill").unwrap();
        let path = generate::generate(input.clone()).unwrap();
        let parsed = nc::round_trip(&path, &input.setup, &input.tools).unwrap();
        let r = verify::simulate(&input, &parsed, Default::default()).unwrap();
        let m = mesh(&r);
        let comparison = stock_mesh(&r, &m, 0., TOLERANCE).unwrap();
        assert_eq!(comparison.status, Status::Pass, "{comparison:?}");
    }
    #[test]
    fn analytic_round_hole_matches_independently_faceted_closed_stock() {
        // Original analytic solid: 2x2 plate, top Z2, radius .25 bore, a
        // cone from tip Z1 to Z1.5, and a vertical wall to the plate top.
        // Intervals below use only radial extrema; no toolpath or STL exporter.
        let height = |d: f64| if d < 0.25 { 1. + 2. * d } else { 2. };
        let mut cells = Vec::new();
        let mut pending = vec![([0., 0.], [2., 2.])];
        while let Some((min, max)) = pending.pop() {
            let near: [f64; 2] = [0, 1].map(|k| (min[k] - 1_f64).max(1. - max[k]).max(0.));
            let far: [f64; 2] = [0, 1].map(|k| (min[k] - 1_f64).abs().max((max[k] - 1.).abs()));
            let low = height(near[0].hypot(near[1]));
            let high = height(far[0].hypot(far[1]));
            if high - low > 0.002 && (max[0] - min[0]).hypot(max[1] - min[1]) > VERIFY_ERROR {
                let c = [(min[0] + max[0]) / 2., (min[1] + max[1]) / 2.];
                pending.extend([(min, c), ([c[0], min[1]], [max[0], c[1]]), (c, max), ([min[0], c[1]], [c[0], max[1]])]);
            } else {
                cells.push(verify::Cell { min, max, low, high, target_low: low, target_high: high });
            }
        }
        let r = report(cells);
        let mut m = Vec::new();
        let ring = |i: usize| {
            let a = (i % 64) as f64 * std::f64::consts::TAU / 64.;
            let (y, x) = a.sin_cos();
            let outer =
                if x.abs() >= y.abs() { Vec3::new(1. + x.signum(), 1. + y / x.abs(), 2.) } else { Vec3::new(1. + x / y.abs(), 1. + y.signum(), 2.) };
            [Vec3::new(1. + 0.25 * x, 1. + 0.25 * y, 2.), Vec3::new(1. + 0.25 * x, 1. + 0.25 * y, 1.5), outer, Vec3::new(outer.x, outer.y, 0.)]
        };
        for i in 0..64 {
            let [a, lo, outer, base] = ring(i);
            let [b, lo_b, outer_b, base_b] = ring(i + 1);
            m.extend([
                [a, outer, outer_b],
                [a, outer_b, b],
                [a, b, lo_b],
                [a, lo_b, lo],
                [Vec3::new(1., 1., 1.), lo, lo_b],
                [outer, base, base_b],
                [outer, base_b, outer_b],
                [Vec3::new(1., 1., 0.), base_b, base],
            ]);
        }
        let comparison = stock_mesh(&r, &m, 0., TOLERANCE).unwrap();
        assert_eq!(comparison.status, Status::Pass, "{comparison:?}");
        // Different facet decomposition must preserve the result.
        let split: Vec<_> = m
            .iter()
            .flat_map(|t| {
                if (t[1] - t[0]).cross(t[2] - t[0]).z == 0. {
                    return vec![*t];
                }
                let c = (t[0] + t[1] + t[2]) / 3.;
                vec![[t[0], t[1], c], [t[1], t[2], c], [t[2], t[0], c]]
            })
            .collect();
        assert_eq!(stock_mesh(&r, &split, 0., TOLERANCE).unwrap().status, Status::Pass);
        let missed = report(vec![cell(0., 2., 2., 2.)]);
        assert_eq!(stock_mesh(&missed, &m, 0., TOLERANCE).unwrap().status, Status::Fail);
    }
    #[test]
    fn invalid_stl_is_not_partially_accepted() {
        assert!(read_stl(b"solid x\nvertex NaN 0 0\nvertex 1 0 0\nvertex 0 1 0\nendsolid").is_err());
        assert!(read_stl(&[0; 84]).is_err());
    }
    #[test]
    fn interval_difference_catches_local_gouge() {
        assert!((gaps([4.0, 4.001], [5.0, 5.001]).0 - 0.999).abs() < 1e-10);
        assert!(gaps([4.99, 5.01], [5.0, 5.0]).1 > 0.009);
    }
    #[test]
    fn incomplete_coverage_is_symmetric_and_stl_is_bounded() {
        let input = corpus::fixture("face").unwrap();
        let path = generate::generate(input.clone()).unwrap();
        let parsed = nc::round_trip(&path, &input.setup, &input.tools).unwrap();
        let full = verify::simulate(&input, &parsed, Default::default()).unwrap();
        let mut partial = full.clone();
        partial.cells.truncate(1);
        assert_eq!(stock_reports(&partial, &full, TOLERANCE).unwrap().status, Status::Inconclusive);
        assert_eq!(stock_reports(&full, &partial, TOLERANCE).unwrap().status, Status::Inconclusive);
        let stl = verify::stock_stl(&full, 0.0).unwrap();
        let mesh = read_stl(stl.as_bytes()).unwrap();
        assert_eq!(mesh.len(), 12); // exact coplanar merge, conforming boundary
        let volume: f64 = mesh.iter().map(|t| t[0].dot(t[1].cross(t[2])) / 6.0).sum();
        assert!((volume - 2000.0).abs() < 1e-9);
        assert!(read_stl(stl.replace("endfacet", "unknown").as_bytes()).is_err());
    }
}
