//! Original native CAD cases driven through the shared UI command and widget paths.
//! Headless frames exercise layout/input; GPU snapshots exercise the stock display.
use serde_json::json;
use solidcraft_engine::cam;
use solidcraft_ui_egui::scenario::Harness;
#[test]
fn four_strategies_generate_simulate_post_and_display_through_ui() {
    solidcraft_engine::capture::install(std::sync::Arc::new(|scene, camera, width, height| {
        let (rgba, renderer) = solidcraft_gpu::capture::headless_rgba(scene, camera, width, height)?;
        Ok(solidcraft_engine::capture::Picture { rgba, renderer })
    }));
    for name in ["face", "contour", "pocket", "drill"] {
        let mut h = Harness::new();
        h.call("ui.resize", json!({"width":1500,"height":1200}));
        h.app.run("solid.box", json!({"length":20,"width":20,"height":if name=="face" {5} else {6}})).unwrap();
        if name != "face" {
            h.app.run("solid.hole",json!({"position":[10,10,6],"direction":[0,0,-1],"diameter":if name=="drill" {1} else {16},"depth":1,"type":if name=="drill" {"drilled"} else {"simple"},"tip_angle":118})).unwrap();
        }
        let mut input = cam::corpus::fixture(name).unwrap();
        input.setup.models[0].body = h.app.session.model.state().bodies[0].name.clone();
        h.app.run("cam.setup.put", json!({"setup":input.setup})).unwrap();
        if name == "contour" || name == "pocket" {
            h.app.run("sketch.create", json!({"plane":"XY","name":"Pocket boundary"})).unwrap();
            h.app.run("sketch.circle.center", json!({"center":[10,10],"radius":8,"id":"wall"})).unwrap();
            h.app.run("sketch.finish", json!({})).unwrap();
            let sketch = h.app.session.model.state().sketches.last().unwrap().feature;
            let b = h.app.run("cam.boundary.from_profile", json!({"setup":1,"sketch":sketch,"profile":0})).unwrap();
            input.operations[0].source = Some(serde_json::from_value(b["source"].clone()).unwrap());
            match &mut input.operations[0].geometry {
                cam::Strategy::Contour { boundary, .. } | cam::Strategy::Pocket { boundary } => {
                    *boundary = serde_json::from_value(b["boundary"].clone()).unwrap()
                }
                _ => {}
            }
            input.operations[0].ramp_angle = Some(5_f64.to_radians());
        }
        if name == "drill" {
            input.operations[0].geometry = cam::Strategy::Drill { holes: vec![[10., 10.]], peck: Some(0.3), full_diameter: true };
        }
        h.app.run("cam.tool.put", json!({"tool":input.tools[0]})).unwrap();
        h.app.run("cam.operation.put", json!({"operation":input.operations[0]})).unwrap();
        h.app.run("cam.program.put", json!({"program":{"id":1,"name":name,"setup":1,"operations":[1]}})).unwrap();
        h.call("ui.set", json!({"tab":"MILLING","showGrid":false,"showOrigin":false,"showSketches":false}));
        if name == "drill" {
            let body = h.app.session.model.state().bodies[0].clone();
            let face = (0..body.body.face_count()).find(|i| solidcraft_engine::kernel::hole_axis(&body.body, *i, 0.0005).is_ok()).unwrap();
            h.app.session.selection =
                vec![solidcraft_engine::Sel::Face { body: body.name, index: face, point: solidcraft_engine::geom::Vec3::new(10.5, 10., 5.5) }];
            h.frames(3);
            h.step(&json!({"widget":"Analytic plate"})).unwrap();
            h.step(&json!({"widget":"drill"})).unwrap();
            h.frames(20);
            h.step(&json!({"widget":"Use selected holes"})).unwrap();
            h.step(&json!({"widget":"Save"})).unwrap();
            assert_eq!(h.app.session.doc.manufacturing.operations[0].hole_sources.len(), 1);
        }
        h.step(&json!({"start":"cam.generate"})).unwrap();
        let id = h.app.manufacturing.job.unwrap();
        wait(&mut h, id, "generated");
        h.step(&json!({"start":"cam.simulate"})).unwrap();
        let report = wait(&mut h, id, "verified");
        assert_eq!(report["verification"]["status"], "pass", "{name}: {report}");
        assert_eq!(report["verification"]["finished_dimensions"], "pass", "{name}: {report}");
        h.app.run("cam.post", json!({"job":id,"path":h.dir.join(format!("{name}.nc"))})).unwrap();
        h.step(&json!({"widget":"Target model"})).unwrap();
        h.step(&json!({"widget":"Remaining stock"})).unwrap();
        h.step(&json!({"widget":"To start"})).unwrap();
        h.call("ui.view", json!({"view":"fit"}));
        let out = h.call("ui.render", json!({"path":h.dir.join(format!("{name}.png")),"width":1000,"height":750}));
        assert_eq!(out["ok"], true, "{out}");
        let state = h.call("ui.inspect", json!({}));
        assert_eq!(state["result"]["manufacturing"]["completed_motions"], 0);
        assert_eq!(state["result"]["manufacturing"]["workpiece"], "RemainingStock", "{name}: {state}");
        assert!(std::fs::read(h.dir.join(format!("{name}.png"))).unwrap().starts_with(b"\x89PNG"));
        // Optional public-safe review evidence from these original fixtures only.
        if let Some(directory) = std::env::var_os("CAM_UI_EVIDENCE") {
            let directory = std::path::PathBuf::from(directory);
            std::fs::create_dir_all(&directory).unwrap();
            std::fs::copy(h.dir.join(format!("{name}.png")), directory.join(format!("{name}.png"))).unwrap();
        }
    }
}
fn wait(h: &mut Harness, id: u64, want: &str) -> serde_json::Value {
    let start = std::time::Instant::now();
    loop {
        h.frames(1);
        let state = h.app.run("cam.job.status", json!({"job":id})).unwrap();
        let s = state["state"].as_str().unwrap();
        if s == want {
            // The worker may finish between the last frame and the status read.
            // Lay out the completed-job controls before resolving widget coordinates.
            h.frames(3);
            return state;
        }
        assert!(s != "failed" && s != "cancelled", "{state}");
        assert!(start.elapsed().as_secs() < 180, "{state}");
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
}
