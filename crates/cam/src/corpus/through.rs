//! Two through holes in a plate with a raised clamp between their axes.
//! Geometry and analytic expectations are independent of toolpath generation.
use crate::*;
use solidcraft_geom::Vec3;

pub(super) fn fixture() -> Result<JobInput> {
    let mut input = super::fixture("drill")?;
    input.setup.name = "Clamped through-hole plate".into();
    input.setup.fixtures = vec![BodyRef { body: "Center clamp".into(), occurrence: vec![] }];
    let mut simple = input.operations[0].clone();
    simple.name = "Simple through hole".into();
    simple.bottom = -0.25;
    simple.geometry = Strategy::Drill { holes: vec![[6., 10.]], peck: None, full_diameter: true };
    let mut peck = simple.clone();
    peck.id = 2;
    peck.name = "Peck through hole".into();
    peck.coolant = true;
    peck.geometry = Strategy::Drill { holes: vec![[14., 10.]], peck: Some(2.), full_diameter: true };
    input.operations = vec![simple, peck];
    let holes: Vec<Vec<[f64; 2]>> = [6., 14.]
        .into_iter()
        .map(|x| {
            (0..96)
                .map(|i| {
                    let a = i as f64 * std::f64::consts::TAU / 96.;
                    [x + 0.5 * a.cos(), 10. + 0.5 * a.sin()]
                })
                .collect()
        })
        .collect();
    let mut target = Vec::new();
    super::extended::flat(&mut target, &[[0., 0.], [20., 0.], [20., 20.], [0., 20.]], &holes, 6.);
    for hole in &holes {
        super::extended::wall(&mut target, hole, 6., 0.);
    }
    input.geometry.target = target;
    // Closed, outward-oriented 2 x 2 x 2 mm clamp on the plate top. A
    // clearance-height transfer passes over it; a transfer at Z7 cannot.
    let p = [[9., 9., 6.], [11., 9., 6.], [11., 11., 6.], [9., 11., 6.], [9., 9., 8.], [11., 9., 8.], [11., 11., 8.], [9., 11., 8.]].map(Vec3::from);
    input.geometry.fixtures = [[0, 3, 2, 1], [4, 5, 6, 7], [0, 1, 5, 4], [1, 2, 6, 5], [2, 3, 7, 6], [3, 0, 4, 7]]
        .into_iter()
        .flat_map(|q| [[p[q[0]], p[q[1]], p[q[2]]], [p[q[0]], p[q[2]], p[q[3]]]])
        .collect();
    input.geometry.fixture_groups = vec![12];
    input.fingerprint.clear();
    input.fingerprint = crate::fingerprint(&input)?;
    Ok(input)
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn clamped_through_holes_preserve_surfaces_and_export_open_bores() {
        let input = fixture().unwrap();
        // Each inscribed 96-gon has area 96/2 * r^2 * sin(2pi/96).
        let expected = 2400. - 6. * 24. * (std::f64::consts::TAU / 96.).sin();
        let volume: f64 = input.geometry.target.iter().map(|t| (t[1] - t[0]).cross(t[2] - t[0]).z.abs() / 2. * (t[0].z + t[1].z + t[2].z) / 3.).sum();
        assert!((volume - expected).abs() < 1e-8);
        let path = generate::generate(input.clone()).unwrap();
        let parsed = nc::round_trip(&path, &input.setup, &input.tools).unwrap();
        let report = verify::simulate(&input, &parsed, Default::default()).unwrap();
        assert_eq!((report.status, report.finished_dimensions), (verify::Status::Pass, verify::Status::Pass), "{:?}", report.findings);
        for (xy, expected) in super::super::probes("clamped-through-holes", 2).unwrap() {
            assert!((verify::probe_height(&input, &parsed, xy).unwrap() - expected).abs() < TOLERANCE);
        }
        let stock = verify::stock_stl(&report, 0.).unwrap();
        let mesh = compare::read_stl(stock.as_bytes()).unwrap();
        assert_eq!(compare::stock_mesh(&report, &mesh, 0., TOLERANCE).unwrap().status, verify::Status::Pass);
    }

    #[test]
    fn missing_breakthrough_and_low_clamp_transfer_do_not_pass() {
        let input = fixture().unwrap();
        // Authored NC: the tip reaches the bottom but the full diameter does
        // not. Both bores retain a cone of material and must fail dimensions.
        let code = "G21 G90 G17 G94 G54\nG0 X6 Y10 Z15\nT1 M6\nS6000 M3\nG1 Z0 F100\nG0 Z15\nG0 X14\nG1 Z0 F100\nG0 Z15\nM5\nM30";
        let parsed = nc::parse(code, &input.tools).unwrap();
        let report = verify::simulate(&input, &parsed, Default::default()).unwrap();
        assert_eq!(report.finished_dimensions, verify::Status::Fail);
        // An otherwise clear rapid at Z7 crosses the raised clamp between
        // axes. Require the actual source line, not merely a non-pass status.
        let code = "G21 G90 G17 G94 G54\nG0 X6 Y10 Z15\nT1 M6\nG0 Z7\nG0 X14\nG0 Z15\nM30";
        let parsed = nc::parse(code, &input.tools).unwrap();
        let report = verify::simulate(&input, &parsed, Default::default()).unwrap();
        assert_eq!(report.status, verify::Status::Fail);
        assert!(report.findings.iter().any(|f| f.kind == "fixture_cutter_collision" && f.line == Some(5)));
        let mut short = input;
        short.tools[0].cutting_length = 6.;
        assert!(generate::generate(short).is_err());
    }
}
