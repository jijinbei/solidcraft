#![allow(clippy::unwrap_used)]
use super::*;
use crate::geom::Vec3;
fn run(s: &mut Session, cmd: &str, value: Value) -> Value {
    s.execute(cmd, &value).unwrap()
}
fn linked_profiles() -> (Session, u64) {
    let mut s = Session::default();
    run(&mut s, "solid.box", json!({"length":20,"width":20,"height":6}));
    run(&mut s, "sketch.create", json!({"plane":"XY"}));
    run(&mut s, "sketch.circle.center", json!({"center":[4,5],"radius":0.5,"id":"a"}));
    run(&mut s, "sketch.circle.center", json!({"center":[12,15],"radius":0.5,"id":"b"}));
    run(&mut s, "sketch.finish", json!({}));
    let state = s.model.state();
    let sketch = state.sketches.last().unwrap();
    let id = sketch.feature;
    let a = sketch.profiles.iter().position(|p| p.curve_ids().contains(&"a".to_string())).unwrap();
    let b = sketch.profiles.iter().position(|p| p.curve_ids().contains(&"b".to_string())).unwrap();
    let mut input = cam::corpus::fixture("drill").unwrap();
    input.setup.models[0].body = state.bodies[0].name.clone();
    input.setup.frame.origin = Vec3::new(100., -50., 0.);
    input.setup.stock.min -= input.setup.frame.origin;
    input.setup.stock.max -= input.setup.frame.origin;
    run(&mut s, "cam.setup.put", json!({"setup":input.setup}));
    let selected = run(
        &mut s,
        "cam.holes.from_selection",
        json!({"setup":1,"selections":[
            {"kind":"profile","sketch":id,"index":b},{"kind":"profile","sketch":id,"index":a}
        ]}),
    );
    let holes: Vec<[f64; 2]> = serde_json::from_value(selected["holes"].clone()).unwrap();
    assert_eq!(holes, vec![[-88., 65.], [-96., 55.]]);
    input.operations[0].hole_sources = serde_json::from_value(selected["sources"].clone()).unwrap();
    input.operations[0].geometry = cam::Strategy::Drill { holes, peck: Some(0.3), full_diameter: false };
    run(&mut s, "cam.tool.put", json!({"tool":input.tools[0]}));
    run(&mut s, "cam.operation.put", json!({"operation":input.operations[0]}));
    run(&mut s, "cam.program.put", json!({"program":{"id":1,"name":"Selected holes","setup":1,"operations":[1]}}));
    (s, id)
}
fn centers(s: &mut Session) -> (u64, Vec<[f64; 2]>) {
    let generated = run(s, "cam.generate", json!({"program":1}));
    assert_eq!(generated["state"], "generated", "{generated}");
    let id = generated["job"].as_u64().unwrap();
    let job = jobs::job(s, id).unwrap();
    let op = &job.input().unwrap().operations[0];
    let holes = match &op.geometry {
        cam::Strategy::Drill { holes, .. } => holes.clone(),
        _ => vec![],
    };
    (id, holes)
}
#[test]
fn hole_selection_follows_profile_edits_frames_undo_and_reopen() {
    let (mut s, sketch) = linked_profiles();
    let (old, before) = centers(&mut s);
    let saved = crate::io::write_design(&s.doc);
    let mut reopened = Session::new(crate::io::read_design(&saved).unwrap());
    assert_eq!(centers(&mut reopened).1, before);
    run(&mut s, "sketch.edit", json!({"sketch":sketch}));
    run(&mut s, "sketch.move", json!({"entities":["a"],"translate":[2,3]}));
    assert_eq!(run(&mut s, "cam.job.status", json!({"job":old}))["stale"], true);
    assert!(s.execute("cam.post", &json!({"job":old})).is_err());
    assert_eq!(centers(&mut s).1, vec![[-88., 65.], [-94., 58.]]);
    run(&mut s, "edit.undo", json!({}));
    assert_eq!(centers(&mut s).1, before);
    run(&mut s, "edit.redo", json!({}));
    assert_eq!(centers(&mut s).1, vec![[-88., 65.], [-94., 58.]]);
    // Unassociated drilling preserves manually authored coordinates on reopen.
    let mut legacy: Value = serde_json::from_slice(&crate::io::write_design(&s.doc)).unwrap();
    legacy["manufacturing"]["operations"][0].as_object_mut().unwrap().remove("hole_sources");
    let doc = crate::io::read_design(legacy.to_string().as_bytes()).unwrap();
    assert!(doc.manufacturing.operations[0].hole_sources.is_empty());
    assert_eq!(doc.manufacturing.operations[0].geometry, s.doc.manufacturing.operations[0].geometry);
}
#[test]
fn hole_selection_rejects_missing_duplicate_tilted_and_wrong_size_sources() {
    let (mut s, sketch) = linked_profiles();
    let pick = json!({"kind":"profile","sketch":sketch,"index":0});
    for picks in [json!([]), json!([pick.clone(), pick.clone()]), json!([{"kind":"profile","sketch":sketch,"index":999}])] {
        assert!(s.execute("cam.holes.from_selection", &json!({"setup":1,"selections":picks})).is_err());
    }
    let excessive = vec![pick.clone(); cam::MAX_ITEMS + 1];
    assert!(s.execute("cam.holes.from_selection", &json!({"setup":1,"selections":excessive})).is_err());
    let mut invalid = s.doc.manufacturing.operations[0].clone();
    invalid.hole_sources.pop();
    assert!(s.execute("cam.operation.put", &json!({"operation":invalid})).is_err());
    let mut invalid = s.doc.manufacturing.operations[0].clone();
    invalid.hole_sources[1] = invalid.hole_sources[0].clone();
    assert!(s.execute("cam.operation.put", &json!({"operation":invalid})).is_err());
    let mut setup = s.doc.manufacturing.setups[0].clone();
    setup.frame = cam::Frame { origin: Vec3::ZERO, x: Vec3::X, y: Vec3::Z, z: -Vec3::Y };
    run(&mut s, "cam.setup.put", json!({"setup":setup}));
    assert!(s.execute("cam.holes.from_selection", &json!({"setup":1,"selections":[pick]})).is_err());
    run(&mut s, "edit.undo", json!({}));
    let mut tool = s.doc.manufacturing.tools[0].clone();
    tool.diameter = 0.8;
    run(&mut s, "cam.tool.put", json!({"tool":tool}));
    let failed = run(&mut s, "cam.generate", json!({"program":1}));
    assert_eq!(failed["state"], "failed");
    assert!(failed["error"].as_str().unwrap().contains("diameter"));
    assert!(s.execute("cam.post", &json!({"job":failed["job"]})).is_err());
    run(&mut s, "edit.undo", json!({}));
    let mut op = s.doc.manufacturing.operations[0].clone();
    if let cam::HoleSource::Profile { profile } = &mut op.hole_sources[0] {
        profile.curves = vec!["missing".into()];
    }
    run(&mut s, "cam.operation.put", json!({"operation":op}));
    assert!(s.execute("cam.generate", &json!({"program":1})).unwrap_err().to_string().contains("reselect"));
}

#[test]
fn hole_selection_keeps_exact_occurrence_and_rejects_lost_faces() {
    let mut s = Session::default();
    let component = run(&mut s, "component.create", json!({"name":"Drilled part"}));
    run(&mut s, "solid.box", json!({"length":20,"width":20,"height":6}));
    run(&mut s, "solid.hole", json!({"position":[10,10,6],"direction":[0,0,-1],"diameter":1,"depth":1,"type":"drilled","tip_angle":118}));
    run(&mut s, "component.activate", json!({"component":0}));
    let second = run(&mut s, "occurrence.copy", json!({"component":component["component"],"translate":[30,0,0]}))["occurrence"].as_u64().unwrap();
    let body = s.model.state().bodies[0].clone();
    let face = (0..body.body.face_count()).find(|i| crate::kernel::hole_axis(&body.body, *i, 0.0005).is_ok()).unwrap();
    let mut input = cam::corpus::fixture("drill").unwrap();
    input.setup.models = vec![cam::BodyRef { body: body.name.clone(), occurrence: vec![second] }];
    input.setup.frame.origin = Vec3::new(20., 0., 0.);
    input.setup.stock.min.x += 10.;
    input.setup.stock.max.x += 20.;
    run(&mut s, "cam.setup.put", json!({"setup":input.setup}));
    let selected = run(
        &mut s,
        "cam.holes.from_selection",
        json!({"setup":1,"selections":[{"kind":"face","body":body.name,"index":face,"occurrence":[second]}]}),
    );
    // Viewport labels resolve through the same setup occurrence as explicit command picks.
    let state = s.model.state();
    let placed = s.placed_document();
    let label = placed.world_body_name(&state, &body.name, placed.model_body_component(&body), &[second]);
    let from_viewport = run(&mut s, "cam.holes.from_selection", json!({"setup":1,"selections":[{"kind":"face","body":label,"index":face}]}));
    assert_eq!(from_viewport, selected);
    assert!(crate::kernel::hole_axis(&body.body, face, f64::NAN).is_err());
    assert!(crate::kernel::hole_axis(&body.body, usize::MAX, 0.0005).is_err());
    let plane = (0..body.body.face_count())
        .find(|i| matches!(crate::kernel::face_surface(&body.body, *i), Some(crate::kernel::FaceSurface::Plane { .. })))
        .unwrap();
    assert!(
        s.execute(
            "cam.holes.from_selection",
            &json!({"setup":1,"selections":[{"kind":"face","body":body.name,"index":plane,"occurrence":[second]}]})
        )
        .is_err()
    );
    input.operations[0].hole_sources = serde_json::from_value(selected["sources"].clone()).unwrap();
    run(&mut s, "cam.tool.put", json!({"tool":input.tools[0]}));
    run(&mut s, "cam.operation.put", json!({"operation":input.operations[0]}));
    run(&mut s, "cam.program.put", json!({"program":{"id":1,"name":"Placed holes","setup":1,"operations":[1]}}));
    let (job, before) = centers(&mut s);
    assert!((before[0][0] - 20.).abs() < 1e-8 && (before[0][1] - 10.).abs() < 1e-8);
    run(&mut s, "occurrence.move", json!({"occurrence":second,"translate":[5,0,0],"capture":false}));
    assert_eq!(run(&mut s, "cam.job.status", json!({"job":job}))["stale"], true);
    let moved = centers(&mut s).1;
    assert!((moved[0][0] - 25.).abs() < 1e-8);
    let mut op = s.doc.manufacturing.operations[0].clone();
    if let cam::HoleSource::Face { name, .. } = &mut op.hole_sources[0] {
        *name = "missing:face".into();
    }
    run(&mut s, "cam.operation.put", json!({"operation":op}));
    let failed = run(&mut s, "cam.generate", json!({"program":1}));
    assert_eq!(failed["state"], "failed");
    assert!(failed["error"].as_str().unwrap().contains("reselect"));
    assert!(s.execute("cam.post", &json!({"job":failed["job"]})).is_err());
}

#[test]
fn hole_selection_combines_seam_faces_but_rejects_exterior_cylinders() {
    let mut s = Session::default();
    s.run_script(&crate::sample::script()).unwrap();
    let body = s.model.state().bodies[0].clone();
    let mut setup = cam::corpus::fixture("drill").unwrap().setup;
    setup.models[0].body = body.name.clone();
    run(&mut s, "cam.setup.put", json!({"setup":setup}));
    let mut picks = vec![];
    for index in 0..body.body.face_count() {
        if let Ok((origin, _, _)) = crate::kernel::hole_axis(&body.body, index, 0.0005) {
            if (origin.x - 68.).abs() < 1e-8 {
                picks.push(json!({"kind":"face","body":body.name,"index":index}));
            }
        } else if matches!(crate::kernel::face_surface(&body.body, index), Some(crate::kernel::FaceSurface::Cylinder { .. })) {
            assert!(
                s.execute("cam.holes.from_selection", &json!({"setup":1,"selections":[{"kind":"face","body":body.name,"index":index}]})).is_err()
            );
        }
    }
    assert!(picks.len() >= 2, "the sample must exercise multiple seam faces on one drilled wall");
    let mut duplicate = picks.clone();
    duplicate.push(picks[1].clone());
    assert!(s.execute("cam.holes.from_selection", &json!({"setup":1,"selections":duplicate})).is_err());
    let result = run(&mut s, "cam.holes.from_selection", json!({"setup":1,"selections":picks}));
    let centers: Vec<[f64; 2]> = serde_json::from_value(result["holes"].clone()).unwrap();
    assert_eq!(centers.len(), 1);
    assert!((centers[0][0] - 68.).abs() < 1e-8 && (centers[0][1] - 25.).abs() < 1e-8);
    assert_eq!(result["sources"].as_array().unwrap().len(), 1);
}
