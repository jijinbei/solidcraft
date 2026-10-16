//! Independent analytic finished-height envelopes. No path-generation or offset
//! code is used here. Radial bands limit diameter error; depth is checked in Z.
use crate::{verify::Status, *};

#[derive(Clone, Copy, Debug)]
pub(crate) struct Envelope {
    lower_min: f64,
    lower_max: f64,
    upper_min: f64,
    upper_max: f64,
}
impl Envelope {
    fn range(lo: f64, hi: f64, tolerance: f64) -> Self {
        Self { lower_min: lo - tolerance, lower_max: hi - tolerance, upper_min: lo + tolerance, upper_max: hi + tolerance }
    }
    fn alternatives(self, b: Self) -> Self {
        Self {
            lower_min: self.lower_min.min(b.lower_min),
            lower_max: self.lower_max.max(b.lower_max),
            upper_min: self.upper_min.min(b.upper_min),
            upper_max: self.upper_max.max(b.upper_max),
        }
    }
    fn cut(self, b: Self) -> Self {
        Self {
            lower_min: self.lower_min.min(b.lower_min),
            lower_max: self.lower_max.min(b.lower_max),
            upper_min: self.upper_min.min(b.upper_min),
            upper_max: self.upper_max.min(b.upper_max),
        }
    }
    pub(crate) fn classify(self, low: f64, high: f64) -> Status {
        if high < self.lower_min || low > self.upper_max {
            Status::Fail
        } else if low >= self.lower_max && high <= self.upper_min {
            Status::Pass
        } else {
            Status::Inconclusive
        }
    }
    pub(crate) fn description(self) -> String {
        format!(
            "Finished-region height must lie within [{:.6}, {:.6}] mm; cell-wide certified range [{:.6}, {:.6}] mm",
            self.lower_min, self.upper_max, self.lower_max, self.upper_min
        )
    }
}

enum Feature {
    Plane(f64),
    Hole { center: [f64; 2], radius: f64, tip: f64, slope: f64 },
    Mill { region: crate::region::Region, outside: bool, width: Option<f64>, allowance: f64, depth: f64 },
}
pub(crate) struct Finished {
    features: Vec<Feature>,
    bottom: f64,
    top: f64,
    tolerance: f64,
    radial_tolerance: f64,
    pub(crate) refinement: f64,
}
impl Finished {
    pub(crate) fn new(input: &JobInput, tolerance: f64) -> Result<Option<Self>> {
        // Reserve radial error on both sides of a diameter, and convert XY
        // interpolation error into a conservative cone-height error as well.
        let has_walls = input.operations.iter().any(|o| !matches!(o.geometry, Strategy::Face));
        let profile_error = input.operations.iter().filter(|o| o.source.is_some()).map(|o| crate::profile_tolerance(o.tolerance)).fold(0.0, f64::max);
        let radial_tolerance = tolerance / 2.0 - POST_ERROR - profile_error;
        let slope = input
            .operations
            .iter()
            .filter(|o| matches!(o.geometry, Strategy::Drill { .. }))
            .filter_map(|o| input.tools.iter().find(|t| t.id == o.tool))
            .map(|t| 1.0 / (t.tip_angle / 2.0).tan())
            .fold(0.0, f64::max);
        let tolerance = tolerance - POST_ERROR * (1.0 + slope);
        if tolerance <= 0.0 || (has_walls && radial_tolerance <= 0.0) {
            return Ok(None);
        }
        let mut features = Vec::new();
        for op in &input.operations {
            match &op.geometry {
                Strategy::Face => features.push(Feature::Plane(op.bottom + op.allowance)),
                Strategy::Drill { holes, full_diameter, .. } => {
                    let tool = input.tools.iter().find(|t| t.id == op.tool).ok_or_else(|| Error("missing dimensional tool".into()))?;
                    let tip = op.bottom - if *full_diameter { tool.tip_length() } else { 0.0 };
                    for center in holes {
                        if features.len() >= MAX_MOVES {
                            return invalid("dimensional feature budget exceeded");
                        }
                        features.push(Feature::Hole { center: *center, radius: tool.diameter / 2.0, tip, slope: 1.0 / (tool.tip_angle / 2.0).tan() });
                    }
                }
                Strategy::Contour { boundary, inside } => {
                    if !boundary.islands.is_empty() {
                        return invalid("select contour islands as separate operations");
                    }
                    let tool = input.tools.iter().find(|t| t.id == op.tool).ok_or_else(|| Error("missing dimensional tool".into()))?;
                    features.push(Feature::Mill {
                        region: crate::region::Region::new(boundary)?,
                        outside: !inside,
                        width: Some(tool.diameter / 2.),
                        allowance: op.allowance,
                        depth: op.bottom + op.allowance,
                    });
                }
                Strategy::Pocket { boundary } => features.push(Feature::Mill {
                    region: crate::region::Region::new(boundary)?,
                    outside: false,
                    width: None,
                    allowance: op.allowance,
                    depth: op.bottom + op.allowance,
                }),
            }
        }
        Ok(Some(Self {
            features,
            bottom: input.setup.stock.min.z,
            top: input.setup.stock.max.z,
            tolerance,
            radial_tolerance,
            refinement: if has_walls { VERIFY_ERROR.min(tolerance / 8.0).min(radial_tolerance / 4.0) } else { VERIFY_ERROR },
        }))
    }
    pub(crate) async fn envelope<F: std::future::Future<Output = ()>>(
        &self,
        region: ([f64; 2], f64),
        work: &mut usize,
        budget: usize,
        cancelled: &impl Fn() -> bool,
        yield_control: &mut impl FnMut() -> F,
    ) -> Result<Option<Envelope>> {
        let (p, half) = region;
        let unchanged = Envelope::range(self.top, self.top, self.tolerance);
        let mut combined = unchanged;
        for (index, feature) in self.features.iter().enumerate() {
            if index % 4096 == 4095 {
                yield_control().await;
                if cancelled() {
                    return invalid("dimensional verification cancelled");
                }
            }
            *work += 1;
            if *work > budget {
                return Ok(None);
            }
            let allowed = match feature {
                Feature::Plane(z) => {
                    let z = z.clamp(self.bottom, self.top);
                    Envelope::range(z, z, self.tolerance)
                }
                Feature::Mill { region, outside, width, allowance, depth } => {
                    let reach = allowance + width.unwrap_or(0.) + self.radial_tolerance;
                    let Some(mut d) = region.range((p, half, reach), work, budget, cancelled, yield_control).await? else {
                        return Ok(None);
                    };
                    if *outside {
                        d = [-d[1], -d[0]];
                    }
                    let floor = Envelope::range(depth.clamp(self.bottom, self.top), depth.clamp(self.bottom, self.top), self.tolerance);
                    // A contour finishes the wall and adjacent floor out to one
                    // tool radius. Farther stock on the cutting side is optional
                    // roughing stock; it must never be cut below the stated floor.
                    let optional = Envelope { upper_min: unchanged.upper_min, upper_max: unchanged.upper_max, ..floor };
                    let mut possible: Option<Envelope> = None;
                    let mut add = |v| possible = Some(possible.map_or(v, |old| old.alternatives(v)));
                    if d[0] <= allowance - self.radial_tolerance {
                        add(unchanged);
                    }
                    if d[1] >= allowance - self.radial_tolerance && d[0] <= allowance + self.radial_tolerance {
                        add(optional);
                    }
                    if d[1] >= allowance + self.radial_tolerance && width.is_none_or(|w| d[0] <= allowance + w) {
                        add(floor);
                    }
                    if width.is_some_and(|w| d[1] >= allowance + w) {
                        add(optional);
                    }
                    let Some(allowed) = possible else {
                        return Ok(None);
                    };
                    allowed
                }
                Feature::Hole { center, radius, tip, slope } => {
                    let r = (p[0] - center[0]).hypot(p[1] - center[1]);
                    let lo = (r - half).max(0.0);
                    let hi = r + half;
                    let zlo = (tip + lo.min(*radius) * slope).clamp(self.bottom, self.top);
                    let zhi = (tip + hi.min(*radius) * slope).clamp(self.bottom, self.top);
                    let cut = Envelope::range(zlo, zhi, self.tolerance);
                    let mut possible: Option<Envelope> = None;
                    let mut add = |v| {
                        possible = Some(possible.map_or(v, |old| old.alternatives(v)));
                    };
                    if lo <= radius - self.radial_tolerance {
                        add(cut);
                    }
                    if hi >= radius + self.radial_tolerance {
                        add(unchanged);
                    }
                    if hi >= radius - self.radial_tolerance && lo <= radius + self.radial_tolerance {
                        add(Envelope { upper_min: unchanged.upper_min, upper_max: unchanged.upper_max, ..cut });
                    }
                    let Some(allowed) = possible else {
                        return Ok(None);
                    };
                    allowed
                }
            };
            combined = combined.cut(allowed);
        }
        Ok(Some(combined))
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    fn envelope(f: &Finished, p: [f64; 2], half: f64, work: &mut usize, budget: usize) -> Option<Envelope> {
        use std::future::Future;
        let mut ready = || std::future::ready(());
        let mut future = std::pin::pin!(f.envelope((p, half), work, budget, &|| false, &mut ready));
        match future.as_mut().poll(&mut std::task::Context::from_waker(std::task::Waker::noop())) {
            std::task::Poll::Ready(result) => result.unwrap(),
            _ => None,
        }
    }
    fn finished() -> Finished {
        Finished::new(&corpus::fixture("drill").unwrap(), TOLERANCE).unwrap().unwrap()
    }
    #[test]
    fn generated_contours_and_pockets_certify_selected_finished_regions() {
        for name in ["contour", "pocket"] {
            let input = corpus::fixture(name).unwrap();
            let path = generate::generate(input.clone()).unwrap();
            let code = nc::post(&path, &input.setup).unwrap();
            let report = simulate(&input, &code);
            assert_eq!(
                report.status,
                Status::Pass,
                "{name} cells {} work {} {:?}",
                report.cells.len(),
                report.work,
                report.findings.iter().take(4).collect::<Vec<_>>()
            );
            assert_eq!(
                report.finished_dimensions,
                Status::Pass,
                "{name} cells {} work {} {:?}",
                report.cells.len(),
                report.work,
                report.findings.iter().take(4).collect::<Vec<_>>()
            );
        }
    }
    #[test]
    fn missed_milling_wrong_floor_and_inaccessible_corners_fail() {
        for name in ["contour", "pocket"] {
            let input = corpus::fixture(name).unwrap();
            let code = nc::post(&generate::generate(input.clone()).unwrap(), &input.setup).unwrap();
            let wrong = simulate(&input, &code.replace("Z5.000000", "Z5.100000"));
            assert_eq!(wrong.finished_dimensions, Status::Fail, "{name}");
            let missed = simulate(&input, "G21 G90 G17 G94 G54 G40 G49 G80\nG0 X0 Y0 Z15\nM30\n");
            assert_eq!(missed.finished_dimensions, Status::Fail, "{name}");
        }
        let mut input = corpus::fixture("pocket").unwrap();
        input.operations[0].geometry =
            Strategy::Pocket { boundary: Boundary { outer: vec![[2., 2.], [18., 2.], [18., 18.], [2., 18.]], islands: vec![] } };
        // A round cutter cannot finish sharp internal corners. Protected geometry
        // is independently relaxed here to isolate the dimensional failure.
        for t in &mut input.geometry.target {
            for p in t {
                p.z = 0.;
            }
        }
        let code = nc::post(&generate::generate(input.clone()).unwrap(), &input.setup).unwrap();
        assert_eq!(simulate(&input, &code).finished_dimensions, Status::Fail);
    }
    #[test]
    fn allowance_winding_multiple_depths_and_external_contours() {
        let mut input = corpus::fixture("contour").unwrap();
        input.operations[0].allowance = 0.2;
        input.operations[0].stepdown = 0.3;
        if let Strategy::Contour { boundary, .. } = &mut input.operations[0].geometry {
            boundary.outer.reverse();
        }
        let code = nc::post(&generate::generate(input.clone()).unwrap(), &input.setup).unwrap();
        let r = simulate(&input, &code);
        assert_eq!(r.finished_dimensions, Status::Pass, "{:?}", r.findings);
        if let Strategy::Contour { inside, .. } = &mut input.operations[0].geometry {
            *inside = false;
        }
        // The separate protected-CAD tests exercise real walls; this case checks
        // only the independent outside/inside selection and allowance contract.
        for t in &mut input.geometry.target {
            for p in t {
                p.z = 0.;
            }
        }
        let code = nc::post(&generate::generate(input.clone()).unwrap(), &input.setup).unwrap();
        let r = simulate(&input, &code);
        assert_eq!(r.finished_dimensions, Status::Pass, "{:?}", r.findings);
    }
    #[test]
    fn pocket_islands_are_protected_and_finished() {
        let mut input = corpus::fixture("pocket").unwrap();
        if let Strategy::Pocket { boundary } = &mut input.operations[0].geometry {
            boundary.islands.push(
                (0..48)
                    .map(|i| {
                        let a = i as f64 * std::f64::consts::TAU / 48.;
                        [10. + a.cos(), 10. + a.sin()]
                    })
                    .collect(),
            );
        }
        input.tools[0].diameter = 2.;
        input.operations[0].stepover = 0.75;
        let code = nc::post(&generate::generate(input.clone()).unwrap(), &input.setup).unwrap();
        let r = simulate(&input, &code);
        assert_eq!(
            r.finished_dimensions,
            Status::Pass,
            "cells {} work {} {:?}",
            r.cells.len(),
            r.work,
            r.findings.iter().take(3).collect::<Vec<_>>()
        );
        let mut without = input.clone();
        if let Strategy::Pocket { boundary } = &mut without.operations[0].geometry {
            boundary.islands.clear();
        }
        let code = nc::post(&generate::generate(without.clone()).unwrap(), &without.setup).unwrap();
        assert_eq!(simulate(&input, &code).finished_dimensions, Status::Fail);
        if let Strategy::Pocket { boundary } = &mut input.operations[0].geometry {
            boundary.islands = [8., 12.]
                .into_iter()
                .map(|x| {
                    (0..48)
                        .map(|i| {
                            let a = i as f64 * std::f64::consts::TAU / 48.;
                            [x + 0.8 * a.cos(), 10. + 0.8 * a.sin()]
                        })
                        .collect()
                })
                .collect();
        }
        let code = nc::post(&generate::generate(input.clone()).unwrap(), &input.setup).unwrap();
        let r = simulate(&input, &code);
        assert_eq!(
            r.finished_dimensions,
            Status::Pass,
            "cells {} work {} {:?}",
            r.cells.len(),
            r.work,
            r.findings.iter().take(3).collect::<Vec<_>>()
        );
    }
    #[test]
    fn associated_profile_chord_error_has_a_separate_budget() {
        let mut input = corpus::fixture("pocket").unwrap();
        input.operations[0].source = Some(ProfileRef { sketch: 1, curves: vec!["wall".into()], occurrence: vec![] });
        let f = Finished::new(&input, TOLERANCE).unwrap().unwrap();
        assert!((f.radial_tolerance - (TOLERANCE / 2. - POST_ERROR - crate::profile_tolerance(TOLERANCE))).abs() < 1e-12);
        assert_eq!(crate::profile_tolerance(TOLERANCE), 0.0005);
        // A looser operation can coexist with a tighter program tolerance. Use
        // the actual sampling budget of each source, not the program minimum.
        input.operations[0].tolerance = 0.1;
        let f = Finished::new(&input, 0.005).unwrap().unwrap();
        assert!((f.radial_tolerance - 0.001).abs() < 1e-12);
        let mut unused = input.tools[0].clone();
        unused.id = 2;
        unused.number = 2;
        unused.kind = ToolKind::TwistDrill;
        unused.tip_angle = 0.11;
        input.tools.push(unused);
        assert!(Finished::new(&input, 0.005).unwrap().is_some());
    }
    #[test]
    fn wrong_milling_diameter_and_shifted_paths_are_detected() {
        let mut input = corpus::fixture("contour").unwrap();
        let code = nc::post(&generate::generate(input.clone()).unwrap(), &input.setup).unwrap();
        for diameter in [5.8, 6.2] {
            let mut other = input.tools[0].clone();
            other.id = 2;
            other.number = 2;
            other.diameter = diameter;
            input.tools.truncate(1);
            input.tools.push(other);
            assert_eq!(simulate(&input, &code.replace("T1 M6", "T2 M6")).finished_dimensions, Status::Fail);
        }
        let mut shifted = nc::parse(&code, &input.tools).unwrap();
        for record in &mut shifted.records {
            match &mut record.motion {
                Motion::Rapid { to } | Motion::Linear { to, .. } => to.x += 0.03,
                Motion::Arc { to, center, .. } => {
                    to.x += 0.03;
                    center.x += 0.03;
                }
                _ => {}
            }
        }
        assert_eq!(verify::simulate(&input, &shifted, Default::default()).unwrap().finished_dimensions, Status::Fail);
    }
    #[test]
    fn hand_calculated_cone_and_diameter_band() {
        let f = finished();
        let mut work = 0;
        assert_eq!(envelope(&f, [10., 10.], 0., &mut work, 100).unwrap().classify(5., 5.), Status::Pass);
        let z = 5. + 0.25 / (59_f64.to_radians().tan());
        assert_eq!(envelope(&f, [10.25, 10.], 0., &mut work, 100).unwrap().classify(z, z), Status::Pass);
        assert_eq!(envelope(&f, [10.49, 10.], 0., &mut work, 100).unwrap().classify(6., 6.), Status::Fail);
        assert_eq!(envelope(&f, [10.51, 10.], 0., &mut work, 100).unwrap().classify(5.3, 5.3), Status::Fail);
        assert_eq!(envelope(&f, [10.5, 10.], 0., &mut work, 100).unwrap().classify(5.31, 6.), Status::Pass);
    }
    #[test]
    fn boundary_uncertainty_and_resource_limits_never_pass() {
        let f = finished();
        let mut work = 0;
        assert_eq!(envelope(&f, [10.5, 10.], 0.01, &mut work, 100).unwrap().classify(5.3, 6.), Status::Inconclusive);
        assert!(envelope(&f, [10., 10.], 0., &mut work, 0).is_none());
    }
    fn program(tool: u32, x: f64, depth: f64) -> String {
        format!(
            "G21 G90 G17 G94 G54 G40 G49 G80\nG0 X0 Y0 Z15\nT{tool} M6\nS6000 M3\nM9\nG0 X{x} Y10 Z15\nG1 Z6 F100\nG1 Z{depth} F100\nG0 Z15\nM5\nM9\nM30\n"
        )
    }
    fn simulate(input: &JobInput, code: &str) -> crate::verify::Report {
        let parsed = nc::parse(code, &input.tools).unwrap();
        verify::simulate(input, &parsed, Default::default()).unwrap()
    }
    #[test]
    fn blind_hole_certifies_and_bad_depth_position_diameter_fail() {
        let mut input = corpus::fixture("drill").unwrap();
        let correct = simulate(&input, &program(1, 10., 5.));
        assert_eq!(correct.status, Status::Pass);
        assert_eq!(correct.finished_dimensions, Status::Pass, "{:?}", correct.findings);
        for (x, depth) in [(10., 5.1), (10., 4.9), (10.03, 5.)] {
            assert_eq!(simulate(&input, &program(1, x, depth)).finished_dimensions, Status::Fail);
        }
        for diameter in [0.8, 1.2] {
            let mut other = input.tools[0].clone();
            other.id = 2;
            other.number = 2;
            other.diameter = diameter;
            other.shank_diameter = diameter;
            input.tools.truncate(1);
            input.tools.push(other);
            assert_eq!(simulate(&input, &program(2, 10., 5.)).finished_dimensions, Status::Fail);
        }
    }
    #[test]
    fn absent_hole_is_not_hidden_by_a_coarse_stock_cell() {
        let input = corpus::fixture("drill").unwrap();
        let code = "G21 G90 G17 G94 G54 G40 G49 G80\nG0 X0 Y0 Z15\nM30\n";
        let report = simulate(&input, code);
        assert_eq!(report.finished_dimensions, Status::Fail);
        assert!(report.findings.iter().any(|f| f.kind == "finished_region_error"));
    }
    #[test]
    fn full_diameter_blind_and_through_depths_are_certified() {
        for bottom in [4.5, 0.] {
            let mut input = corpus::fixture("drill").unwrap();
            input.operations[0].bottom = bottom;
            input.operations[0].geometry = Strategy::Drill { holes: vec![[10., 10.]], peck: None, full_diameter: true };
            for tri in &mut input.geometry.target {
                for p in tri {
                    p.z = 0.;
                }
            }
            let depth = bottom - input.tools[0].tip_length();
            let report = simulate(&input, &program(1, 10., depth));
            assert_eq!(report.status, Status::Pass, "{:?}", report.findings);
            assert_eq!(report.finished_dimensions, Status::Pass, "{:?}", report.findings);
        }
    }
    #[test]
    fn declared_drill_allowance_is_rejected_instead_of_ignored() {
        let mut input = corpus::fixture("drill").unwrap();
        input.operations[0].allowance = 0.1;
        assert!(input.validate().is_err());
    }
    #[test]
    fn facing_and_multiple_holes_compose_without_restoring_stock() {
        let mut input = corpus::fixture("drill").unwrap();
        for t in &mut input.geometry.target {
            for p in t {
                p.z = 0.0;
            }
        }
        input.operations[0].geometry = Strategy::Drill { holes: vec![[10., 10.], [12., 10.]], peck: Some(0.3), full_diameter: false };
        let mut face = corpus::fixture("face").unwrap();
        face.tools[0].id = 2;
        face.tools[0].number = 2;
        face.operations[0].id = 2;
        face.operations[0].tool = 2;
        face.operations[0].bottom = 5.8;
        input.tools.push(face.tools[0].clone());
        input.operations.push(face.operations[0].clone());
        let path = generate::generate(input.clone()).unwrap();
        let code = nc::post(&path, &input.setup).unwrap();
        let report = simulate(&input, &code);
        assert_eq!(report.finished_dimensions, Status::Pass, "{:?}", report.findings);
    }
    #[test]
    fn dimensional_work_yields_and_cancels() {
        use std::{
            future::Future,
            task::{Context, Poll, Waker},
        };
        let mut f = finished();
        f.features = (0..5000).map(|_| Feature::Plane(5.)).collect();
        let cancel = std::cell::Cell::new(false);
        let mut yield_once = || {
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
        let mut work = 0;
        let cancelled = || cancel.get();
        let mut future = std::pin::pin!(f.envelope(([10., 10.], 0.001), &mut work, 10000, &cancelled, &mut yield_once));
        let mut context = Context::from_waker(Waker::noop());
        assert!(future.as_mut().poll(&mut context).is_pending());
        cancel.set(true);
        assert!(matches!(future.as_mut().poll(&mut context), Poll::Ready(Err(_))));
    }
    #[test]
    fn equivalent_inch_and_mm_holes_certify_the_same_dimensions() {
        let input = corpus::fixture("drill").unwrap();
        let inch = format!(
            "G20 G90 G17 G94 G54 G40 G49 G80\nG0 X0 Y0 Z{}\nT1 M6\nS6000 M3\nM9\nG0 X{} Y{} Z{}\nG1 Z{} F{}\nG1 Z{}\nG0 Z{}\nM5\nM9\nM30\n",
            15. / 25.4,
            10. / 25.4,
            10. / 25.4,
            15. / 25.4,
            6. / 25.4,
            100. / 25.4,
            5. / 25.4,
            15. / 25.4
        );
        let metric = simulate(&input, &program(1, 10., 5.));
        let imperial = simulate(&input, &inch);
        assert_eq!(imperial.finished_dimensions, Status::Pass);
        assert!((imperial.cutting_distance - metric.cutting_distance).abs() < 1e-10);
        assert!((imperial.remaining_volume_high - metric.remaining_volume_high).abs() < 1e-8);
    }
}
