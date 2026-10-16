//! Manufacture workspace. All edits and jobs use the public CAM commands.
#[cfg(any(target_arch = "wasm32", test))]
mod scheduler;
pub(crate) mod view;
use crate::{SolveApp, theme::Tokens};
use egui::{Color32, Stroke};
use serde_json::json;
use solidcraft_engine::{cam, geom::Vec3, manufacturing::JobState};

#[derive(Default)]
pub struct State {
    editor: Option<Editor>,
    pub program: u64,
    pub job: Option<u64>,
    pending_export: Option<u64>,
    pub playback: usize,
    pub playing: bool,
    pub finding: Option<usize>,
    error: String,
    pub view: view::View,
    #[cfg(target_arch = "wasm32")]
    poll_cursor: usize,
}
/// Share a four-millisecond target across all jobs, preserving round-robin order
/// between frames. Individual opaque geometry calls can still exceed that target.
#[cfg(target_arch = "wasm32")]
pub(crate) fn advance(app: &mut SolveApp, ctx: &egui::Context) {
    let started = web_time::Instant::now();
    let jobs: Vec<_> = app.session.cam_jobs.jobs.values().collect();
    let pending = scheduler::tick(
        &mut app.manufacturing.poll_cursor,
        jobs.len(),
        |i| jobs.get(i).is_some_and(|job| solidcraft_engine::manufacturing::advance(job)),
        || started.elapsed() < std::time::Duration::from_millis(4),
    );
    if pending {
        ctx.request_repaint();
    }
}

enum Editor {
    Setup(cam::Setup),
    Tool(cam::Tool),
    Operation(cam::Operation),
    Program(cam::Program),
}
fn next(ids: impl Iterator<Item = u64>) -> u64 {
    ids.max().unwrap_or(0).saturating_add(1)
}
pub fn cancel_editor(app: &mut SolveApp) {
    app.manufacturing.editor = None;
}
pub fn start(app: &mut SolveApp, id: &str) {
    crate::dialogs::cancel(app);
    app.tool = None;
    app.ui.tab = "MILLING".into();
    let m = &app.session.doc.manufacturing;
    match id {
        "cam.setup.put" => {
            let st = app.session.model.state();
            let Some(b) = st.bodies.first() else {
                app.set_status("Create a solid before a milling setup", true);
                return;
            };
            let bounds = b.mesh().bounds();
            app.manufacturing.editor = Some(Editor::Setup(cam::Setup {
                id: next(m.setups.iter().map(|s| s.id)),
                name: "Milling setup".into(),
                stock: cam::Stock { min: bounds.min, max: bounds.max + Vec3::new(0.0, 0.0, 1.0) },
                frame: cam::Frame::default(),
                models: vec![cam::BodyRef { body: b.name.clone(), occurrence: vec![] }],
                fixtures: vec![],
                clearance: bounds.max.z + 15.0,
                retract: bounds.max.z + 5.0,
                work_offset: 54,
            }));
        }
        "cam.tool.put" => {
            if let Ok(mut f) = cam::corpus::fixture("face")
                && let Some(mut t) = f.tools.pop()
            {
                t.id = next(m.tools.iter().map(|t| t.id));
                t.number = t.id as u32;
                app.manufacturing.editor = Some(Editor::Tool(t));
            }
        }
        "cam.operation.put" => {
            let (Some(s), Some(t)) = (m.setups.first(), m.tools.first()) else {
                app.set_status("Create a setup and a tool first", true);
                return;
            };
            if let Ok(mut f) = cam::corpus::fixture("face")
                && let Some(mut o) = f.operations.pop()
            {
                o.id = next(m.operations.iter().map(|o| o.id));
                o.setup = s.id;
                o.tool = t.id;
                o.name = "Facing".into();
                o.top = s.stock.max.z;
                o.bottom = o.top - 1.0;
                o.stepover = t.diameter / 3.0;
                app.manufacturing.editor = Some(Editor::Operation(o));
            }
        }
        "cam.program.put" => {
            let Some(s) = m.setups.first() else {
                app.set_status("Create a setup first", true);
                return;
            };
            app.manufacturing.editor = Some(Editor::Program(cam::Program {
                id: next(m.programs.iter().map(|p| p.id)),
                name: "NC program".into(),
                setup: s.id,
                operations: m.operations.iter().filter(|o| o.setup == s.id).map(|o| o.id).collect(),
            }));
        }
        "cam.generate" => {
            let p = if app.manufacturing.program == 0 { m.programs.first().map(|p| p.id).unwrap_or(0) } else { app.manufacturing.program };
            if let Ok(v) = app.run(id, json!({"program":p,"background":true})) {
                app.manufacturing.job = v["job"].as_u64();
                app.manufacturing.playback = 0;
                app.manufacturing.playing = false;
                app.manufacturing.view.loaded_job = None;
                app.manufacturing.finding = None;
            }
        }
        "cam.simulate" => {
            if let Some(job) = app.manufacturing.job {
                let _ = app.run(id, json!({"job":job,"background":true}));
            }
        }
        "cam.post" => export(app, "gcode"),
        _ => {}
    }
}
fn export(app: &mut SolveApp, format: &str) {
    if app.manufacturing.pending_export.is_some() {
        app.set_status("Finish or cancel the current export first", true);
        return;
    }
    let Some(job) = app.manufacturing.job else {
        app.set_status("Generate a program first", true);
        return;
    };
    let ext = match format {
        "gcode" => "nc",
        "stock" => "stl",
        "setup_sheet" => "html",
        _ => "json",
    };
    let format = format.to_owned();
    app.pick_save(&format!("milling.{ext}"), &[ext], move |app, path| {
        let Some(path) = path else { return };
        let cmd = if format == "gcode" { "cam.post" } else { "cam.export" };
        if app.run(cmd, json!({"job":job,"format":format,"path":path,"background":true})).is_ok() {
            app.manufacturing.pending_export = Some(job);
            app.set_status("Preparing machining export…", false);
        }
    });
}
/// Collect through a command only after the worker finishes; the command rechecks freshness.
pub(crate) fn update_exports(app: &mut SolveApp, ctx: &egui::Context) {
    let Some(id) = app.manufacturing.pending_export else {
        return;
    };
    let state = solidcraft_engine::manufacturing::job(&app.session, id).and_then(|job| {
        if let JobState::Failed(error) = job.state()? {
            return Err(solidcraft_engine::EngineError::Other(error));
        }
        solidcraft_engine::manufacturing::exports::status(&job)
    });
    match state {
        Ok(state) if state["state"] == "running" => {
            ctx.request_repaint_after(std::time::Duration::from_millis(16));
        }
        Ok(state) if state["state"] == "ready" => {
            app.manufacturing.pending_export = None;
            if let Ok(result) = app.run("cam.export.result", json!({"job":id})) {
                app.set_status(format!("Export saved · {} bytes", result["bytes"]), false);
            }
        }
        other => {
            app.manufacturing.pending_export = None;
            let message = match other {
                Ok(state) => state["error"].as_str().unwrap_or("CAM export cancelled").to_owned(),
                Err(error) => error.to_string(),
            };
            app.set_status(message, true);
        }
    }
}
pub fn panel(app: &mut SolveApp, ui: &mut egui::Ui) {
    let model = app.session.doc.manufacturing.clone();
    let mut action = None;
    let previous_program = app.manufacturing.program;
    egui::Panel::left("manufacturing_browser").default_size(290.0).show(ui, |ui| {
        egui::ScrollArea::vertical().id_salt("cam_panel_scroll").show(ui, |ui| {
            ui.heading("Manufacture");
            ui.label("Three-axis milling · reference post");
            ui.horizontal(|ui| {
                if ui.button("Setup").clicked() {
                    action = Some("cam.setup.put");
                }
                if ui.button("Tool").clicked() {
                    action = Some("cam.tool.put");
                }
                if ui.button("Operation").clicked() {
                    action = Some("cam.operation.put");
                }
            });
            egui::ScrollArea::vertical().max_height(280.0).show(ui, |ui| {
                for setup in &model.setups {
                    ui.collapsing(&setup.name, |ui| {
                        if ui.small_button("Edit setup").clicked() {
                            app.manufacturing.editor = Some(Editor::Setup(setup.clone()));
                        }
                        for o in model.operations.iter().filter(|o| o.setup == setup.id) {
                            if ui.selectable_label(false, &o.name).clicked() {
                                app.manufacturing.editor = Some(Editor::Operation(o.clone()));
                            }
                        }
                    });
                }
                ui.collapsing("Tools", |ui| {
                    for tool in &model.tools {
                        if ui.button(format!("T{} · {}", tool.number, tool.name)).clicked() {
                            app.manufacturing.editor = Some(Editor::Tool(tool.clone()));
                        }
                    }
                });
                ui.collapsing("NC programs", |ui| {
                    for p in &model.programs {
                        ui.horizontal(|ui| {
                            ui.selectable_value(&mut app.manufacturing.program, p.id, &p.name);
                            if ui.small_button("Edit").clicked() {
                                app.manufacturing.editor = Some(Editor::Program(p.clone()));
                            }
                        });
                    }
                    if ui.button("New program").clicked() {
                        action = Some("cam.program.put");
                    }
                });
            });
            ui.separator();
            ui.horizontal(|ui| {
                if ui.button("Generate").clicked() {
                    action = Some("cam.generate");
                }
                if ui.button("Simulate").clicked() {
                    action = Some("cam.simulate");
                }
            });
            if let Some(id) = app.manufacturing.job {
                if let Ok(job) = solidcraft_engine::manufacturing::job(&app.session, id) {
                    if solidcraft_engine::manufacturing::current(&app.session, &job).is_err() {
                        ui.colored_label(Tokens::get().warning, "Inputs changed — regenerate");
                    }
                    match job.state() {
                        Ok(JobState::Verifying(_)) => {
                            ui.spinner();
                            ui.label("Checking tool clearances and remaining stock…");
                            ui.ctx().request_repaint();
                        }
                        Ok(JobState::Preparing) => {
                            ui.spinner();
                            ui.label("Preparing machining geometry…");
                            ui.ctx().request_repaint();
                        }
                        Ok(JobState::Running) => {
                            ui.spinner();
                            ui.label(format!("Generating {}%", job.progress.load(std::sync::atomic::Ordering::Relaxed)));
                            ui.ctx().request_repaint();
                        }
                        Ok(JobState::Failed(e)) => {
                            ui.colored_label(Tokens::get().error, e);
                        }
                        Ok(JobState::Cancelled) => {
                            ui.label("Cancelled");
                        }
                        Ok(JobState::Generated(path)) => {
                            ui.label(format!("{} motion and machine events", path.blocks.len()));
                        }
                        Ok(JobState::Verified(_, r)) => {
                            ui.label(format!("Path checks: {:?}", r.status));
                            ui.label(format!("Finished dimensions: {:?}", r.finished_dimensions));
                            ui.label(format!("Cutting: {:.1} mm · {:.2} min", r.cutting_distance, r.cutting_minutes));
                            egui::ScrollArea::vertical().max_height(120.0).show(ui, |ui| {
                                for (index, f) in r.findings.iter().enumerate() {
                                    ui.colored_label(view::severity_color(f.severity), format!("{}: {}", f.kind.replace('_', " "), f.detail));
                                    if let Some(source) = f.source {
                                        let name = job
                                            .input()
                                            .ok()
                                            .and_then(|input| input.operations.iter().find(|o| o.id == source.operation))
                                            .map(|o| o.name.as_str())
                                            .unwrap_or("Unknown operation");
                                        ui.label(format!("{name} · motion {} · NC line {}", source.block, source.line));
                                    } else if let Some(line) = f.line {
                                        ui.label(format!("NC line {line} · operation unmapped"));
                                    }
                                    if ui.small_button(format!("Locate issue {}", index + 1)).clicked() {
                                        locate_finding(app, id, index);
                                    }
                                    ui.separator();
                                }
                            });
                        }
                        Err(e) => {
                            ui.label(e.to_string());
                        }
                    }
                    if app.manufacturing.finding.is_some() && ui.small_button("Clear issue marker").clicked() {
                        app.manufacturing.finding = None;
                    }
                    if app.manufacturing.pending_export == Some(id) {
                        ui.spinner();
                        ui.label("Preparing export…");
                    }
                    if ui.button("Cancel job").clicked() {
                        let _ = app.run("cam.job.cancel", json!({"job":id}));
                    }
                    let count = match job.state() {
                        Ok(JobState::Generated(p) | JobState::Verified(p, _)) => p.blocks.len(),
                        _ => 0,
                    };
                    if count > 0 {
                        view::advance(&mut app.manufacturing, count, 0.);
                        if ui.checkbox(&mut app.manufacturing.playing, "Play toolpath").changed()
                            && app.manufacturing.playing
                            && app.manufacturing.playback == count
                        {
                            app.manufacturing.playback = 0;
                            app.manufacturing.view.fraction = 0.;
                        }
                        ui.horizontal(|ui| {
                            if ui.small_button("To start").clicked() {
                                app.manufacturing.playback = 0;
                                app.manufacturing.playing = false;
                            }
                            if ui.small_button("To end").clicked() {
                                app.manufacturing.playback = count;
                                app.manufacturing.playing = false;
                            }
                        });
                        if ui.add(egui::Slider::new(&mut app.manufacturing.playback, 0..=count).text("Motion")).changed() {
                            app.manufacturing.playing = false;
                            app.manufacturing.view.fraction = 0.;
                        }
                        ui.add(egui::Slider::new(&mut app.manufacturing.view.speed, 1.0..=300.0).text("Events / second"));
                        view::advance(&mut app.manufacturing, count, ui.input(|i| f64::from(i.stable_dt)));
                        if app.manufacturing.playing {
                            ui.ctx().request_repaint();
                        }
                        if let (Ok(JobState::Generated(path) | JobState::Verified(path, _)), Ok(input)) = (job.state(), job.input())
                            && let Ok(state) = cam::playback::at(&path, &input.setup, app.manufacturing.playback)
                        {
                            if let Some(o) = input.operations.iter().find(|o| Some(o.id) == state.operation) {
                                ui.label(format!("Operation: {}", o.name));
                            }
                            ui.label(format!("X {:.3}  Y {:.3}  Z {:.3}", state.position.x, state.position.y, state.position.z));
                            if let Some(t) = input.tools.iter().find(|t| Some(t.id) == state.tool) {
                                ui.label(format!("T{} · {} · {:.0} rpm", t.number, t.name, state.spindle));
                            }
                        }
                    }
                } else {
                    ui.label("Regenerate after reopening or switching documents");
                }
            }
            ui.separator();
            egui::ComboBox::from_label("Workpiece")
                .selected_text(match app.manufacturing.view.workpiece {
                    view::Workpiece::Model => "Target model",
                    view::Workpiece::InitialStock => "Initial stock",
                    view::Workpiece::RemainingStock => "Remaining stock",
                })
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut app.manufacturing.view.workpiece, view::Workpiece::Model, "Target model");
                    ui.selectable_value(&mut app.manufacturing.view.workpiece, view::Workpiece::InitialStock, "Initial stock");
                    ui.selectable_value(&mut app.manufacturing.view.workpiece, view::Workpiece::RemainingStock, "Remaining stock");
                });
            if app.manufacturing.view.workpiece == view::Workpiece::RemainingStock {
                match view::fresh_job(app).map(|j| j.stock_mesh()) {
                    Some(Ok(Some(_))) => {
                        ui.label("Final stock · conservative upper surface");
                    }
                    Some(Err(e)) => {
                        ui.colored_label(Tokens::get().warning, e.to_string());
                    }
                    _ => {
                        ui.label("Simulate the current program to display remaining stock");
                    }
                }
            }
            ui.checkbox(&mut app.manufacturing.view.fixtures, "Show fixtures");
            ui.checkbox(&mut app.manufacturing.view.paths, "Show toolpath");
            ui.checkbox(&mut app.manufacturing.view.boundary, "Show stock boundary");
            if ui.button("Fit machining setup").clicked() {
                app.fit_view();
            }
            ui.horizontal_wrapped(|ui| {
                let t = Tokens::get();
                ui.colored_label(t.cam_cut, "Cut");
                ui.colored_label(t.cam_rapid, "Rapid");
                ui.colored_label(t.cam_fixture, "Fixture");
            });
            ui.separator();
            for (label, f) in [
                ("Post G-code", "gcode"),
                ("Setup sheet", "setup_sheet"),
                ("Toolpath JSON", "toolpath"),
                ("Validation report", "validation"),
                ("Remaining stock STL", "stock"),
            ] {
                if ui.button(label).clicked() {
                    export(app, f);
                }
            }
        });
    });
    if previous_program != app.manufacturing.program {
        app.manufacturing.job = None;
        app.manufacturing.playing = false;
        app.manufacturing.playback = 0;
        app.manufacturing.view.loaded_job = None;
        app.manufacturing.finding = None;
    }
    if let Some(id) = action {
        start(app, id);
    }
}
fn number(ui: &mut egui::Ui, label: &str, v: &mut f64) {
    ui.horizontal(|ui| {
        ui.label(label);
        ui.add(egui::DragValue::new(v).speed(0.1).max_decimals(4));
    });
}
fn point(ui: &mut egui::Ui, label: &str, p: &mut Vec3) {
    ui.label(label);
    ui.horizontal(|ui| {
        for (axis, v) in [("X", &mut p.x), ("Y", &mut p.y), ("Z", &mut p.z)] {
            ui.label(axis);
            ui.add(egui::DragValue::new(v).speed(0.1));
        }
    });
}
fn boundary(ui: &mut egui::Ui, b: &mut cam::Boundary) {
    ui.label("Boundary in setup XY (mm)");
    for (i, ring) in std::iter::once(&mut b.outer).chain(&mut b.islands).enumerate() {
        ui.collapsing(if i == 0 { "Outer contour".into() } else { format!("Island {i}") }, |ui| {
            for p in ring.iter_mut() {
                ui.horizontal(|ui| {
                    ui.add(egui::DragValue::new(&mut p[0]));
                    ui.add(egui::DragValue::new(&mut p[1]));
                });
            }
            if ui.small_button("Add vertex").clicked() {
                ring.push([0.0, 0.0]);
            }
            if ring.len() > 3 && ui.small_button("Remove last vertex").clicked() {
                ring.pop();
            }
        });
    }
    if ui.small_button("Add island").clicked() {
        b.islands.push(vec![[5.0, 5.0], [7.0, 5.0], [7.0, 7.0], [5.0, 7.0]]);
    }
}
pub fn editor(app: &mut SolveApp, ctx: &egui::Context) {
    let Some(mut editor) = app.manufacturing.editor.take() else { return };
    let mut save = false;
    let mut delete = false;
    let mut open = true;
    crate::frame::window(ctx, "Manufacturing parameters", crate::frame::Width::Normal).open(&mut open).resizable(true).default_width(420.0).show(
        ctx,
        |ui| {
            egui::ScrollArea::vertical().max_height(540.0).show(ui, |ui| match &mut editor {
                Editor::Setup(s) => edit_setup(app, ui, s),
                Editor::Tool(t) => edit_tool(ui, t),
                Editor::Operation(o) => edit_operation(app, ui, o),
                Editor::Program(p) => edit_program(app, ui, p),
            });
            if !app.manufacturing.error.is_empty() {
                ui.colored_label(Tokens::get().error, &app.manufacturing.error);
            }
            ui.horizontal(|ui| {
                save = ui.button("Save").clicked();
                delete = ui.button("Delete item").clicked();
            });
        },
    );
    if save {
        let (id, p) = match &editor {
            Editor::Setup(s) => ("cam.setup.put", json!({"setup":s})),
            Editor::Tool(t) => ("cam.tool.put", json!({"tool":t})),
            Editor::Operation(o) => ("cam.operation.put", json!({"operation":o})),
            Editor::Program(p) => ("cam.program.put", json!({"program":p})),
        };
        match app.run(id, p) {
            Ok(_) => {
                open = false;
                app.manufacturing.error.clear();
            }
            Err(e) => app.manufacturing.error = e.to_string(),
        }
    }
    if delete {
        let (kind, id) = match &editor {
            Editor::Setup(s) => ("setup", s.id),
            Editor::Tool(t) => ("tool", t.id),
            Editor::Operation(o) => ("operation", o.id),
            Editor::Program(p) => ("program", p.id),
        };
        match app.run("cam.delete", json!({"kind":kind,"id":id})) {
            Ok(_) => {
                open = false;
                app.manufacturing.error.clear();
                if kind == "program" && app.manufacturing.program == id {
                    app.manufacturing.program = 0;
                    app.manufacturing.job = None;
                }
            }
            Err(e) => app.manufacturing.error = e,
        }
    }
    if open {
        app.manufacturing.editor = Some(editor);
    }
}
fn selected_bodies(app: &SolveApp) -> Vec<cam::BodyRef> {
    let mut refs = Vec::new();
    for sel in &app.session.selection {
        match sel {
            solidcraft_engine::Sel::Body { name } => refs.push(cam::BodyRef { body: name.clone(), occurrence: vec![] }),
            solidcraft_engine::Sel::Occurrence { path } => {
                if let Ok(p) = app.session.occurrence_placement(path) {
                    for b in p.bodies {
                        refs.push(cam::BodyRef { body: b.source, occurrence: path.clone() });
                    }
                }
            }
            _ => {}
        }
    }
    refs
}
fn edit_setup(app: &mut SolveApp, ui: &mut egui::Ui, s: &mut cam::Setup) {
    ui.label("Setup name");
    ui.text_edit_singleline(&mut s.name);
    point(ui, "Stock minimum (mm)", &mut s.stock.min);
    point(ui, "Stock maximum (mm)", &mut s.stock.max);
    point(ui, "Work origin (world mm)", &mut s.frame.origin);
    ui.collapsing("Setup orientation", |ui| {
        point(ui, "X axis", &mut s.frame.x);
        point(ui, "Y axis", &mut s.frame.y);
        point(ui, "Z axis", &mut s.frame.z);
    });
    number(ui, "Clearance Z", &mut s.clearance);
    number(ui, "Retract Z", &mut s.retract);
    ui.horizontal(|ui| {
        ui.label("Work offset G");
        ui.add(egui::DragValue::new(&mut s.work_offset).range(54..=59));
    });
    ui.label("Model bodies");
    for r in &mut s.models {
        ui.text_edit_singleline(&mut r.body);
    }
    if ui.button("Use selected bodies / occurrences").clicked() {
        let refs = selected_bodies(app);
        if !refs.is_empty() {
            s.models = refs;
        }
    }
    if ui.button("Add selected bodies as fixtures").clicked() {
        for reference in selected_bodies(app) {
            if !s.fixtures.contains(&reference) {
                s.fixtures.push(reference);
            }
        }
    }
    for f in &s.fixtures {
        ui.label(format!("Fixture: {}", f.body));
    }
    if ui.small_button("Clear fixtures").clicked() {
        s.fixtures.clear();
    }
}
fn edit_tool(ui: &mut egui::Ui, t: &mut cam::Tool) {
    ui.label("Tool name");
    ui.text_edit_singleline(&mut t.name);
    ui.horizontal(|ui| {
        ui.label("Tool number");
        ui.add(egui::DragValue::new(&mut t.number).range(1..=9999));
    });
    egui::ComboBox::from_label("Tool type").selected_text(format!("{:?}", t.kind)).show_ui(ui, |ui| {
        ui.selectable_value(&mut t.kind, cam::ToolKind::FlatEndMill, "Flat end mill (center cutting)");
        ui.selectable_value(&mut t.kind, cam::ToolKind::TwistDrill, "Twist drill");
    });
    for (label, v) in [
        ("Diameter", &mut t.diameter),
        ("Cutting length", &mut t.cutting_length),
        ("Shank diameter", &mut t.shank_diameter),
        ("Overall length", &mut t.overall_length),
        ("Holder diameter", &mut t.holder_diameter),
        ("Holder length", &mut t.holder_length),
    ] {
        number(ui, label, v);
    }
    if t.kind == cam::ToolKind::TwistDrill {
        let mut angle = t.tip_angle.to_degrees();
        number(ui, "Point angle (degrees)", &mut angle);
        t.tip_angle = angle.to_radians();
    }
}
fn edit_operation(app: &mut SolveApp, ui: &mut egui::Ui, o: &mut cam::Operation) {
    ui.label("Operation name");
    ui.text_edit_singleline(&mut o.name);
    egui::ComboBox::from_label("Setup").selected_text(o.setup.to_string()).show_ui(ui, |ui| {
        for s in &app.session.doc.manufacturing.setups {
            ui.selectable_value(&mut o.setup, s.id, &s.name);
        }
    });
    egui::ComboBox::from_label("Tool").selected_text(o.tool.to_string()).show_ui(ui, |ui| {
        for t in &app.session.doc.manufacturing.tools {
            ui.selectable_value(&mut o.tool, t.id, &t.name);
        }
    });
    let mut kind = match o.geometry {
        cam::Strategy::Face => 0,
        cam::Strategy::Contour { .. } => 1,
        cam::Strategy::Pocket { .. } => 2,
        cam::Strategy::Drill { .. } => 3,
    };
    let before = kind;
    egui::ComboBox::from_label("Strategy").selected_text(["Facing", "Contour", "Pocket", "Drilling"][kind]).show_ui(ui, |ui| {
        for (i, label) in ["Facing", "Contour", "Pocket", "Drilling"].into_iter().enumerate() {
            ui.selectable_value(&mut kind, i, label);
        }
    });
    if kind != before {
        o.source = None;
        o.hole_sources.clear();
        let b = cam::Boundary { outer: vec![[0.0, 0.0], [20.0, 0.0], [20.0, 20.0], [0.0, 20.0]], islands: vec![] };
        o.geometry = match kind {
            1 => cam::Strategy::Contour { boundary: b, inside: false },
            2 => cam::Strategy::Pocket { boundary: b },
            3 => cam::Strategy::Drill { holes: vec![[0.0, 0.0]], peck: None, full_diameter: false },
            _ => cam::Strategy::Face,
        };
    }
    if matches!(o.geometry, cam::Strategy::Contour { .. } | cam::Strategy::Pocket { .. }) {
        if ui.button("Use selected sketch profile").clicked() {
            link_profile(app, o);
        }
        if o.source.is_some() {
            ui.label("Boundary linked to sketch; regeneration follows edits");
            if ui.small_button("Unlink boundary").clicked() {
                o.source = None;
            }
        }
    }
    if matches!(o.geometry, cam::Strategy::Drill { .. }) {
        if ui.button("Use selected holes").on_hover_text("Select cylindrical hole faces or circular sketch profiles in drilling order").clicked() {
            link_holes(app, o);
        }
        if !o.hole_sources.is_empty() {
            ui.label(format!("{} linked holes; regeneration follows edits", o.hole_sources.len()));
            if ui.small_button("Unlink holes").clicked() {
                o.hole_sources.clear();
            }
        }
    }
    let linked_holes = !o.hole_sources.is_empty();
    match &mut o.geometry {
        cam::Strategy::Contour { boundary: b, inside } => {
            ui.checkbox(inside, "Inside contour");
            boundary(ui, b);
        }
        cam::Strategy::Pocket { boundary: b } => boundary(ui, b),
        cam::Strategy::Drill { holes, peck, full_diameter } => {
            ui.add_enabled_ui(!linked_holes, |ui| {
                for (index, p) in holes.iter_mut().enumerate() {
                    ui.horizontal(|ui| {
                        ui.label(format!("Hole {} XY", index + 1));
                        ui.add(egui::DragValue::new(&mut p[0]));
                        ui.add(egui::DragValue::new(&mut p[1]));
                    });
                }
                if ui.button("Add hole").clicked() {
                    holes.push([0.0, 0.0]);
                }
                if holes.len() > 1 && ui.button("Remove last hole").clicked() {
                    holes.pop();
                }
            });
            let mut enabled = peck.is_some();
            if ui.checkbox(&mut enabled, "Peck drilling").changed() {
                *peck = enabled.then_some(1.0);
            }
            if let Some(p) = peck {
                number(ui, "Peck depth", p);
            }
            ui.checkbox(full_diameter, "Depth is full-diameter depth");
        }
        _ => {}
    }
    for (label, v) in [
        ("Top Z", &mut o.top),
        ("Bottom Z", &mut o.bottom),
        ("Stepdown", &mut o.stepdown),
        ("Stepover", &mut o.stepover),
        ("Feed mm/min", &mut o.feed),
        ("Plunge mm/min", &mut o.plunge_feed),
        ("Spindle rpm", &mut o.spindle),
        ("Stock allowance", &mut o.allowance),
        ("Tolerance", &mut o.tolerance),
    ] {
        number(ui, label, v);
    }
    if matches!(o.geometry, cam::Strategy::Contour { .. } | cam::Strategy::Pocket { .. }) {
        number(ui, "Tangential lead distance", &mut o.lead_distance);
        let mut ramp = o.ramp_angle.is_some();
        if ui.checkbox(&mut ramp, "Ramp entry").changed() {
            o.ramp_angle = ramp.then_some(3_f64.to_radians());
        }
        if let Some(a) = &mut o.ramp_angle {
            let mut degrees = a.to_degrees();
            number(ui, "Ramp angle (degrees)", &mut degrees);
            *a = degrees.to_radians();
        }
    }
    ui.checkbox(&mut o.climb, "Climb milling");
    ui.checkbox(&mut o.coolant, "Coolant");
}
fn link_holes(app: &mut SolveApp, o: &mut cam::Operation) {
    let mut picks = Vec::new();
    for selected in &app.session.selection {
        match selected {
            solidcraft_engine::Sel::Face { body, index, .. } => picks.push(json!({"kind":"face","body":body,"index":index})),
            solidcraft_engine::Sel::Profile { sketch, index } => picks.push(json!({"kind":"profile","sketch":sketch,"index":index,
                "occurrence":app.session.active_occurrence_path.clone().unwrap_or_default()})),
            _ => {
                app.manufacturing.error = "Select only cylindrical hole faces or circular sketch profiles".into();
                return;
            }
        }
    }
    match app.run("cam.holes.from_selection", json!({"setup":o.setup,"selections":picks})) {
        Ok(value) => {
            let data = serde_json::from_value::<Vec<[f64; 2]>>(value["holes"].clone())
                .and_then(|holes| serde_json::from_value::<Vec<cam::HoleSource>>(value["sources"].clone()).map(|sources| (holes, sources)));
            match data {
                Ok((centers, sources)) => {
                    if let cam::Strategy::Drill { holes, .. } = &mut o.geometry {
                        *holes = centers;
                        o.hole_sources = sources;
                    }
                    app.manufacturing.error.clear();
                }
                Err(error) => app.manufacturing.error = error.to_string(),
            }
        }
        Err(error) => app.manufacturing.error = error.to_string(),
    }
}
fn link_profile(app: &mut SolveApp, o: &mut cam::Operation) {
    let selected = app.session.selection.iter().find_map(|s| match s {
        solidcraft_engine::Sel::Profile { sketch, index } => Some((*sketch, *index)),
        _ => None,
    });
    let Some((sketch, index)) = selected else {
        app.manufacturing.error = "Select a closed sketch profile first".into();
        return;
    };
    let result = app.run(
        "cam.boundary.from_profile",
        json!({"setup":o.setup,"sketch":sketch,
        "profile":index,"occurrence":app.session.active_occurrence_path.clone().unwrap_or_default()}),
    );
    match result {
        Ok(v) => {
            if let (Ok(b), Ok(source)) = (serde_json::from_value(v["boundary"].clone()), serde_json::from_value(v["source"].clone())) {
                match &mut o.geometry {
                    cam::Strategy::Contour { boundary, .. } | cam::Strategy::Pocket { boundary } => *boundary = b,
                    _ => {}
                }
                o.source = Some(source);
            }
        }
        Err(e) => app.manufacturing.error = e.to_string(),
    }
}
fn edit_program(app: &mut SolveApp, ui: &mut egui::Ui, p: &mut cam::Program) {
    ui.label("Program name");
    ui.text_edit_singleline(&mut p.name);
    egui::ComboBox::from_label("Setup").selected_text(p.setup.to_string()).show_ui(ui, |ui| {
        for s in &app.session.doc.manufacturing.setups {
            ui.selectable_value(&mut p.setup, s.id, &s.name);
        }
    });
    ui.label("Ordered operations");
    let mut swap = None;
    let mut remove = None;
    for (i, id) in p.operations.iter().enumerate() {
        ui.push_id(i, |ui| {
            ui.horizontal(|ui| {
                ui.label(
                    app.session.doc.manufacturing.operations.iter().find(|o| o.id == *id).map(|o| o.name.as_str()).unwrap_or("Missing operation"),
                );
                if i > 0 && ui.small_button("↑").clicked() {
                    swap = Some(i);
                }
                if ui.small_button("Remove").clicked() {
                    remove = Some(i);
                }
            });
        });
    }
    if let Some(i) = swap {
        p.operations.swap(i - 1, i);
    }
    if let Some(i) = remove {
        p.operations.remove(i);
    }
    ui.label("Add an operation");
    let available: Vec<_> =
        app.session.doc.manufacturing.operations.iter().filter(|o| o.setup == p.setup && !p.operations.contains(&o.id)).cloned().collect();
    for operation in available {
        if ui.button(format!("Add {}", operation.name)).clicked() {
            p.operations.push(operation.id);
        }
    }
    if ui.button("Use all operations in this setup").clicked() {
        p.operations = app.session.doc.manufacturing.operations.iter().filter(|o| o.setup == p.setup).map(|o| o.id).collect();
    }
}
fn locate_finding(app: &mut SolveApp, job: u64, index: usize) {
    match app.run("cam.inspect", json!({"job":job,"finding":index})) {
        Ok(result) => {
            if let Ok(point) = serde_json::from_value(result["diagnostic"]["world"].clone()) {
                app.cam.target = point;
                app.cam_anim = None;
            }
            if let Some(block) = result["diagnostic"]["playback"]["completed"].as_u64().and_then(|n| usize::try_from(n).ok()) {
                app.manufacturing.playback = block;
                app.manufacturing.view.loaded_job = Some(job);
            }
            app.manufacturing.playing = false;
            app.manufacturing.view.fraction = 0.;
            app.manufacturing.finding = Some(index);
            app.manufacturing.error.clear();
        }
        Err(error) => app.manufacturing.error = error,
    }
}
pub fn overlay(app: &SolveApp, painter: &egui::Painter, proj: &crate::viewport::Proj) {
    if app.ui.tab != "MILLING" {
        return;
    }
    let line = |a: Vec3, b: Vec3, color: Color32| {
        if let (Some(a), Some(b)) = (proj.to_screen(a), proj.to_screen(b)) {
            painter.line_segment([a, b], Stroke::new(1.2, color));
        }
    };
    if app.manufacturing.view.boundary
        && let Some(s) = view::setup(app)
    {
        let corners = view::corners(s.stock);
        for (i, j) in [(0, 1), (1, 2), (2, 3), (3, 0), (4, 5), (5, 6), (6, 7), (7, 4), (0, 4), (1, 5), (2, 6), (3, 7)] {
            line(s.frame.world(corners[i]), s.frame.world(corners[j]), Tokens::get().cam_stock);
        }
    }
    if let Some((at, color)) = view::finding_marker(app).and_then(|(point, color)| proj.to_screen(point).map(|point| (point, color))) {
        painter.circle_stroke(at, 8., Stroke::new(2., color));
        for delta in [egui::vec2(11., 0.), egui::vec2(0., 11.)] {
            painter.line_segment([at - delta, at + delta], Stroke::new(1.5, color));
        }
    }
    if !app.manufacturing.view.paths {
        return;
    }
    let Some(id) = app.manufacturing.job else {
        return;
    };
    let Ok(job) = solidcraft_engine::manufacturing::job(&app.session, id) else {
        return;
    };
    if solidcraft_engine::manufacturing::current(&app.session, &job).is_err() {
        return;
    }
    let path = match job.state() {
        Ok(JobState::Generated(p) | JobState::Verified(p, _)) => p,
        _ => return,
    };
    let Ok(input) = job.input() else { return };
    view::segments(&path, &input.setup, app.manufacturing.playback, |a, b, rapid| {
        line(a, b, if rapid { Tokens::get().cam_rapid } else { Tokens::get().cam_cut })
    });
    if let Ok(state) = cam::playback::at(&path, &input.setup, app.manufacturing.playback.min(path.blocks.len()))
        && let Some(p) = proj.to_screen(input.setup.frame.world(state.position))
    {
        painter.circle_filled(p, 5., Tokens::get().cam_tool);
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    #[test]
    fn manufacturing_dialogs_create_persistent_definitions() {
        let mut h = crate::scenario::Harness::new();
        h.app.run("solid.box", json!({"length":20,"width":20,"height":5})).unwrap();
        for command in ["cam.setup.put", "cam.tool.put", "cam.operation.put", "cam.program.put"] {
            h.step(&json!({"start":command})).unwrap();
            h.frames(2);
            h.step(&json!({"widget":"Save"})).unwrap();
            h.frames(2);
            assert!(h.app.manufacturing.editor.is_none(), "{}: {}", command, h.app.manufacturing.error);
        }
        let m = &h.app.session.doc.manufacturing;
        assert_eq!((m.setups.len(), m.tools.len(), m.operations.len(), m.programs.len()), (1, 1, 1, 1));
        let job = h.app.run("cam.generate", json!({"program":1})).unwrap()["job"].clone();
        let report = h.app.run("cam.simulate", json!({"job":job})).unwrap();
        assert_eq!(report["verification"]["status"], "pass", "{report}");
        assert!(h.app.run("cam.post", json!({"job":job})).is_ok());
        h.app.manufacturing.job = job.as_u64();
        h.frames(3);
        h.app.manufacturing.view.workpiece = view::Workpiece::RemainingStock;
        let stock = view::stock(&h.app).unwrap();
        assert!(!stock.triangles.is_empty());
        assert!((stock.bounds().max.z - 5.).abs() < 1e-6);
        let inspected = h.app.run("cam.inspect", json!({"job":job,"motion":0,"include_stock":true})).unwrap();
        assert_eq!(inspected["playback"]["completed"], 0);
        assert!(inspected["playback"]["tool"].is_null());
        assert!(!inspected["stock_mesh"]["triangles"].as_array().unwrap().is_empty());
        h.step(&json!({"widget":"To start"})).unwrap();
        h.frames(2);
        assert_eq!(h.app.manufacturing.playback, 0);
        let export_path = std::env::temp_dir().join(format!("solidcraft-cam-ui-export-{}.nc", std::process::id()));
        let picked = export_path.clone();
        h.app.services.pick_save = Some(Box::new(move |_, _| {
            let path = picked.to_string_lossy().into_owned();
            Box::pin(async move { Some(path) })
        }));
        h.step(&json!({"start":"cam.post"})).unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
        while h.app.file_picker_busy() || h.app.manufacturing.pending_export.is_some() {
            h.frames(1);
            assert!(std::time::Instant::now() < deadline, "UI export did not finish");
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        assert!(std::fs::read_to_string(&export_path).unwrap().contains("G21"));
        std::fs::remove_file(export_path).unwrap();
        h.app.run("solid.box", json!({"length":1,"width":1,"height":1})).unwrap();
        assert!(view::stock(&h.app).is_none());
        assert!(h.app.run("cam.inspect", json!({"job":job,"include_stock":true})).is_err());
    }
    #[test]
    fn hole_selection_dialog_links_saves_and_unlinks_ordered_profiles() {
        let mut h = crate::scenario::Harness::new();
        h.call("ui.resize", json!({"width":1500,"height":1200}));
        h.app.run("solid.box", json!({"length":20,"width":20,"height":6})).unwrap();
        h.app.run("sketch.create", json!({"plane":"XY"})).unwrap();
        h.app.run("sketch.circle.center", json!({"center":[5,6],"radius":0.5})).unwrap();
        h.app.run("sketch.finish", json!({})).unwrap();
        let state = h.app.session.model.state();
        let sketch = state.sketches.last().unwrap().feature;
        let mut input = cam::corpus::fixture("drill").unwrap();
        input.setup.models[0].body = state.bodies[0].name.clone();
        h.app.run("cam.setup.put", json!({"setup":input.setup})).unwrap();
        h.app.run("cam.tool.put", json!({"tool":input.tools[0]})).unwrap();
        h.step(&json!({"start":"cam.operation.put"})).unwrap();
        h.app.session.selection = vec![solidcraft_engine::Sel::Profile { sketch, index: 0 }];
        h.app.manufacturing.editor = Some(Editor::Operation(input.operations.remove(0)));
        h.app.ui.tab = "MILLING".into();
        // Changing strategy resizes this centered dialog; wait for its layout animation.
        h.frames(20);
        h.step(&json!({"widget":"Use selected holes"})).unwrap();
        h.frames(2);
        assert!(h.app.manufacturing.error.is_empty(), "{}", h.app.manufacturing.error);
        assert!(matches!(&h.app.manufacturing.editor,Some(Editor::Operation(o)) if o.hole_sources.len()==1));
        h.step(&json!({"widget":"Save"})).unwrap();
        let operation = h.app.session.doc.manufacturing.operations[0].clone();
        assert_eq!(operation.hole_sources.len(), 1);
        assert!(matches!(&operation.geometry,cam::Strategy::Drill {holes,..} if holes==&vec![[5.,6.]]));
        h.app.manufacturing.editor = Some(Editor::Operation(operation));
        h.frames(20);
        h.step(&json!({"widget":"Unlink holes"})).unwrap();
        h.step(&json!({"widget":"Save"})).unwrap();
        assert!(h.app.session.doc.manufacturing.operations[0].hole_sources.is_empty());
    }
    #[test]
    fn diagnostic_location_uses_inspection_and_hides_after_edit() {
        let mut h = crate::scenario::Harness::new();
        h.app.run("solid.box", json!({"length":20,"width":20,"height":5})).unwrap();
        for command in ["cam.setup.put", "cam.tool.put", "cam.operation.put", "cam.program.put"] {
            h.step(&json!({"start":command})).unwrap();
            h.step(&json!({"widget":"Save"})).unwrap();
        }
        let mut operation = h.app.session.doc.manufacturing.operations[0].clone();
        operation.bottom = 4.;
        h.app.run("cam.operation.put", json!({"operation":operation})).unwrap();
        let id = h.app.run("cam.generate", json!({"program":1})).unwrap()["job"].as_u64().unwrap();
        let report = h.app.run("cam.simulate", json!({"job":id})).unwrap();
        assert_eq!(report["verification"]["status"], "fail");
        h.app.manufacturing.job = Some(id);
        h.frames(3);
        let findings = report["verification"]["findings"].as_array().unwrap();
        let index = findings.iter().position(|f| f["source"].is_object()).unwrap();
        // The first issue is visible in the nested findings scroll area.
        assert_eq!(index, 0);
        h.step(&json!({"widget":"Locate issue 1"})).unwrap();
        assert_eq!(h.app.manufacturing.finding, Some(0));
        let block = findings[0]["source"]["block"].as_u64().unwrap() as usize;
        assert_eq!(h.app.manufacturing.playback, block);
        assert!(!h.app.manufacturing.playing);
        assert_eq!(view::finding_marker(&h.app).map(|(point, _)| point), Some(h.app.cam.target));
        h.app.manufacturing.view.paths = false;
        let mut scene = solidcraft_engine::view::scene(&h.app.session, &h.app.cam);
        view::cpu(&h.app, &mut scene);
        view::cpu_overlays(&h.app, &mut scene);
        let color = Tokens::get().error;
        assert!(scene.lines.iter().any(|line| line.color == solidcraft_engine::render::Rgb(color.r(), color.g(), color.b())));
        h.app.run("solid.box", json!({"length":1,"width":1,"height":1})).unwrap();
        assert!(view::finding_marker(&h.app).map(|(point, _)| point).is_none());
        let previous = h.app.cam.target;
        locate_finding(&mut h.app, id, 0);
        assert_eq!(h.app.cam.target, previous);
        assert!(!h.app.manufacturing.error.is_empty());
    }
}
