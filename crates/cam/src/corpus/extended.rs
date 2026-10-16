//! Original dimensioned recipes. Analytic surfaces are triangulated independently
//! of the CAM offset/generation algorithms; no proprietary artifacts are used.
use crate::*;
use solidcraft_geom::{Loop2, Region2, Seg2, Vec2, Vec3};

fn circle(center: [f64; 2], radius: f64) -> Vec<[f64; 2]> {
    (0..96)
        .map(|i| {
            let a = i as f64 * std::f64::consts::TAU / 96.;
            [center[0] + radius * a.cos(), center[1] + radius * a.sin()]
        })
        .collect()
}

// An L with R2 convex corners and an R2 reentrant corner. The sampled polygon
// itself is the recipe, so there is no unstated curve approximation in its input.
fn boundary() -> Boundary {
    let mut outer = Vec::new();
    for (center, start, sweep) in
        [([4., 4.], 180., 90.), ([16., 4.], 270., 90.), ([16., 8.], 0., 90.), ([12., 12.], 270., -90.), ([8., 16.], 0., 90.), ([4., 16.], 90., 90.)]
    {
        for i in 0..=32 {
            let a = (start + sweep * i as f64 / 32.) * std::f64::consts::PI / 180.;
            outer.push([center[0] + 2. * a.cos(), center[1] + 2. * a.sin()]);
        }
    }
    Boundary { outer, islands: vec![circle([6., 6.], 0.8), circle([6., 13.], 0.8)] }
}
fn ring(points: &[[f64; 2]]) -> Loop2 {
    Loop2 {
        segs: points
            .iter()
            .zip(points.iter().cycle().skip(1))
            .take(points.len())
            .map(|(a, b)| Seg2::Line { a: Vec2::new(a[0], a[1]), b: Vec2::new(b[0], b[1]) })
            .collect(),
    }
}
pub(super) fn flat(out: &mut Vec<[Vec3; 3]>, outer: &[[f64; 2]], holes: &[Vec<[f64; 2]>], z: f64) {
    let region = Region2 { outer: ring(outer), holes: holes.iter().map(|h| ring(h)).collect() };
    for t in region.triangulate(1e-6) {
        if (t[1] - t[0]).cross(t[2] - t[0]).abs() > 1e-14 {
            out.push(t.map(|p| Vec3::new(p.x, p.y, z)));
        }
    }
}
pub(super) fn wall(out: &mut Vec<[Vec3; 3]>, points: &[[f64; 2]], top: f64, bottom: f64) {
    for (a, b) in points.iter().zip(points.iter().cycle().skip(1)).take(points.len()) {
        let (a, b) = (Vec3::new(a[0], a[1], top), Vec3::new(b[0], b[1], top));
        let (c, d) = (Vec3::new(a.x, a.y, bottom), Vec3::new(b.x, b.y, bottom));
        out.extend([[a, b, d], [a, d, c]]);
    }
}
fn target(boundary: &Boundary, top: f64, drilled: bool) -> Vec<[Vec3; 3]> {
    let mut out = Vec::new();
    flat(&mut out, &[[0., 0.], [20., 0.], [20., 20.], [0., 20.]], std::slice::from_ref(&boundary.outer), top);
    let mut holes = boundary.islands.clone();
    let drill = circle([15., 6.], 0.5);
    if drilled {
        holes.push(drill.clone());
    }
    flat(&mut out, &boundary.outer, &holes, 4.);
    wall(&mut out, &boundary.outer, top, 4.);
    for island in &boundary.islands {
        flat(&mut out, island, &[], top);
        wall(&mut out, island, top, 4.);
    }
    if drilled {
        let shoulder = 3. + 0.5 / 59_f64.to_radians().tan();
        wall(&mut out, &drill, 4., shoulder);
        for (a, b) in drill.iter().zip(drill.iter().cycle().skip(1)).take(drill.len()) {
            out.push([Vec3::new(15., 6., 3.), Vec3::new(a[0], a[1], shoulder), Vec3::new(b[0], b[1], shoulder)]);
        }
    }
    out
}

pub(super) fn fixture(name: &str) -> Result<JobInput> {
    let mut input = super::fixture("pocket")?;
    let mut boundary = boundary();
    input.setup.name = "Rounded L with two islands".into();
    input.tools[0].diameter = 2.;
    input.tools[0].shank_diameter = 2.;
    input.tools[0].name = "2 mm center-cutting end mill".into();
    let mut pocket = input.operations[0].clone();
    pocket.name = "Concave pocket and islands".into();
    pocket.bottom = 4.;
    pocket.stepdown = 0.5;
    pocket.stepover = 0.75;
    // Exercise winding normalization independently for outer and island loops.
    boundary.outer.reverse();
    if let Some(first) = boundary.islands.first_mut() {
        first.reverse();
    }
    pocket.geometry = Strategy::Pocket { boundary: boundary.clone() };
    input.geometry.target = target(&boundary, if name == "multi-operation" { 5.5 } else { 6. }, name == "multi-operation");
    input.operations = vec![pocket.clone()];
    if name == "multi-operation" {
        let mut face = super::fixture("face")?;
        face.tools[0].id = 2;
        face.tools[0].number = 2;
        face.operations[0].id = 1;
        face.operations[0].tool = 2;
        face.operations[0].bottom = 5.5;
        pocket.id = 2;
        pocket.top = 5.5;
        pocket.allowance = 0.2;
        pocket.coolant = true;
        let mut finish = pocket.clone();
        finish.id = 3;
        finish.name = "Finish pocket".into();
        finish.allowance = 0.;
        finish.climb = false;
        let mut drill = super::fixture("drill")?;
        drill.tools[0].id = 3;
        drill.tools[0].number = 3;
        drill.operations[0].id = 4;
        drill.operations[0].tool = 3;
        drill.operations[0].top = 4.;
        drill.operations[0].bottom = 3.;
        drill.operations[0].geometry = Strategy::Drill { holes: vec![[15., 6.]], peck: Some(0.3), full_diameter: false };
        input.tools.extend([face.tools.remove(0), drill.tools.remove(0)]);
        input.operations = vec![face.operations.remove(0), pocket, finish, drill.operations.remove(0)];
        input.setup.frame.origin = Vec3::new(35., -20., 10.);
        input.setup.work_offset = 55;
    }
    input.fingerprint.clear();
    input.fingerprint = crate::fingerprint(&input)?;
    Ok(input)
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    #[test]
    fn protected_target_volume_matches_the_original_recipe() {
        // Polygonal R2 quarter arcs contain 32 equal angle segments; islands
        // and the drill use 96. These areas come from triangle sector formulae.
        let pocket_area = 176. + 256. * (std::f64::consts::PI / 64.).sin() - 96. * 0.8_f64.powi(2) * (std::f64::consts::TAU / 96.).sin();
        for name in ["concave-islands", "multi-operation"] {
            let input = fixture(name).unwrap();
            let volume: f64 =
                input.geometry.target.iter().map(|t| ((t[1] - t[0]).cross(t[2] - t[0]).z / 2.).abs() * (t[0].z + t[1].z + t[2].z) / 3.).sum();
            let expected = if name == "concave-islands" {
                2400. - 2. * pocket_area
            } else {
                let hole_area = 12. * (std::f64::consts::TAU / 96.).sin();
                2200. - 1.5 * pocket_area - hole_area * (1. - 1. / (3. * 59_f64.to_radians().tan()))
            };
            assert!((volume - expected).abs() < 1e-8, "{name}: {volume} != {expected}");
        }
    }
    #[test]
    fn extended_fixtures_certify_complete_protected_surfaces() {
        for name in ["concave-islands", "multi-operation"] {
            let input = fixture(name).unwrap();
            let path = generate::generate(input.clone()).unwrap();
            let parsed = nc::round_trip(&path, &input.setup, &input.tools).unwrap();
            let report = verify::simulate(&input, &parsed, Default::default()).unwrap();
            assert_eq!(
                (report.status, report.finished_dimensions),
                (verify::Status::Pass, verify::Status::Pass),
                "{name} {} cells {} work {:?}",
                report.cells.len(),
                report.work,
                report.findings.iter().take(4).collect::<Vec<_>>()
            );
            for (xy, expected) in super::super::probes(name, input.operations.len()).unwrap() {
                assert!((verify::probe_height(&input, &parsed, xy).unwrap() - expected).abs() < TOLERANCE, "{name} {xy:?}");
            }
            // Adaptive drill-tip cells must form a conforming stock surface,
            // including where their refinement meets the pocket floor.
            verify::stock_triangles(&report, input.setup.stock.min.z).unwrap();
        }
    }
    #[test]
    fn intermediate_roughing_is_allowed_but_missing_finishing_is_not() {
        let mut input = fixture("multi-operation").unwrap();
        input.operations.truncate(2);
        let path = generate::generate(input.clone()).unwrap();
        let parsed = nc::round_trip(&path, &input.setup, &input.tools).unwrap();
        let rough = verify::simulate(&input, &parsed, Default::default()).unwrap();
        assert_eq!(
            (rough.status, rough.finished_dimensions),
            (verify::Status::Pass, verify::Status::Pass),
            "{:?}",
            rough.findings.iter().take(5).collect::<Vec<_>>()
        );
        assert!((verify::probe_height(&input, &parsed, [6., 9.]).unwrap() - 4.2).abs() < 1e-8);
        let mut finished = fixture("multi-operation").unwrap();
        finished.operations.truncate(3);
        let missing = verify::simulate(&finished, &parsed, Default::default()).unwrap();
        assert_eq!(missing.finished_dimensions, verify::Status::Fail);
    }
    #[test]
    fn removed_islands_wrong_tool_and_wrong_work_offset_are_rejected() {
        let input = fixture("concave-islands").unwrap();
        let mut damaged = input.clone();
        if let Strategy::Pocket { boundary } = &mut damaged.operations[0].geometry {
            boundary.islands.clear();
        }
        let path = generate::generate(damaged).unwrap();
        let parsed = nc::round_trip(&path, &input.setup, &input.tools).unwrap();
        let result = verify::simulate(&input, &parsed, Default::default()).unwrap();
        assert_eq!(result.status, verify::Status::Fail);
        assert_eq!(result.finished_dimensions, verify::Status::Fail);
        let input = fixture("multi-operation").unwrap();
        let path = generate::generate(input.clone()).unwrap();
        let tools: Vec<_> = path.blocks.iter().filter_map(|b| if let Motion::Tool { number, .. } = b.motion { Some(number) } else { None }).collect();
        assert_eq!(tools, vec![2, 1, 1, 3]);
        let code = nc::post(&path, &input.setup).unwrap();
        let wrong = nc::parse(&code.replace("T3 M6", "T1 M6"), &input.tools).unwrap();
        let report = verify::simulate(&input, &wrong, Default::default()).unwrap();
        assert_eq!(report.status, verify::Status::Fail);
        assert_eq!(report.finished_dimensions, verify::Status::Fail);
        let wrong = nc::parse(&code.replace("G55", "G54"), &input.tools).unwrap();
        assert!(verify::simulate(&input, &wrong, Default::default()).is_err());
    }
}
