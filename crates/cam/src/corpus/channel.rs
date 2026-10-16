//! Original narrow-channel recipes with independently triangulated protected stock.
use crate::*;

fn capsule() -> Boundary {
    let mut outer = Vec::new();
    for (x, start) in [(5., 90_f64), (15., 270_f64)] {
        for i in 0..=64 {
            let a = (start + 180. * i as f64 / 64.).to_radians();
            outer.push([x + 1.1 * a.cos(), 10. + 1.1 * a.sin()]);
        }
    }
    Boundary { outer, islands: vec![] }
}
fn with_boundary(boundary: Boundary) -> Result<JobInput> {
    let mut input = super::fixture("pocket")?;
    input.setup.name = "2.2 mm capsule channel".into();
    input.tools[0].name = "2 mm center-cutting end mill".into();
    input.tools[0].diameter = 2.;
    input.tools[0].shank_diameter = 2.;
    input.operations[0].name = "Finish narrow channel".into();
    input.operations[0].stepover = 0.75;
    input.operations[0].geometry = Strategy::Pocket { boundary: boundary.clone() };
    input.geometry.target.clear();
    super::extended::flat(&mut input.geometry.target, &[[0., 0.], [20., 0.], [20., 20.], [0., 20.]], std::slice::from_ref(&boundary.outer), 6.);
    super::extended::flat(&mut input.geometry.target, &boundary.outer, &[], 5.);
    super::extended::wall(&mut input.geometry.target, &boundary.outer, 6., 5.);
    input.fingerprint.clear();
    input.fingerprint = crate::fingerprint(&input)?;
    Ok(input)
}
pub(super) fn fixture() -> Result<JobInput> {
    with_boundary(capsule())
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::verify::Status;

    fn program(input: &JobInput) -> nc::Parsed {
        let path = generate::generate(input.clone()).unwrap();
        nc::round_trip(&path, &input.setup, &input.tools).unwrap()
    }

    #[test]
    fn narrow_channel_finishes_protected_walls_and_floor() {
        let input = fixture().unwrap();
        // A rectangle plus two polygonal semicircles, each with 64 segments.
        let area = 22. + 64. * 1.1_f64.powi(2) * (std::f64::consts::PI / 64.).sin();
        let volume: f64 =
            input.geometry.target.iter().map(|t| ((t[1] - t[0]).cross(t[2] - t[0]).z / 2.).abs() * (t[0].z + t[1].z + t[2].z) / 3.).sum();
        assert!((volume - (2400. - area)).abs() < 1e-8);
        let parsed = program(&input);
        let report = verify::simulate(&input, &parsed, Default::default()).unwrap();
        assert_eq!((report.status, report.finished_dimensions), (Status::Pass, Status::Pass), "{:?}", report.findings);
        for (xy, expected) in super::super::probes("narrow-channel", 1).unwrap() {
            assert!((verify::probe_height(&input, &parsed, xy).unwrap() - expected).abs() < TOLERANCE);
        }
    }

    #[test]
    fn oversized_tool_and_inaccessible_neck_cannot_succeed() {
        let mut input = fixture().unwrap();
        input.tools[0].diameter = 2.4;
        input.tools[0].shank_diameter = 2.4;
        assert!(generate::generate(input).unwrap_err().0.contains("tool cannot fit pocket"));

        // R2 lobes linked by a 1.6 mm wide, >6 mm long neck. A 2 mm tool
        // can enter both lobes but cannot reach the center of the neck.
        let angle = 0.4_f64.asin();
        let mut outer = Vec::new();
        for (x, start) in [(5., angle), (15., std::f64::consts::PI + angle)] {
            for i in 0..=128 {
                let a = start + (std::f64::consts::TAU - 2. * angle) * i as f64 / 128.;
                outer.push([x + 2. * a.cos(), 10. + 2. * a.sin()]);
            }
        }
        let input = with_boundary(Boundary { outer, islands: vec![] }).unwrap();
        let parsed = program(&input);
        assert_eq!(verify::probe_height(&input, &parsed, [10., 10.]).unwrap(), 6.);
        let report = verify::simulate(&input, &parsed, Default::default()).unwrap();
        assert_eq!(report.finished_dimensions, Status::Fail, "{:?}", report.findings);
        assert!(report.findings.iter().any(|f| f.kind == "finished_region_error"));
    }

    #[test]
    fn unsafe_lead_is_rejected_against_original_protected_geometry() {
        let mut input = fixture().unwrap();
        input.operations[0].lead_distance = 5.;
        input.fingerprint.clear();
        input.fingerprint = crate::fingerprint(&input).unwrap();
        let parsed = program(&input);
        let report = verify::simulate(&input, &parsed, Default::default()).unwrap();
        assert_eq!(report.status, Status::Fail, "{:?}", report.findings);
        assert!(report.findings.iter().any(|f| f.severity == verify::FindingSeverity::Error && f.source.is_some()));
    }
}
