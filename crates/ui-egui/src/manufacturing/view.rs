//! Manufacture display choices. Geometry comes from immutable engine jobs;
//! this module changes neither the model nor the verification result.
use super::*;
use crate::theme::Tokens;
use solidcraft_engine::{
    geom::{Aabb3, Mesh},
    render::{Rgb, Scene, SceneMesh},
};
use std::{
    collections::BTreeSet,
    hash::{Hash, Hasher},
    sync::Arc,
};

#[derive(Clone, Copy, Default, Debug, PartialEq, Eq, Hash)]
pub enum Workpiece {
    #[default]
    Model,
    InitialStock,
    RemainingStock,
}
pub struct View {
    pub workpiece: Workpiece,
    pub fixtures: bool,
    pub paths: bool,
    pub boundary: bool,
    pub speed: f64,
    pub loaded_job: Option<u64>,
    pub fraction: f64,
}
impl Default for View {
    fn default() -> Self {
        Self { workpiece: Workpiece::Model, fixtures: true, paths: true, boundary: true, speed: 60., loaded_job: None, fraction: 0. }
    }
}
pub fn setup(app: &SolveApp) -> Option<&cam::Setup> {
    if let Some(Editor::Setup(setup)) = &app.manufacturing.editor {
        return Some(setup);
    }
    let m = &app.session.doc.manufacturing;
    let program = m.programs.iter().find(|p| p.id == app.manufacturing.program).or_else(|| m.programs.first());
    program.and_then(|p| m.setups.iter().find(|s| s.id == p.setup)).or_else(|| m.setups.first())
}
pub fn corners(stock: cam::Stock) -> [Vec3; 8] {
    let a = stock.min;
    let b = stock.max;
    [
        Vec3::new(a.x, a.y, a.z),
        Vec3::new(b.x, a.y, a.z),
        Vec3::new(b.x, b.y, a.z),
        Vec3::new(a.x, b.y, a.z),
        Vec3::new(a.x, a.y, b.z),
        Vec3::new(b.x, a.y, b.z),
        Vec3::new(b.x, b.y, b.z),
        Vec3::new(a.x, b.y, b.z),
    ]
}
pub fn bounds(app: &SolveApp) -> Aabb3 {
    let mut bounds = Aabb3::EMPTY;
    if app.ui.tab != "MILLING" {
        return bounds;
    }
    if let Some(setup) = setup(app).filter(|s| s.validate().is_ok()) {
        for p in corners(setup.stock) {
            bounds.add(setup.frame.world(p));
        }
        for p in corners(cam::Stock { min: setup.stock.min, max: Vec3::new(setup.stock.max.x, setup.stock.max.y, setup.clearance) }) {
            bounds.add(setup.frame.world(p));
        }
    }
    bounds
}
pub fn fresh_job(app: &SolveApp) -> Option<Arc<solidcraft_engine::manufacturing::Job>> {
    let job = solidcraft_engine::manufacturing::job(&app.session, app.manufacturing.job?).ok()?;
    solidcraft_engine::manufacturing::current(&app.session, &job).ok()?;
    Some(job)
}
pub fn stock(app: &SolveApp) -> Option<Arc<Mesh>> {
    if app.ui.tab != "MILLING" || matches!(app.manufacturing.editor, Some(Editor::Setup(_))) {
        return None;
    }
    match app.manufacturing.view.workpiece {
        Workpiece::Model => None,
        Workpiece::RemainingStock => fresh_job(app)?.stock_mesh().ok().flatten(),
        Workpiece::InitialStock => {
            let setup = setup(app).filter(|s| s.validate().is_ok())?;
            let points = corners(setup.stock).map(|p| setup.frame.world(p));
            let mut mesh = Mesh::default();
            for face in [[0, 3, 2, 1], [4, 5, 6, 7], [0, 1, 5, 4], [1, 2, 6, 5], [2, 3, 7, 6], [3, 0, 4, 7]] {
                for indices in [[face[0], face[1], face[2]], [face[0], face[2], face[3]]] {
                    let t = indices.map(|i| points[i]);
                    let n = (t[1] - t[0]).cross(t[2] - t[0]).normalized().unwrap_or(Vec3::Z);
                    let base = mesh.positions.len() as u32;
                    mesh.positions.extend(t);
                    mesh.normals.extend([n; 3]);
                    mesh.triangles.push([base, base + 1, base + 2]);
                }
            }
            Some(Arc::new(mesh))
        }
    }
}
fn names(app: &SolveApp, refs: &[cam::BodyRef]) -> BTreeSet<String> {
    refs.iter()
        .filter_map(|r| {
            if r.occurrence.is_empty() {
                Some(r.body.clone())
            } else {
                app.session.occurrence_placement(&r.occurrence).ok()?.bodies.into_iter().find(|b| b.source == r.body).map(|b| b.world)
            }
        })
        .collect()
}
pub struct Bodies {
    models: BTreeSet<String>,
    fixtures: BTreeSet<String>,
    active: bool,
    show_model: bool,
    show_fixtures: bool,
}
impl Bodies {
    pub fn new(app: &SolveApp) -> Self {
        let setup = setup(app).filter(|_| app.ui.tab == "MILLING");
        Self {
            models: setup.map(|s| names(app, &s.models)).unwrap_or_default(),
            fixtures: setup.map(|s| names(app, &s.fixtures)).unwrap_or_default(),
            active: setup.is_some() && !matches!(app.manufacturing.editor, Some(Editor::Setup(_))),
            show_model: stock(app).is_none(),
            show_fixtures: app.manufacturing.view.fixtures,
        }
    }
    pub fn visible(&self, name: &str) -> bool {
        !self.active || (self.fixtures.contains(name) && self.show_fixtures) || (self.models.contains(name) && self.show_model)
    }
    pub fn color(&self, name: &str) -> Option<Color32> {
        (self.active && self.fixtures.contains(name)).then_some(Tokens::get().cam_fixture)
    }
}
pub fn key(app: &SolveApp, h: &mut impl Hasher) {
    let active = app.ui.tab == "MILLING";
    active.hash(h);
    if !active {
        return;
    }
    matches!(app.manufacturing.editor, Some(Editor::Setup(_))).hash(h);
    if let Some(Editor::Setup(setup)) = &app.manufacturing.editor {
        for r in &setup.fixtures {
            r.body.hash(h);
            r.occurrence.hash(h);
        }
    }
    app.manufacturing.program.hash(h);
    app.manufacturing.job.hash(h);
    app.manufacturing.view.workpiece.hash(h);
    app.manufacturing.view.fixtures.hash(h);
    fresh_job(app).and_then(|j| j.stock_mesh().ok().flatten()).map(|m| Arc::as_ptr(&m) as usize).hash(h);
}
pub fn gpu(app: &SolveApp, scene: &mut crate::gpu::GpuScene) {
    let Some(mesh) = stock(app) else { return };
    let color = Tokens::get().cam_stock.to_array();
    for indices in &mesh.triangles {
        for i in indices {
            if let (Some(p), Some(n)) = (mesh.positions.get(*i as usize), mesh.normals.get(*i as usize)) {
                scene.tri(p.to_f32(), n.to_f32(), color);
            }
        }
    }
}
pub fn cpu(app: &SolveApp, scene: &mut Scene) {
    if app.ui.tab != "MILLING" {
        return;
    }
    // Replace the model mesh list with the selected machining bodies, preserving
    // occurrence meshes and applying the same visibility/color choices as GPU rendering.
    let bodies = Bodies::new(app);
    let world = app.session.world_state();
    scene.meshes.clear();
    for body in &world.bodies {
        if !app.session.body_visible(&body.name) || !bodies.visible(&body.name) {
            continue;
        }
        let c = bodies.color(&body.name).unwrap_or(Tokens::get().body);
        scene.meshes.push(SceneMesh { mesh: body.mesh(), color: Rgb(c.r(), c.g(), c.b()), face_colors: vec![] });
    }
    // CAD edge lines would reveal hidden target geometry inside the stock preview.
    // CAM outlines and paths are drawn by the shared overlay afterwards.
    scene.lines.clear();
    if let Some(mesh) = stock(app) {
        let c = Tokens::get().cam_stock;
        scene.meshes.push(SceneMesh { mesh, color: Rgb(c.r(), c.g(), c.b()), face_colors: vec![] });
    }
    scene.radius = scene.radius.max(bounds(app).diagonal());
}

/// Complete motion blocks. Zero is the initial pose, never a sentinel for all.
pub fn advance(state: &mut State, count: usize, dt: f64) {
    if state.view.loaded_job != state.job {
        state.view.loaded_job = state.job;
        state.playback = count;
        state.playing = false;
        state.view.fraction = 0.;
    }
    state.playback = state.playback.min(count);
    if !state.playing {
        return;
    }
    state.view.fraction += dt.clamp(0., 0.1) * state.view.speed.clamp(1., 1000.);
    let steps = state.view.fraction.floor() as usize;
    state.view.fraction -= steps as f64;
    state.playback = state.playback.saturating_add(steps).min(count);
    if state.playback == count {
        state.playing = false;
        state.view.fraction = 0.;
    }
}

/// Draw the same motion prefix for native overlays and CPU snapshots.
pub fn segments(path: &cam::Toolpath, setup: &cam::Setup, completed: usize, mut line: impl FnMut(Vec3, Vec3, bool)) {
    let mut pos = Vec3::new(0., 0., setup.clearance);
    for block in path.blocks.iter().take(completed) {
        match block.motion {
            cam::Motion::Rapid { to } | cam::Motion::Linear { to, .. } => {
                line(setup.frame.world(pos), setup.frame.world(to), matches!(block.motion, cam::Motion::Rapid { .. }));
                pos = to;
            }
            cam::Motion::Arc { to, center, clockwise, .. } => {
                let start = (pos.y - center.y).atan2(pos.x - center.x);
                let end = (to.y - center.y).atan2(to.x - center.x);
                let delta = (end - start).rem_euclid(std::f64::consts::TAU);
                let sweep = if clockwise {
                    if delta < 1e-12 { -std::f64::consts::TAU } else { delta - std::f64::consts::TAU }
                } else if delta < 1e-12 {
                    std::f64::consts::TAU
                } else {
                    delta
                };
                let radius = (pos.x - center.x).hypot(pos.y - center.y);
                let angle = (2. * (1. - 0.02 / radius).clamp(-1., 1.).acos()).max(0.001);
                let n = (sweep.abs() / angle).ceil().clamp(1., 512.) as usize;
                let mut previous = pos;
                for i in 1..=n {
                    let theta = start + sweep * i as f64 / n as f64;
                    let p = if i == n { to } else { Vec3::new(center.x + radius * theta.cos(), center.y + radius * theta.sin(), to.z) };
                    line(setup.frame.world(previous), setup.frame.world(p), false);
                    previous = p;
                }
                pos = to;
            }
            _ => {}
        }
    }
}
pub fn severity_color(severity: cam::verify::FindingSeverity) -> Color32 {
    match severity {
        cam::verify::FindingSeverity::Error => Tokens::get().error,
        cam::verify::FindingSeverity::Unresolved => Tokens::get().warning,
        cam::verify::FindingSeverity::Information => Tokens::get().cam_stock,
    }
}
pub fn finding_marker(app: &SolveApp) -> Option<(Vec3, Color32)> {
    let index = app.manufacturing.finding?;
    let job = fresh_job(app)?;
    let JobState::Verified(_, report) = job.state().ok()? else { return None };
    let finding = report.findings.get(index)?;
    Some((job.input().ok()?.setup.frame.world(finding.at), severity_color(finding.severity)))
}
pub fn cpu_overlays(app: &SolveApp, scene: &mut Scene) {
    if app.ui.tab != "MILLING" {
        return;
    }
    let tokens = Tokens::get();
    let mut line = |a: Vec3, b: Vec3, c: Color32| {
        scene.lines.push(solidcraft_engine::render::SceneLine { points: vec![a, b], color: Rgb(c.r(), c.g(), c.b()), width: 1.2, on_top: true })
    };
    if app.manufacturing.view.boundary
        && let Some(setup) = setup(app)
    {
        let points = corners(setup.stock).map(|p| setup.frame.world(p));
        for (a, b) in [(0, 1), (1, 2), (2, 3), (3, 0), (4, 5), (5, 6), (6, 7), (7, 4), (0, 4), (1, 5), (2, 6), (3, 7)] {
            line(points[a], points[b], tokens.cam_stock);
        }
    }
    if let Some((point, color)) = finding_marker(app) {
        let size = setup(app).map_or(1., |s| (s.stock.max - s.stock.min).len() / 100.);
        for delta in [Vec3::X * size, Vec3::Y * size, Vec3::Z * size] {
            line(point - delta, point + delta, color);
        }
    }
    if !app.manufacturing.view.paths {
        return;
    }
    let Some(job) = fresh_job(app) else { return };
    let Ok(input) = job.input() else { return };
    let path = match job.state() {
        Ok(JobState::Generated(p) | JobState::Verified(p, _)) => p,
        _ => return,
    };
    segments(&path, &input.setup, app.manufacturing.playback, |a, b, rapid| line(a, b, if rapid { tokens.cam_rapid } else { tokens.cam_cut }));
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    #[test]
    fn playback_zero_is_initial_and_rate_is_independent_of_frames() {
        let mut s = State { job: Some(1), ..Default::default() };
        advance(&mut s, 100, 0.);
        assert_eq!(s.playback, 100);
        s.playback = 0;
        advance(&mut s, 100, 0.1);
        assert_eq!(s.playback, 0);
        for fps in [10, 30, 60] {
            s.playback = 0;
            s.playing = true;
            s.view.fraction = 0.;
            for _ in 0..fps {
                advance(&mut s, 100, 1. / fps as f64);
            }
            assert_eq!(s.playback, 60);
        }
        s.job = Some(2);
        advance(&mut s, 10, 0.1);
        assert_eq!(s.playback, 10);
        assert!(!s.playing);
    }
    #[test]
    fn full_circle_playback_preserves_direction_and_empty_prefix() {
        let input = cam::corpus::fixture("face").unwrap();
        for clockwise in [true, false] {
            let path = cam::Toolpath {
                fingerprint: "test".into(),
                blocks: vec![
                    cam::Block { operation: 1, motion: cam::Motion::Rapid { to: Vec3::new(1., 0., 0.) } },
                    cam::Block { operation: 1, motion: cam::Motion::Arc { to: Vec3::new(1., 0., 0.), center: Vec3::ZERO, clockwise, feed: 100. } },
                ],
            };
            let mut lines = Vec::new();
            segments(&path, &input.setup, 0, |a, b, _| lines.push((a, b)));
            assert!(lines.is_empty());
            segments(&path, &input.setup, 2, |a, b, rapid| {
                if !rapid {
                    lines.push((a, b))
                }
            });
            assert!(lines.len() > 8);
            assert_eq!(lines.last().unwrap().1, Vec3::new(1., 0., 0.));
            assert_eq!(lines[0].1.y < 0., clockwise);
            let length: f64 = lines.iter().map(|(a, b)| a.dist(*b)).sum();
            assert!((length - std::f64::consts::TAU).abs() < 0.1);
        }
    }
    #[test]
    fn stock_display_uses_setup_frame_and_respects_fixture_visibility() {
        let mut h = crate::scenario::Harness::new();
        h.app.run("solid.box", json!({"length":20,"width":20,"height":5})).unwrap();
        h.app.run("solid.box", json!({"length":3,"width":3,"height":3})).unwrap();
        let names: Vec<_> = h.app.session.model.state().bodies.iter().map(|b| b.name.clone()).collect();
        let mut input = cam::corpus::fixture("face").unwrap();
        input.setup.models[0].body = names[0].clone();
        input.setup.fixtures = vec![cam::BodyRef { body: names[1].clone(), occurrence: vec![] }];
        input.setup.frame = cam::Frame { origin: Vec3::new(40., 30., 2.), x: Vec3::Y, y: -Vec3::X, z: Vec3::Z };
        h.app.run("cam.setup.put", json!({"setup":input.setup})).unwrap();
        h.app.ui.tab = "MILLING".into();
        h.app.manufacturing.view.workpiece = Workpiece::InitialStock;
        let mesh = stock(&h.app).unwrap();
        let b = mesh.bounds();
        assert_eq!(b.min, Vec3::new(20., 30., 2.));
        assert_eq!(b.max, Vec3::new(40., 50., 8.));
        assert!((mesh.measure().volume - 2400.).abs() < 1e-8);
        let bodies = Bodies::new(&h.app);
        assert!(!bodies.visible(&names[0]));
        assert!(bodies.visible(&names[1]));
        assert_eq!(bodies.color(&names[1]), Some(Tokens::get().cam_fixture));
        h.app.manufacturing.view.fixtures = false;
        assert!(!Bodies::new(&h.app).visible(&names[1]));
        let mut scene = solidcraft_engine::view::scene(&h.app.session, &h.app.cam);
        cpu(&h.app, &mut scene);
        assert_eq!(scene.meshes.len(), 1);
        assert_eq!(scene.meshes[0].mesh.bounds().min, b.min);
        h.app.manufacturing.editor = Some(Editor::Setup(input.setup));
        assert!(Bodies::new(&h.app).visible(&names[1]));
        assert!(stock(&h.app).is_none());
    }
}
