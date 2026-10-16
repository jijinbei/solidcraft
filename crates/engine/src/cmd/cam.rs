//! Manufacturing commands shared by UI, CLI and MCP.
use super::CommandSpec;
use crate::{EngineError, Result, Session, cam, manufacturing as jobs};
use serde_json::{Value, json};
use std::sync::atomic::Ordering;

pub static COMMANDS: &[CommandSpec] = &[
    CommandSpec::new("cam.holes.from_selection", "Use Selected Holes", from_holes)
        .noundo()
        .params("setup: ID, selections: [{kind:face,body,index,occurrence?:[IDs]}|{kind:profile,sketch,index,occurrence?:[IDs]}]; returns ordered holes, diameters and durable sources; face body is a world label unless occurrence is explicit"),
    CommandSpec::new("cam.boundary.from_profile", "Use Sketch Profile", from_profile)
        .noundo()
        .params("setup: ID, sketch: feature ID, profile: index, occurrence?: [IDs]; returns boundary and durable source reference"),
    CommandSpec::new("cam.setup.put", "Milling Setup", setup)
        .at("MILLING", "SETUP")
        .icon("box")
        .params("setup: complete Setup object (create or replace by ID)"),
    CommandSpec::new("cam.tool.put", "Tool Library", tool).at("MILLING", "SETUP").icon("hole").params("tool: complete Tool object"),
    CommandSpec::new("cam.operation.put", "Milling Operation", operation)
        .at("MILLING", "TOOLPATHS")
        .icon("cam_path")
        .params("operation: complete Operation object; strategy face, contour, pocket or drill"),
    CommandSpec::new("cam.program.put", "NC Program", program)
        .at("MILLING", "SETUP")
        .icon("save")
        .params("program: {id,name,setup,operations:[ordered IDs]}"),
    CommandSpec::new("cam.operation.order", "Reorder Operations", order).params("program: ID, operations: permutation of existing operation IDs"),
    CommandSpec::new("cam.delete", "Delete Manufacturing Item", delete)
        .params("kind: setup|tool|operation|program, id; referenced items cannot be deleted"),
    CommandSpec::new("cam.inspect", "Inspect Manufacturing", inspect)
        .noundo()
        .params("job?: ID; include_path?: bool; include_stock?: bool (world-space display mesh); motion?: completed block count; finding?: zero-based verification finding index"),
    CommandSpec::new("cam.generate", "Generate Toolpaths", generate)
        .at("MILLING", "TOOLPATHS")
        .icon("cam_generate")
        .noundo()
        .params("program: ID, background?: bool (default false); returns job ID"),
    CommandSpec::new("cam.job.status", "CAM Job Status", status).noundo().params("job: ID"),
    CommandSpec::new("cam.job.cancel", "Cancel CAM Job", cancel).noundo().params("job: ID"),
    CommandSpec::new("cam.simulate", "Simulate Toolpaths", simulate)
        .at("MILLING", "VERIFY")
        .icon("cam_verify")
        .noundo()
        .params("job: ID, background?: bool"),
    CommandSpec::new("cam.post", "Post Reference G-code", post)
        .at("MILLING", "EXPORT")
        .icon("save")
        .noundo()
        .params("job: ID, path?: string, background?: bool; requires current passing verification; background output is published by cam.export.result"),
    CommandSpec::new("cam.export", "Export CAM Artifact", export)
        .noundo()
        .params("job: ID, format: toolpath|setup_sheet|stock|validation, path?: string, background?: bool; background output is published by cam.export.result"),
    CommandSpec::new("cam.export.result", "Collect CAM Export", export_result).noundo().params("job: ID; consumes completed output after checking freshness; writes the requested path or returns content"),
];
fn from_holes(s: &mut Session, p: &Value) -> Result<Value> {
    let setup = id(p, "setup")?;
    let setup = s.doc.manufacturing.setups.iter().find(|x| x.id == setup).ok_or_else(|| EngineError::Other("setup not found".into()))?;
    let (sources, selected) = jobs::holes::select(s, setup, decode(p, "selections")?)?;
    Ok(
        json!({"sources":sources,"holes":selected.iter().map(|h|h.center).collect::<Vec<_>>(),"diameters":selected.iter().map(|h|h.diameter).collect::<Vec<_>>()}),
    )
}
fn from_profile(s: &mut Session, p: &Value) -> Result<Value> {
    let setup = id(p, "setup")?;
    let sketch = id(p, "sketch")?;
    let profile = id(p, "profile")? as usize;
    let state = s.model.state();
    let profile = state
        .sketches
        .iter()
        .find(|x| x.feature == sketch)
        .and_then(|s| s.profiles.get(profile))
        .ok_or_else(|| EngineError::Other("selected profile not found".into()))?;
    let source = cam::ProfileRef {
        sketch,
        curves: profile.curve_ids(),
        occurrence: p.get("occurrence").map(|_| decode(p, "occurrence")).transpose()?.unwrap_or_default(),
    };
    let setup = s.doc.manufacturing.setups.iter().find(|x| x.id == setup).ok_or_else(|| EngineError::Other("setup not found".into()))?;
    let boundary = jobs::profile_boundary(s, setup, &source, cam::TOLERANCE)?;
    Ok(json!({"boundary":boundary,"source":source}))
}
fn decode<T: serde::de::DeserializeOwned>(p: &Value, key: &str) -> Result<T> {
    serde_json::from_value(p.get(key).cloned().ok_or_else(|| EngineError::Other(format!("missing {key}")))?)
        .map_err(|e| EngineError::Other(e.to_string()))
}
fn id(p: &Value, key: &str) -> Result<u64> {
    p.get(key).and_then(Value::as_u64).ok_or_else(|| EngineError::Other(format!("{key} must be an ID")))
}
macro_rules! put {($fn:ident,$key:literal,$ty:ty,$field:ident)=>{fn $fn(s:&mut Session,p:&Value)->Result<Value>{
    let item:$ty=decode(p,$key)?;let id=item.id;let mut m=s.doc.manufacturing.clone();
    if let Some(old)=m.$field.iter_mut().find(|x|x.id==id){*old=item;}else{m.$field.push(item);}
    m.validate().map_err(jobs::error)?;s.doc_mut().manufacturing=m;Ok(json!({"id":id}))
}};}
put!(setup, "setup", cam::Setup, setups);
put!(tool, "tool", cam::Tool, tools);
put!(operation, "operation", cam::Operation, operations);
put!(program, "program", cam::Program, programs);
fn order(s: &mut Session, p: &Value) -> Result<Value> {
    let program = id(p, "program")?;
    let ids: Vec<u64> = decode(p, "operations")?;
    let mut m = s.doc.manufacturing.clone();
    let target = m.programs.iter_mut().find(|x| x.id == program).ok_or_else(|| EngineError::Other("program not found".into()))?;
    let mut a = target.operations.clone();
    let mut b = ids.clone();
    a.sort();
    b.sort();
    if a != b {
        return Err(EngineError::Other("order must be a permutation of the program operations".into()));
    }
    target.operations = ids;
    m.validate().map_err(jobs::error)?;
    s.doc_mut().manufacturing = m;
    Ok(json!({"program":program}))
}
fn delete(s: &mut Session, p: &Value) -> Result<Value> {
    let id = id(p, "id")?;
    let mut m = s.doc.manufacturing.clone();
    let removed = match p.get("kind").and_then(Value::as_str) {
        Some("setup") => {
            let n = m.setups.len();
            m.setups.retain(|x| x.id != id);
            n != m.setups.len()
        }
        Some("tool") => {
            let n = m.tools.len();
            m.tools.retain(|x| x.id != id);
            n != m.tools.len()
        }
        Some("operation") => {
            let n = m.operations.len();
            m.operations.retain(|x| x.id != id);
            n != m.operations.len()
        }
        Some("program") => {
            let n = m.programs.len();
            m.programs.retain(|x| x.id != id);
            n != m.programs.len()
        }
        _ => return Err(EngineError::Other("unknown CAM item kind".into())),
    };
    if !removed {
        return Err(EngineError::Other("CAM item not found".into()));
    }
    m.validate().map_err(jobs::error)?;
    s.doc_mut().manufacturing = m;
    Ok(json!({"deleted":id}))
}
fn inspect(s: &mut Session, p: &Value) -> Result<Value> {
    if p.get("job").is_none() {
        if p.get("finding").is_some()
            || p.get("motion").is_some()
            || ["include_path", "include_stock"].iter().any(|key| p.get(key).and_then(Value::as_bool) == Some(true))
        {
            return Err(EngineError::Other("job is required for result inspection".into()));
        }
        return Ok(json!({"manufacturing":s.doc.manufacturing,"jobs":s.cam_jobs.jobs.keys().collect::<Vec<_>>()}));
    }
    let job = jobs::job(s, id(p, "job")?)?;
    let mut result = status(s, p)?;
    match job.state()? {
        jobs::JobState::Generated(path) | jobs::JobState::Verified(path, _) if p.get("include_path").and_then(Value::as_bool) == Some(true) => {
            result["toolpath"] = json!(path);
        }
        _ => {}
    }
    if let Some(motion) = p.get("motion") {
        jobs::current(s, &job)?;
        let motion = motion
            .as_u64()
            .and_then(|n| usize::try_from(n).ok())
            .ok_or_else(|| EngineError::Other("motion must be a nonnegative block count".into()))?;
        let path = match job.state()? {
            jobs::JobState::Generated(path) | jobs::JobState::Verified(path, _) => path,
            _ => return Err(EngineError::Other("generate before inspecting playback".into())),
        };
        let state = cam::playback::at(&path, &job.input()?.setup, motion).map_err(jobs::error)?;
        result["playback"] = json!(state);
        let operation = job.input()?.operations.iter().find(|o| Some(o.id) == state.operation);
        result["source"] = json!(operation.and_then(|o| o.source.as_ref()));
        result["hole_sources"] = json!(operation.map(|o| &o.hole_sources));
    }
    if let Some(index) = p.get("finding") {
        jobs::current(s, &job)?;
        let index =
            index.as_u64().and_then(|i| usize::try_from(i).ok()).ok_or_else(|| EngineError::Other("finding must be a nonnegative index".into()))?;
        let jobs::JobState::Verified(path, report) = job.state()? else {
            return Err(EngineError::Other("simulate before inspecting findings".into()));
        };
        let finding = report.findings.get(index).ok_or_else(|| EngineError::Other("finding index out of range".into()))?;
        let input = job.input()?;
        let operation = finding.source.and_then(|source| input.operations.iter().find(|o| o.id == source.operation));
        let code = cam::nc::post(&path, &input.setup).map_err(jobs::error)?;
        let line = finding.line.and_then(|line| line.checked_sub(1)).and_then(|index| code.lines().nth(index));
        result["diagnostic"] = json!({"index":index,"finding":finding,"world":input.setup.frame.world(finding.at),"operation":operation,
            "source":operation.and_then(|o|o.source.as_ref()),"hole_sources":operation.map(|o|&o.hole_sources),"gcode_line":line,
            "playback":finding.source.map(|source|cam::playback::at(&path,&input.setup,source.block)).transpose().map_err(jobs::error)?});
    }
    if p.get("include_stock").and_then(Value::as_bool) == Some(true) {
        jobs::current(s, &job)?;
        result["stock_mesh"] = serde_json::to_value(job.stock_mesh()?.as_deref()).map_err(|e| EngineError::Other(e.to_string()))?;
    }
    Ok(result)
}
fn generate(s: &mut Session, p: &Value) -> Result<Value> {
    let id = jobs::start(s, id(p, "program")?, p.get("background").and_then(Value::as_bool).unwrap_or(false))?;
    status(s, &json!({"job":id}))
}
fn status(s: &mut Session, p: &Value) -> Result<Value> {
    let id = id(p, "job")?;
    let job = jobs::job(s, id)?;
    #[cfg(target_arch = "wasm32")]
    jobs::advance(&job);
    let stale = jobs::current(s, &job).is_err();
    let mut v = json!({"job":id,"stale":stale,"progress":job.progress.load(Ordering::Relaxed),"fingerprint":job.input().ok().map(|input| &input.fingerprint)});
    match job.state()? {
        jobs::JobState::Verifying(_) => v["state"] = json!("simulating"),
        jobs::JobState::Preparing => v["state"] = json!("preparing"),
        jobs::JobState::Running => v["state"] = json!("running"),
        jobs::JobState::Cancelled => v["state"] = json!("cancelled"),
        jobs::JobState::Failed(e) => {
            v["state"] = json!("failed");
            v["error"] = json!(e);
        }
        jobs::JobState::Generated(path) => {
            v["state"] = json!("generated");
            v["blocks"] = json!(path.blocks.len());
        }
        jobs::JobState::Verified(path, r) => {
            v["state"] = json!("verified");
            v["blocks"] = json!(path.blocks.len());
            v["stock_preview"] = match job.stock_mesh() {
                Ok(Some(mesh)) => json!({"available":true,"triangles":mesh.triangles.len()}),
                Ok(None) => json!({"available":false}),
                Err(e) => json!({"available":false,"error":e.to_string()}),
            };
            v["verification"] = json!({"status":r.status,"finished_dimensions":r.finished_dimensions,"stock_complete":r.stock_complete,
                "findings":r.findings,"cells":r.cells.len(),"work":r.work});
        }
    }
    v["export"] = jobs::exports::status(&job)?;
    Ok(v)
}
fn cancel(s: &mut Session, p: &Value) -> Result<Value> {
    let job = jobs::job(s, id(p, "job")?)?;
    job.cancelled.store(true, Ordering::Relaxed);
    job.set(jobs::JobState::Cancelled);
    status(s, p)
}
fn simulate(s: &mut Session, p: &Value) -> Result<Value> {
    let job = jobs::job(s, id(p, "job")?)?;
    jobs::current(s, &job)?;
    if jobs::exports::busy(&job)? {
        return Err(EngineError::Other("finish or cancel the current export first".into()));
    }
    let path = match job.state()? {
        jobs::JobState::Generated(p) | jobs::JobState::Verified(p, _) => p,
        _ => return Err(EngineError::Other("generate a path first; another job may still be running".into())),
    };
    job.set(jobs::JobState::Verifying(path));
    jobs::dispatch(job, p.get("background").and_then(Value::as_bool).unwrap_or(false), true)?;
    status(s, p)
}
fn output(p: &Value, content: String) -> Result<Value> {
    if let Some(path) = p.get("path").and_then(Value::as_str) {
        crate::io::write_atomic(std::path::Path::new(path), content.as_bytes(), false).map_err(|e| EngineError::Other(e.to_string()))?;
        Ok(json!({"path":path,"bytes":content.len()}))
    } else {
        Ok(json!({"content":content}))
    }
}
fn post(s: &mut Session, p: &Value) -> Result<Value> {
    let job = jobs::job(s, id(p, "job")?)?;
    jobs::current(s, &job)?;
    let jobs::JobState::Verified(path, report) = job.state()? else {
        return Err(EngineError::Other("simulate before posting".into()));
    };
    if report.status != cam::verify::Status::Pass || report.finished_dimensions != cam::verify::Status::Pass || !report.stock_complete {
        return Err(EngineError::Other("posting blocked by failed or inconclusive verification".into()));
    }
    if p.get("background").and_then(Value::as_bool).unwrap_or(false) {
        jobs::exports::start(job, "gcode".into(), p.get("path").and_then(Value::as_str).map(str::to_owned))?;
        return status(s, p);
    }
    cam::nc::round_trip(&path, &job.input()?.setup, &job.input()?.tools).map_err(jobs::error)?;
    output(p, cam::nc::post(&path, &job.input()?.setup).map_err(jobs::error)?)
}
fn export(s: &mut Session, p: &Value) -> Result<Value> {
    let job = jobs::job(s, id(p, "job")?)?;
    jobs::current(s, &job)?;
    let (path, report) = match job.state()? {
        jobs::JobState::Generated(p) => (p, None),
        jobs::JobState::Verified(p, r) => (p, Some(r)),
        _ => return Err(EngineError::Other("no generated result".into())),
    };
    let format = p.get("format").and_then(Value::as_str).unwrap_or("validation");
    if p.get("background").and_then(Value::as_bool).unwrap_or(false) {
        jobs::exports::start(job, format.into(), p.get("path").and_then(Value::as_str).map(str::to_owned))?;
        return status(s, p);
    }
    output(p, crate::io::cam::artifact(job.input()?, &path, report.as_deref(), format)?)
}

fn export_result(s: &mut Session, p: &Value) -> Result<Value> {
    let job = jobs::job(s, id(p, "job")?)?;
    jobs::current(s, &job)?;
    let (content, destination) = jobs::exports::take(&job)?;
    output(&destination.map_or_else(|| json!({}), |path| json!({"path":path})), content)
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    fn wait_export(s: &mut Session, job: u64) -> Value {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
        loop {
            let status = s.execute("cam.job.status", &json!({"job":job})).unwrap();
            if status["export"]["state"] != "running" {
                return status;
            }
            assert!(std::time::Instant::now() < deadline, "export worker timeout");
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
    }
    #[test]
    fn background_exports_only_publish_complete_current_results_once() {
        let mut s = configured();
        let job = s.execute("cam.generate", &json!({"program":1})).unwrap()["job"].as_u64().unwrap();
        s.execute("cam.simulate", &json!({"job":job})).unwrap();
        for format in ["toolpath", "validation", "setup_sheet", "stock", "gcode"] {
            let cmd = if format == "gcode" { "cam.post" } else { "cam.export" };
            let expected = s.execute(cmd, &json!({"job":job,"format":format})).unwrap()["content"].as_str().unwrap().to_owned();
            s.execute(cmd, &json!({"job":job,"format":format,"background":true})).unwrap();
            assert!(s.execute(cmd, &json!({"job":job,"format":format,"background":true})).is_err());
            let status = wait_export(&mut s, job);
            assert_eq!(status["export"]["state"], "ready", "{status}");
            let actual = s.execute("cam.export.result", &json!({"job":job})).unwrap()["content"].as_str().unwrap().to_owned();
            match format {
                "toolpath" | "validation" => {
                    assert_eq!(serde_json::from_str::<Value>(&actual).unwrap(), serde_json::from_str::<Value>(&expected).unwrap())
                }
                "setup_sheet" => assert!(actual.contains("<h2>Operation settings and selected geometry</h2>")),
                _ => assert_eq!(actual, expected),
            }
            assert!(s.execute("cam.export.result", &json!({"job":job})).is_err());
        }
        let path = std::env::temp_dir().join(format!("solidcraft-cam-export-{}.nc", std::process::id()));
        s.execute("cam.post", &json!({"job":job,"background":true,"path":path})).unwrap();
        assert_eq!(wait_export(&mut s, job)["export"]["state"], "ready");
        assert!(!path.exists());
        s.execute("cam.export.result", &json!({"job":job})).unwrap();
        assert!(std::fs::read_to_string(&path).unwrap().contains("G21"));
        std::fs::remove_file(&path).unwrap();
        s.execute("cam.post", &json!({"job":job,"background":true,"path":path})).unwrap();
        wait_export(&mut s, job);
        let mut tool = s.doc.manufacturing.tools[0].clone();
        tool.diameter += 0.1;
        s.execute("cam.tool.put", &json!({"tool":tool})).unwrap();
        assert!(s.execute("cam.export.result", &json!({"job":job})).unwrap_err().to_string().contains("stale"));
        assert!(!path.exists());
    }
    #[test]
    fn cancelling_a_background_export_cannot_publish_or_restart_it() {
        let mut s = configured();
        let job = s.execute("cam.generate", &json!({"program":1})).unwrap()["job"].as_u64().unwrap();
        s.execute("cam.export", &json!({"job":job,"format":"toolpath","background":true})).unwrap();
        s.execute("cam.job.cancel", &json!({"job":job})).unwrap();
        assert_eq!(wait_export(&mut s, job)["export"]["state"], "cancelled");
        assert!(s.execute("cam.export.result", &json!({"job":job})).is_err());
        assert!(s.execute("cam.export", &json!({"job":job,"format":"toolpath","background":true})).is_err());
    }
    fn configured() -> Session {
        let mut s = Session::default();
        s.execute("solid.box", &json!({"length":20,"width":20,"height":4})).unwrap();
        let mut i = cam::corpus::fixture("face").unwrap();
        i.setup.models[0].body = s.model.state().bodies[0].name.clone();
        let bounds = s.model.state().bodies[0].mesh().bounds();
        i.setup.stock.min = bounds.min;
        i.setup.stock.max = bounds.max + crate::geom::Vec3::Z;
        i.operations[0].top = bounds.max.z + 1.0;
        i.operations[0].bottom = bounds.max.z;
        s.execute("cam.setup.put", &json!({"setup":i.setup})).unwrap();
        s.execute("cam.tool.put", &json!({"tool":i.tools[0]})).unwrap();
        s.execute("cam.operation.put", &json!({"operation":i.operations[0]})).unwrap();
        s.execute("cam.program.put", &json!({"program":{"id":1,"name":"Plate","setup":1,"operations":[1]}})).unwrap();
        s
    }
    #[test]
    fn save_undo_and_stale_generation() {
        let mut s = configured();
        let saved = crate::io::write_design(&s.doc);
        assert_eq!(crate::io::read_design(&saved).unwrap().manufacturing, s.doc.manufacturing);
        let r = s.execute("cam.generate", &json!({"program":1})).unwrap();
        assert_eq!(r["state"], "generated");
        let id = r["job"].clone();
        s.execute("cam.operation.order", &json!({"program":1,"operations":[1]})).unwrap();
        s.execute("solid.box", &json!({"length":2,"width":2,"height":2})).unwrap();
        assert!(s.execute("cam.post", &json!({"job":id})).is_err());
        assert_eq!(s.execute("cam.job.status", &json!({"job":id})).unwrap()["stale"], true);
        s.execute("edit.undo", &json!({})).unwrap();
        assert_eq!(s.execute("cam.job.status", &json!({"job":id})).unwrap()["stale"], false);
    }
    #[test]
    fn operation_order_survives_reopen_undo_redo_and_controls_generated_sequence() {
        let mut s = configured();
        let mut second = s.doc.manufacturing.operations[0].clone();
        second.id = 2;
        second.name = "Second facing pass".into();
        second.feed += 25.;
        s.execute("cam.operation.put", &json!({"operation":second})).unwrap();
        s.execute("cam.program.put", &json!({"program":{"id":1,"name":"Ordered milling","setup":1,"operations":[1,2]}})).unwrap();
        let job = s.execute("cam.generate", &json!({"program":1})).unwrap()["job"].clone();
        for invalid in [json!([1, 1]), json!([2]), json!([1, 3])] {
            let before = s.doc.clone();
            assert!(s.execute("cam.operation.order", &json!({"program":1,"operations":invalid})).is_err());
            assert_eq!(s.doc, before);
        }
        s.execute("cam.operation.order", &json!({"program":1,"operations":[2,1]})).unwrap();
        assert_eq!(s.execute("cam.job.status", &json!({"job":job})).unwrap()["stale"], true);
        s.execute("edit.undo", &json!({})).unwrap();
        assert_eq!(s.doc.manufacturing.programs[0].operations, vec![1, 2]);
        assert_eq!(s.execute("cam.job.status", &json!({"job":job})).unwrap()["stale"], false);
        s.execute("edit.redo", &json!({})).unwrap();
        assert_eq!(s.doc.manufacturing.programs[0].operations, vec![2, 1]);
        assert_eq!(s.execute("cam.job.status", &json!({"job":job})).unwrap()["stale"], true);
        let doc = crate::io::read_design(&crate::io::write_design(&s.doc)).unwrap();
        let mut reopened = Session::new(doc);
        let id = reopened.execute("cam.generate", &json!({"program":1})).unwrap()["job"].as_u64().unwrap();
        let state = jobs::job(&reopened, id).unwrap().state().unwrap();
        let path = match state {
            jobs::JobState::Generated(path) => Some(path),
            _ => None,
        }
        .unwrap();
        let mut sequence = path.blocks.iter().map(|b| b.operation).collect::<Vec<_>>();
        sequence.dedup();
        assert_eq!(sequence, vec![2, 1]);
        assert_eq!(reopened.doc.manufacturing.programs[0].operations, vec![2, 1]);
    }

    #[test]
    fn referenced_tool_delete_rolls_back() {
        let mut s = configured();
        let d = s.doc.clone();
        assert!(s.execute("cam.delete", &json!({"kind":"tool","id":1})).is_err());
        assert_eq!(s.doc, d);
    }
    #[test]
    fn missing_body_is_not_guessed() {
        let mut s = configured();
        s.doc_mut().manufacturing.setups[0].models[0].body = "missing".into();
        assert!(s.execute("cam.generate", &json!({"program":1})).is_err());
    }
    #[test]
    fn generate_simulate_post_and_reopen() {
        let mut s = configured();
        let r = s.execute("cam.generate", &json!({"program":1})).unwrap();
        let id = r["job"].clone();
        let v = s.execute("cam.simulate", &json!({"job":id})).unwrap();
        assert_eq!(v["verification"]["status"], "pass", "{v}");
        let posted = s.execute("cam.post", &json!({"job":id})).unwrap();
        assert!(posted["content"].as_str().unwrap().contains("G21 G90 G17 G91.1 G94 G54 G40 G49 G80\nM5\nM9\n"));
        for format in ["toolpath", "setup_sheet", "validation", "stock"] {
            assert!(!s.execute("cam.export", &json!({"job":id,"format":format})).unwrap()["content"].as_str().unwrap().is_empty());
        }
        let doc = crate::io::read_design(&crate::io::write_design(&s.doc)).unwrap();
        let mut reopened = Session::new(doc);
        assert!(reopened.execute("cam.post", &json!({"job":id})).is_err());
        assert_eq!(reopened.execute("cam.generate", &json!({"program":1})).unwrap()["state"], "generated");
        s.execute("cam.job.cancel", &json!({"job":id})).unwrap();
        assert!(s.execute("cam.post", &json!({"job":id})).is_err());
    }
    #[test]
    fn legacy_document_has_no_manufacturing() {
        let mut value: Value = serde_json::from_slice(&crate::io::write_design(&solidcraft_doc::Document::default())).unwrap();
        value["format"] = json!("solidcraft/11");
        value.as_object_mut().unwrap().remove("manufacturing");
        let migrated = crate::io::read_design(value.to_string().as_bytes()).unwrap();
        assert!(migrated.manufacturing.is_empty());
    }
    #[test]
    fn inconclusive_dimensions_cannot_be_posted() {
        let mut s = configured();
        let r = s.execute("cam.generate", &json!({"program":1})).unwrap();
        let id = r["job"].as_u64().unwrap();
        s.execute("cam.simulate", &json!({"job":id})).unwrap();
        let job = jobs::job(&s, id).unwrap();
        if let jobs::JobState::Verified(path, report) = job.state().unwrap() {
            let mut report = (*report).clone();
            report.finished_dimensions = cam::verify::Status::Inconclusive;
            job.set(jobs::JobState::Verified(path, std::sync::Arc::new(report)));
        }
        assert!(s.execute("cam.post", &json!({"job":id})).is_err());
    }
    #[test]
    fn drilled_cad_body_generates_verifies_posts_and_reopens() {
        let mut s = Session::default();
        s.execute("solid.box", &json!({"length":20,"width":20,"height":6})).unwrap();
        s.execute("solid.hole", &json!({"position":[10,10,6],"direction":[0,0,-1],"diameter":1,"depth":1,"type":"drilled","tip_angle":118})).unwrap();
        let mut input = cam::corpus::fixture("drill").unwrap();
        input.setup.models[0].body = s.model.state().bodies[0].name.clone();
        input.operations[0].geometry = cam::Strategy::Drill { holes: vec![[10., 10.]], peck: Some(0.3), full_diameter: true };
        s.execute("cam.setup.put", &json!({"setup":input.setup})).unwrap();
        let body = s.model.state().bodies[0].clone();
        let face = (0..body.body.face_count()).find(|i| crate::kernel::hole_axis(&body.body, *i, 0.0005).is_ok()).unwrap();
        let selected =
            s.execute("cam.holes.from_selection", &json!({"setup":1,"selections":[{"kind":"face","body":body.name,"index":face}]})).unwrap();
        input.operations[0].hole_sources = serde_json::from_value(selected["sources"].clone()).unwrap();
        assert!((selected["diameters"][0].as_f64().unwrap() - 1.).abs() < 1e-8);
        let center: Vec<[f64; 2]> = serde_json::from_value(selected["holes"].clone()).unwrap();
        assert!((center[0][0] - 10.).abs() < 1e-8 && (center[0][1] - 10.).abs() < 1e-8);
        s.execute("cam.tool.put", &json!({"tool":input.tools[0]})).unwrap();
        s.execute("cam.operation.put", &json!({"operation":input.operations[0]})).unwrap();
        s.execute("cam.program.put", &json!({"program":{"id":1,"name":"Drilled plate","setup":1,"operations":[1]}})).unwrap();
        let generated = s.execute("cam.generate", &json!({"program":1})).unwrap();
        let id = generated["job"].clone();
        let verified = s.execute("cam.simulate", &json!({"job":id})).unwrap();
        assert_eq!(verified["verification"]["status"], "pass", "{verified}");
        assert_eq!(verified["verification"]["finished_dimensions"], "pass", "{verified}");
        assert!(s.execute("cam.post", &json!({"job":id})).unwrap()["content"].as_str().unwrap().contains("M30"));
        let saved = crate::io::write_design(&s.doc);
        let mut reopened = Session::new(crate::io::read_design(&saved).unwrap());
        let generated = reopened.execute("cam.generate", &json!({"program":1})).unwrap();
        let verified = reopened.execute("cam.simulate", &json!({"job":generated["job"]})).unwrap();
        assert_eq!(verified["verification"]["finished_dimensions"], "pass");
    }
    #[test]
    fn milled_cad_pocket_and_contour_generate_verify_post_and_reopen() {
        let mut model = Session::default();
        model.execute("solid.box", &json!({"length":20,"width":20,"height":6})).unwrap();
        model.execute("solid.hole", &json!({"position":[10,10,6],"direction":[0,0,-1],"diameter":16,"depth":1,"type":"simple"})).unwrap();
        model.execute("sketch.create", &json!({"plane":"XY","name":"Pocket boundary"})).unwrap();
        model.execute("sketch.circle.center", &json!({"center":[10,10],"radius":8,"id":"wall"})).unwrap();
        model.execute("sketch.finish", &json!({})).unwrap();
        let sketch = model.model.state().sketches.last().unwrap().feature;
        for name in ["contour", "pocket"] {
            let mut s = Session::new((*model.doc).clone());
            let mut input = cam::corpus::fixture(name).unwrap();
            input.setup.models[0].body = s.model.state().bodies[0].name.clone();
            input.operations[0].ramp_angle = Some(5_f64.to_radians());
            s.execute("cam.setup.put", &json!({"setup":input.setup})).unwrap();
            let selected = s.execute("cam.boundary.from_profile", &json!({"setup":1,"sketch":sketch,"profile":0})).unwrap();
            input.operations[0].source = Some(serde_json::from_value(selected["source"].clone()).unwrap());
            match &mut input.operations[0].geometry {
                cam::Strategy::Contour { boundary, .. } | cam::Strategy::Pocket { boundary } => {
                    *boundary = serde_json::from_value(selected["boundary"].clone()).unwrap()
                }
                _ => {}
            }
            s.execute("cam.tool.put", &json!({"tool":input.tools[0]})).unwrap();
            s.execute("cam.operation.put", &json!({"operation":input.operations[0]})).unwrap();
            s.execute("cam.program.put", &json!({"program":{"id":1,"name":name,"setup":1,"operations":[1]}})).unwrap();
            let generated = s.execute("cam.generate", &json!({"program":1})).unwrap();
            let id = generated["job"].clone();
            let verified = s.execute("cam.simulate", &json!({"job":id})).unwrap();
            assert_eq!(verified["verification"]["status"], "pass", "{name}: {verified}");
            assert_eq!(verified["verification"]["finished_dimensions"], "pass", "{name}: {verified}");
            assert!(s.execute("cam.post", &json!({"job":id})).unwrap()["content"].as_str().unwrap().contains("M30"));
            let mut reopened = Session::new(crate::io::read_design(&crate::io::write_design(&s.doc)).unwrap());
            let generated = reopened.execute("cam.generate", &json!({"program":1})).unwrap();
            let verified = reopened.execute("cam.simulate", &json!({"job":generated["job"]})).unwrap();
            assert_eq!(verified["verification"]["finished_dimensions"], "pass", "{name}: {verified}");
        }
    }
    #[test]
    fn diagnostic_inspection_identifies_posted_operation_and_rejects_stale_results() {
        let mut s = configured();
        assert!(s.execute("cam.inspect", &json!({"finding":0})).is_err());
        let origin = crate::geom::Vec3::new(100., 200., 0.);
        let mut setup = s.doc.manufacturing.setups[0].clone();
        setup.frame.origin = origin;
        setup.stock.min -= origin;
        setup.stock.max -= origin;
        s.execute("cam.setup.put", &json!({"setup":setup})).unwrap();
        let mut operation = s.doc.manufacturing.operations[0].clone();
        operation.bottom = 3.;
        s.execute("cam.operation.put", &json!({"operation":operation})).unwrap();
        let id = s.execute("cam.generate", &json!({"program":1})).unwrap()["job"].clone();
        assert!(s.execute("cam.inspect", &json!({"job":id,"finding":0})).is_err());
        let report = s.execute("cam.simulate", &json!({"job":id})).unwrap();
        assert_eq!(report["verification"]["status"], "fail");
        let index = report["verification"]["findings"].as_array().unwrap().iter().position(|f| f["source"].is_object()).unwrap();
        let inspected = s.execute("cam.inspect", &json!({"job":id,"finding":index})).unwrap();
        let diagnostic = &inspected["diagnostic"];
        assert_eq!(diagnostic["operation"]["id"], 1);
        assert_eq!(diagnostic["playback"]["completed"], diagnostic["finding"]["source"]["block"]);
        assert!(diagnostic["gcode_line"].as_str().unwrap().starts_with("G1"));
        let at: crate::geom::Vec3 = serde_json::from_value(diagnostic["finding"]["at"].clone()).unwrap();
        assert_eq!(diagnostic["world"], json!(at + origin));
        assert!(s.execute("cam.inspect", &json!({"job":id,"finding":-1})).is_err());
        assert!(s.execute("cam.inspect", &json!({"job":id,"finding":999999})).is_err());
        assert!(s.execute("cam.post", &json!({"job":id})).is_err());
        let exported = s.execute("cam.export", &json!({"job":id,"format":"validation"})).unwrap();
        assert!(exported["content"].as_str().unwrap().contains("source"));
        s.execute("solid.box", &json!({"length":1,"width":1,"height":1})).unwrap();
        assert!(s.execute("cam.inspect", &json!({"job":id,"finding":index})).is_err());
    }
}

#[cfg(test)]
#[path = "cam_holes_tests.rs"]
mod holes_tests;
