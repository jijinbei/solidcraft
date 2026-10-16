//! Native file actions must share the toolbar flow while JSON calls remain dialog-free.
#![allow(clippy::unwrap_used)]
use crate::scenario::Harness;
use serde_json::json;
use std::{cell::Cell, rc::Rc};

fn manufacturing(h: &mut Harness) {
    let mut input = solidcraft_engine::cam::corpus::fixture("face").unwrap();
    h.app.run("solid.box", json!({"length":20,"width":20,"height":5})).unwrap();
    input.setup.models[0].body = h.app.session.model.state().bodies[0].name.clone();
    h.app.run("cam.setup.put", json!({"setup":input.setup})).unwrap();
    h.app.run("cam.tool.put", json!({"tool":input.tools[0]})).unwrap();
    h.app.run("cam.operation.put", json!({"operation":input.operations[0]})).unwrap();
    h.app.run("cam.program.put", json!({"program":{"id":1,"name":"Facing","setup":1,"operations":[1]}})).unwrap();
}

#[test]
fn interactive_save_and_save_as_preserve_cam_definitions_and_cancel_cleanly() {
    let mut h = Harness::new();
    manufacturing(&mut h);
    let before = serde_json::to_value(&h.app.session.doc.manufacturing).unwrap();
    let first = h.dir.join("first.solidcraft");
    let second = h.dir.join("second.solidcraft");
    let picked = first.clone();
    h.app.services.pick_save = Some(Box::new(move |_, _| {
        let path = picked.to_string_lossy().into_owned();
        Box::pin(async move { Some(path) })
    }));
    // Programmatic commands must never invoke the picker.
    assert!(h.app.run("file.save", json!({})).is_err());
    assert!(!first.exists());
    h.app.start("file.save");
    h.frames(2);
    assert!(first.exists());
    assert!(!h.app.session.is_dirty());
    h.app.run("solid.box", json!({"length":1,"width":1,"height":1})).unwrap();
    h.app.services.pick_save = Some(Box::new(|_, _| Box::pin(async { None })));
    h.app.start("file.save_as");
    h.frames(2);
    assert!(h.app.session.is_dirty());
    assert_eq!(h.app.session.path.as_deref(), first.to_str());
    let picked = second.clone();
    h.app.services.pick_save = Some(Box::new(move |_, _| {
        let path = picked.to_string_lossy().into_owned();
        Box::pin(async move { Some(path) })
    }));
    h.app.start("file.save_as");
    h.frames(2);
    let reopened = solidcraft_engine::io::read_design(&std::fs::read(&second).unwrap()).unwrap();
    assert_eq!(serde_json::to_value(&reopened.manufacturing).unwrap(), before);
    assert_eq!(reopened.features.len(), 2);
    assert!(!h.app.session.is_dirty());
}

#[test]
fn quick_save_uses_key_event_modifiers_after_same_frame_release() {
    for modifiers in [egui::Modifiers { command: true, mac_cmd: true, ..Default::default() }, egui::Modifiers::CTRL] {
        let mut h = Harness::new();
        h.app.home.open = false;
        let calls = Rc::new(Cell::new(0));
        let count = calls.clone();
        h.app.services.pick_save = Some(Box::new(move |_, _| {
            count.set(count.get() + 1);
            Box::pin(async { None })
        }));
        let modifiers = egui::Modifiers { command: true, ..modifiers };
        let raw = egui::RawInput {
            focused: true,
            events: vec![
                egui::Event::ModifiersChanged(modifiers),
                egui::Event::Key { key: egui::Key::S, physical_key: None, pressed: true, repeat: false, modifiers },
                egui::Event::ModifiersChanged(egui::Modifiers::default()),
            ],
            ..Default::default()
        };
        let _ = h.ctx.run_ui(raw, |ui| crate::toolbar::shortcuts(&mut h.app, ui.ctx()));
        assert_eq!(calls.get(), 1, "the Save picker must open, rather than plain-S command search");
    }
}

#[test]
fn interactive_open_uses_background_loader_and_keeps_current_design_on_cancel() {
    let mut h = Harness::new();
    manufacturing(&mut h);
    let path = h.dir.join("cam.solidcraft");
    h.app.run("file.save", json!({"path":path})).unwrap();
    let before = serde_json::to_value(&h.app.session.doc.manufacturing).unwrap();
    h.app.services.pick_open = Some(Box::new(|| Box::pin(async { None })));
    h.app.start("doc.open");
    h.frames(2);
    assert_eq!(serde_json::to_value(&h.app.session.doc.manufacturing).unwrap(), before);
    crate::documents::new_design(&mut h.app);
    let picked = path.clone();
    h.app.services.pick_open = Some(Box::new(move || {
        let path = picked.to_string_lossy().into_owned();
        Box::pin(async move { Some(path) })
    }));
    h.app.start("doc.open");
    h.frames(2);
    let started = std::time::Instant::now();
    while h.app.file_picker_busy() || h.app.file_load.busy() {
        h.frames(1);
        assert!(started.elapsed().as_secs() < 30);
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    assert_eq!(serde_json::to_value(&h.app.session.doc.manufacturing).unwrap(), before);
    assert_eq!(h.app.session.path.as_deref(), path.to_str());
}
