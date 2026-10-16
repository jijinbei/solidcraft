//! Outside contour of a square boss, with allowance roughing then finishing.
use crate::*;

pub(super) fn fixture() -> Result<JobInput> {
    let mut input = super::fixture("contour")?;
    input.setup.name = "External square boss".into();
    input.tools[0].diameter = 2.;
    input.tools[0].shank_diameter = 2.;
    input.tools[0].name = "2 mm center-cutting end mill".into();
    let square = vec![[5., 5.], [15., 5.], [15., 15.], [5., 15.]];
    let mut target = Vec::new();
    super::extended::flat(&mut target, &[[0., 0.], [20., 0.], [20., 20.], [0., 20.]], std::slice::from_ref(&square), 4.);
    super::extended::flat(&mut target, &square, &[], 6.);
    super::extended::wall(&mut target, &square, 6., 4.);
    input.geometry.target = target;
    let mut rough = input.operations[0].clone();
    rough.name = "External rough contour".into();
    rough.geometry = Strategy::Contour { boundary: Boundary { outer: square.into_iter().rev().collect(), islands: vec![] }, inside: false };
    rough.bottom = 4.;
    rough.stepdown = 0.5;
    rough.stepover = 1.;
    rough.allowance = 0.2;
    rough.lead_distance = 0.5;
    rough.ramp_angle = Some(5_f64.to_radians());
    let mut finish = rough.clone();
    finish.id = 2;
    finish.name = "External finish contour".into();
    finish.allowance = 0.;
    finish.climb = false;
    finish.coolant = true;
    input.operations = vec![rough, finish];
    input.fingerprint.clear();
    input.fingerprint = crate::fingerprint(&input)?;
    Ok(input)
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn external_contour_finishes_walls_with_leads_and_multiple_depths() {
        let input = fixture().unwrap();
        let volume: f64 = input.geometry.target.iter().map(|t| (t[1] - t[0]).cross(t[2] - t[0]).z.abs() / 2. * (t[0].z + t[1].z + t[2].z) / 3.).sum();
        assert!((volume - 1800.).abs() < 1e-9);
        let path = generate::generate(input.clone()).unwrap();
        assert!(path.blocks.iter().any(|b| matches!(b.motion, Motion::Arc { .. })));
        let parsed = nc::round_trip(&path, &input.setup, &input.tools).unwrap();
        let report = verify::simulate(&input, &parsed, Default::default()).unwrap();
        assert_eq!(
            (report.status, report.finished_dimensions),
            (verify::Status::Pass, verify::Status::Pass),
            "cells={} work={} blocks={} {:?}",
            report.cells.len(),
            report.work,
            path.blocks.len(),
            report.findings
        );
        for (xy, expected) in super::super::probes("external-contour", 2).unwrap() {
            assert!((verify::probe_height(&input, &parsed, xy).unwrap() - expected).abs() < TOLERANCE);
        }
    }

    #[test]
    fn wrong_contour_side_gouges_and_omitted_finish_leaves_allowance() {
        let input = fixture().unwrap();
        let mut wrong = input.clone();
        wrong.operations.truncate(1);
        if let Strategy::Contour { inside, .. } = &mut wrong.operations[0].geometry {
            *inside = true;
        }
        let path = generate::generate(wrong).unwrap();
        let parsed = nc::round_trip(&path, &input.setup, &input.tools).unwrap();
        let report = verify::simulate(&input, &parsed, Default::default()).unwrap();
        assert_eq!(report.status, verify::Status::Fail);
        let mut rough = input.clone();
        rough.operations.truncate(1);
        let path = generate::generate(rough).unwrap();
        let parsed = nc::round_trip(&path, &input.setup, &input.tools).unwrap();
        let report = verify::simulate(&input, &parsed, Default::default()).unwrap();
        assert_eq!(report.finished_dimensions, verify::Status::Fail, "cells={} work={} {:?}", report.cells.len(), report.work, report.findings);
    }
}
