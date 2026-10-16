//! CAM analytic validation and local-only Fusion oracle reports.
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use solidcraft_cam::{self as cam, JobInput, nc, verify};
use std::path::{Path, PathBuf};

mod reference;
#[cfg(test)]
mod tests;
const COMPARISON_VERSION: &str = "cam-oracle/2";

fn write(path: &Path, content: impl AsRef<[u8]>) -> Result<(), String> {
    std::fs::write(path, content).map_err(|e| format!("{}: {e}", path.display()))
}
fn json_file(path: &Path) -> Result<Value, String> {
    let bytes = read(path)?;
    serde_json::from_slice(&bytes).map_err(|e| format!("{}: {e}", path.display()))
}
fn read(path: &Path) -> Result<Vec<u8>, String> {
    let meta = std::fs::metadata(path).map_err(|e| format!("{}: {e}", path.display()))?;
    if meta.len() > 128 * 1024 * 1024 {
        return Err("oracle artifact exceeds 128 MiB".into());
    }
    std::fs::read(path).map_err(|e| e.to_string())
}
fn pretty(value: &Value) -> Result<String, String> {
    serde_json::to_string_pretty(value).map_err(|e| e.to_string())
}
fn directory(path: &Path) -> Result<(), String> {
    std::fs::create_dir_all(path).map_err(|e| e.to_string())
}
fn esc(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

pub fn validate(root: &Path) -> Result<(), String> {
    let out = root.join("target/cam-validation");
    directory(&out)?;
    let mut cases = Vec::new();
    for &name in cam::corpus::CASES {
        let result = (|| -> Result<Value, String> {
            let input = cam::corpus::fixture(name).map_err(|e| e.to_string())?;
            let path = cam::generate::generate(input.clone()).map_err(|e| e.to_string())?;
            let mut stages = Vec::new();
            for completed in 1..=input.operations.len() {
                let operation = input.operations.get(completed - 1).ok_or("missing operation")?;
                let end = path.blocks.iter().rposition(|b| b.operation == operation.id).ok_or("missing operation motions")? + 1;
                // Validate prefixes of the actual complete program, not independently
                // generated replacements. Final protected geometry remains intact.
                let mut stage_input = input.clone();
                stage_input.operations.truncate(completed);
                stage_input.fingerprint.clear();
                stage_input.fingerprint = cam::fingerprint(&stage_input).map_err(|e| e.to_string())?;
                let stage_path = cam::Toolpath { fingerprint: stage_input.fingerprint.clone(), blocks: path.blocks[..end].to_vec() };
                let mut dir = out.join(name);
                if completed != input.operations.len() {
                    dir = dir.join(format!("after-{completed}"));
                }
                let mut stage = validate_stage(&dir, name, completed, &stage_input, &stage_path)?;
                stage["operation"] = json!(operation.id);
                stage["artifacts"] = json!(if completed == input.operations.len() { ".".to_string() } else { format!("after-{completed}") });
                stages.push(stage);
            }
            let mut summary = stages.last().cloned().ok_or("empty fixture")?;
            summary["case"] = json!(name);
            summary["pass"] = json!(stages.iter().all(|s| s["pass"] == true));
            summary["stages"] = json!(stages);
            Ok(summary)
        })();
        cases.push(result.unwrap_or_else(|e| json!({"case":name,"pass":false,"error":e})));
    }
    report(&out, &cases, "Analytic CAM validation")?;
    println!("CAM validation: {}/{} passed → {}", cases.iter().filter(|v| v["pass"] == true).count(), cases.len(), out.display());
    if cases.iter().all(|v| v["pass"] == true) { Ok(()) } else { Err("CAM analytic validation failed; see report.json".into()) }
}
fn validate_stage(dir: &Path, name: &str, completed: usize, input: &JobInput, path: &cam::Toolpath) -> Result<Value, String> {
    let parsed = nc::round_trip(path, &input.setup, &input.tools).map_err(|e| e.to_string())?;
    let report = verify::simulate(input, &parsed, Default::default()).map_err(|e| e.to_string())?;
    let mut probes = Vec::new();
    for (xy, expected) in cam::corpus::probes(name, completed).map_err(|e| e.to_string())? {
        let actual = verify::probe_height(input, &parsed, xy).map_err(|e| e.to_string())?;
        probes.push(json!({"xy":xy,"expected":expected,"actual":actual,"pass":(actual-expected).abs()<=cam::TOLERANCE}));
    }
    directory(dir)?;
    write(&dir.join("input.json"), pretty(&json!(input))?)?;
    write(&dir.join("toolpath.json"), pretty(&json!(path))?)?;
    let code = nc::post(path, &input.setup).map_err(|e| e.to_string())?;
    write(&dir.join("program.nc"), &code)?;
    write(&dir.join("solve.html"), program_html(&code, &report))?;
    write(&dir.join("stock-difference.svg"), stock_overlay(input, &report))?;
    write(&dir.join("validation.json"), pretty(&json!(report))?)?;
    let stock = verify::stock_stl(&report, input.setup.stock.min.z).map_err(|e| e.to_string())?;
    write(&dir.join("stock.stl"), &stock)?;
    let mesh = cam::compare::read_stl(stock.as_bytes()).map_err(|e| e.to_string())?;
    let exported = cam::compare::stock_mesh(&report, &mesh, input.setup.stock.min.z, cam::TOLERANCE).map_err(|e| e.to_string())?;
    let mut export_values = json!(exported);
    export_values["maximum_added_height_budget"] = json!(cam::STOCK_EXPORT_ERROR);
    write(&dir.join("stock-export-validation.json"), pretty(&export_values)?)?;
    write(&dir.join("paths.svg"), overlay(input, &parsed, None)?)?;
    let values = json!({"setup":input.setup,"tools":input.tools,"operations":input.operations});
    write(&dir.join("setup-sheet-values.json"), pretty(&values)?)?;
    write(
        &dir.join("setup-sheet.html"),
        format!(
            "<!doctype html><meta charset=utf-8><title>Analytic CAM setup</title><h1>Analytic CAM setup</h1><p>Lengths: mm; feeds: mm/min; angles: radians. Generator: {}. Input: {}</p><pre>{}</pre>",
            cam::VERSION,
            esc(&input.fingerprint),
            esc(&pretty(&values)?)
        ),
    )?;
    let pass = report.status == verify::Status::Pass
        && report.finished_dimensions == verify::Status::Pass
        && report.stock_complete
        && exported.status == verify::Status::Pass
        && probes.iter().all(|p| p["pass"] == true);
    Ok(json!({"pass":pass,"status":report.status,"finished_dimensions":report.finished_dimensions,"probes":probes,
        "stock_export":export_values,"cells":report.cells.len(),"work":report.work,"metrics":metrics(input,&parsed,&report)?,
        "findings":report.findings.iter().take(5).collect::<Vec<_>>()}))
}
fn metrics(input: &JobInput, parsed: &nc::Parsed, report: &verify::Report) -> Result<Value, String> {
    let segments = nc::segments(parsed, &input.setup).map_err(|e| e.to_string())?;
    let tools: Vec<_> =
        parsed.records.iter().filter_map(|r| if let cam::Motion::Tool { number, .. } = r.motion { Some(number) } else { None }).collect();
    let retracts = segments.iter().filter(|s| s.rapid && s.to.z > s.from.z + 1e-9).count();
    Ok(json!({"tool_order":tools,"retract_count":retracts,"cutting_distance":report.cutting_distance,
        "rapid_distance":report.rapid_distance,"cutting_minutes":report.cutting_minutes,
        "remaining_volume":[report.remaining_volume_low,report.remaining_volume_high]}))
}
pub fn oracle(root: &Path, args: &[&str]) -> Result<(), String> {
    let dir = args.windows(2).find(|w| w[0] == "--dir").map(|w| PathBuf::from(w[1])).ok_or("cam-oracle requires --dir LOCAL_ORACLE_DIRECTORY")?;
    let canonical = std::fs::canonicalize(&dir).map_err(|e| e.to_string())?;
    if canonical.starts_with(root) {
        let ignored = std::process::Command::new("git")
            .current_dir(root)
            .args(["check-ignore", "--quiet", "--"])
            .arg(&canonical)
            .status()
            .map_err(|e| e.to_string())?;
        if !ignored.success() {
            return Err("Fusion storage inside this repository must be git-ignored (use plan/fusion/cam)".into());
        }
    }
    let out = dir.join("comparison");
    directory(&out)?;
    let mut cases = Vec::new();
    for name in ["face", "contour", "pocket", "drill"] {
        let case = dir.join(name);
        let value = compare_case(&case, &out.join(name)).unwrap_or_else(|e| json!({"pass":false,"status":"incomplete","error":e}));
        let mut value = value;
        value["case"] = json!(name);
        cases.push(value);
    }
    report(&out, &cases, "Local Fusion CAM comparison")?;
    let _ = root;
    println!("Fusion CAM oracle: {}/{} passed → {}", cases.iter().filter(|v| v["pass"] == true).count(), cases.len(), out.display());
    if cases.iter().all(|v| v["pass"] == true) {
        Ok(())
    } else {
        Err("Fusion parity unverified: missing, failed or inconclusive reference cases".into())
    }
}
fn compare_case(dir: &Path, out: &Path) -> Result<Value, String> {
    let manifest = json_file(&dir.join("manifest.json"))?;
    for field in ["fusion_version", "post_sha256", "captured_at", "input_fingerprint"] {
        if manifest[field].as_str().is_none_or(str::is_empty) {
            return Err(format!("missing reference provenance: {field}"));
        }
    }
    let digest = format!("{:x}", Sha256::digest(read(&dir.join("post.cps"))?));
    if manifest["post_sha256"] != digest {
        return Err("reference post hash mismatch".into());
    }
    if manifest["capture_complete"] != true {
        return Err("reference capture is incomplete".into());
    }
    let input_value = json_file(&dir.join("input.json"))?;
    let reference_value = json_file(&dir.join("reference-input.json"))?;
    if input_value != reference_value {
        return Err("SolidCraft and reference inputs differ; comparison invalid".into());
    }
    let input: JobInput = serde_json::from_value(input_value).map_err(|e| e.to_string())?;
    if manifest["input_fingerprint"] != input.fingerprint {
        return Err("manifest input fingerprint mismatch".into());
    }
    let mut unhashed = input.clone();
    unhashed.fingerprint.clear();
    if cam::fingerprint(&unhashed).map_err(|e| e.to_string())? != input.fingerprint {
        return Err("input fingerprint does not describe supplied geometry and settings".into());
    }
    let capture = reference::validate(dir, &manifest, &input)?;
    let reference = String::from_utf8(read(&dir.join("reference.nc"))?).map_err(|e| e.to_string())?;
    let reference_code = reference;
    let reference = nc::parse(&reference_code, &input.tools).map_err(|e| e.to_string())?;
    let path = cam::generate::generate(input.clone()).map_err(|e| e.to_string())?;
    let own = nc::round_trip(&path, &input.setup, &input.tools).map_err(|e| e.to_string())?;
    let ours = verify::simulate(&input, &own, Default::default()).map_err(|e| e.to_string())?;
    let mut theirs = verify::simulate(&input, &reference, Default::default()).map_err(|e| e.to_string())?;
    // The verifier interprets the reference dialect, but did not create this post.
    // Attribute the imported program to its captured post rather than our own.
    theirs.post_version = format!("{}; sha256={digest}", manifest["post_identity"].as_str().unwrap_or_default());
    directory(out)?;
    write(&out.join("solve.html"), program_html(&nc::post(&path, &input.setup).map_err(|e| e.to_string())?, &ours))?;
    write(&out.join("fusion.html"), program_html(&reference_code, &theirs))?;
    write(&out.join("stock-difference.svg"), stock_overlay(&input, &ours))?;
    write(&out.join("solve-validation.json"), pretty(&json!(ours))?)?;
    write(&out.join("fusion-nc-validation.json"), pretty(&json!(theirs))?)?;
    write(&out.join("paths.svg"), overlay(&input, &own, Some(&reference))?)?;
    let mesh = cam::compare::read_stl(&read(&dir.join("stock.stl"))?).map_err(|e| e.to_string())?;
    let stock = cam::compare::stock_reports(&ours, &theirs, cam::TOLERANCE).map_err(|e| e.to_string())?;
    let mesh_check = cam::compare::stock_mesh(&ours, &mesh, input.setup.stock.min.z, cam::TOLERANCE).map_err(|e| e.to_string())?;
    let reference_mesh_check = cam::compare::stock_mesh(&theirs, &mesh, input.setup.stock.min.z, cam::TOLERANCE).map_err(|e| e.to_string())?;
    let metadata = json_file(&dir.join("setup-sheet-values.json"))?;
    let setup_values_match =
        metadata["setup"] == json!(input.setup) && metadata["tools"] == json!(input.tools) && metadata["operations"] == json!(input.operations);
    let status = combined_status([
        ours.status,
        theirs.status,
        ours.finished_dimensions,
        theirs.finished_dimensions,
        stock.status,
        mesh_check.status,
        reference_mesh_check.status,
        capture.simulation_status,
        if setup_values_match { verify::Status::Pass } else { verify::Status::Fail },
    ]);
    let pass = status == verify::Status::Pass;
    let solve_metrics = metrics(&input, &own, &ours)?;
    let reference_metrics = metrics(&input, &reference, &theirs)?;
    Ok(json!({"pass":pass,"status":status,"comparison_version":COMPARISON_VERSION,"provenance":capture.provenance,
            "solve":ours.status,"reference":theirs.status,"finished_dimensions":[ours.finished_dimensions,theirs.finished_dimensions],
            "stock":stock,"reference_stl":mesh_check,"reference_nc_vs_reference_stl":reference_mesh_check,
            "setup_values_match":setup_values_match,"distance_delta":ours.cutting_distance-theirs.cutting_distance,
            "minutes_delta":ours.cutting_minutes-theirs.cutting_minutes,
            "volume_intervals":[[ours.remaining_volume_low,ours.remaining_volume_high],[theirs.remaining_volume_low,theirs.remaining_volume_high]],
            "metrics":{"solve":solve_metrics,"reference":reference_metrics}}))
}
fn combined_status(statuses: impl IntoIterator<Item = verify::Status>) -> verify::Status {
    statuses.into_iter().fold(verify::Status::Pass, |result, next| match (result, next) {
        (verify::Status::Fail, _) | (_, verify::Status::Fail) => verify::Status::Fail,
        (verify::Status::Inconclusive, _) | (_, verify::Status::Inconclusive) => verify::Status::Inconclusive,
        _ => verify::Status::Pass,
    })
}
fn overlay(input: &JobInput, a: &nc::Parsed, b: Option<&nc::Parsed>) -> Result<String, String> {
    let min = input.setup.stock.min;
    let max = input.setup.stock.max;
    let w = (max.x - min.x).max(1.0);
    let h = (max.y - min.y).max(1.0);
    let mut svg = format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"{} {} {} {}\"><rect x=\"{}\" y=\"{}\" width=\"{}\" height=\"{}\" fill=\"#101820\"/>",
        min.x - 5.0,
        min.y - 5.0,
        w + 10.0,
        h + 10.0,
        min.x - 5.0,
        min.y - 5.0,
        w + 10.0,
        h + 10.0
    );
    for (program, color, name) in std::iter::once((a, "#42c4ff", "solve")).chain(b.map(|b| (b, "#ffb347", "fusion"))) {
        for s in nc::segments(program, &input.setup).map_err(|e| e.to_string())? {
            svg.push_str(&format!(
                "<a href=\"{name}.html#L{line}\"><path d=\"M {} {} L {} {}\" fill=\"none\" stroke=\"{}\" stroke-width=\"{}\" opacity=\"{}\"><title>NC line {}</title></path></a>",
                s.from.x,
                s.from.y,
                s.to.x,
                s.to.y,
                color,
                w / 400.0,
                if s.rapid { 0.25 } else { 0.8 },
                s.line,
                line = s.line
            ));
        }
    }
    svg.push_str("</svg>");
    Ok(svg)
}
fn report(out: &Path, cases: &[Value], title: &str) -> Result<(), String> {
    write(&out.join("report.json"), pretty(&json!({"version":cam::VERSION,"comparison_version":COMPARISON_VERSION,"cases":cases}))?)?;
    let mut html = format!(
        "<!doctype html><meta charset=utf-8><title>{title}</title><style>body{{font:16px system-ui;margin:40px;max-width:1100px}}pre{{white-space:pre-wrap;background:#eee;padding:16px}}img{{width:480px}}a{{color:#145aa8}}</style><h1>{title}</h1><p>Blue: SolidCraft; orange: Fusion. Path and time differences are diagnostic, not equivalence gates.</p>"
    );
    for c in cases {
        let name = c["case"].as_str().unwrap_or("unknown");
        html.push_str(&format!(
            "<h2>{}</h2><pre>{}</pre><a href=\"{}/paths.svg\">Toolpath overlay</a><br><img src=\"{}/paths.svg\" alt=\"Toolpath overlay\">",
            esc(name),
            esc(&pretty(c)?),
            esc(name),
            esc(name)
        ));
        html.push_str(&format!("<p><a href=\"{name}/solve.html\">SolidCraft line-linked diagnostics</a></p><img src=\"{name}/stock-difference.svg\" alt=\"Remaining material relative to protected geometry\">"));
        if let Some(stages) = c["stages"].as_array() {
            html.push_str("<h3>After each operation</h3><ul>");
            for stage in stages {
                let directory = esc(stage["artifacts"].as_str().unwrap_or("."));
                html.push_str(&format!("<li>Operation {}: {} — <a href=\"{name}/{directory}/solve.html\">NC and diagnostics</a>, <a href=\"{name}/{directory}/setup-sheet.html\">setup sheet</a>, <a href=\"{name}/{directory}/stock.stl\">stock STL</a>, <a href=\"{name}/{directory}/stock-export-validation.json\">stock export comparison</a></li>",stage["operation"],if stage["pass"] == true { "pass" } else { "failed or inconclusive" }));
            }
            html.push_str("</ul>");
        }
    }
    write(&out.join("report.html"), html)
}

fn program_html(code: &str, validation: &verify::Report) -> String {
    let mut html = String::from(
        "<!doctype html><meta charset=utf-8><title>NC diagnostics</title><style>body{font:16px system-ui;margin:30px}pre{line-height:1.6}pre a{display:block;color:inherit;text-decoration:none}a:target{background:#ffc}code{white-space:pre}</style><h1>NC diagnostics</h1><ul>",
    );
    for f in &validation.findings {
        let location = f.source.map(|source| format!("Operation {}, motion {} · ", source.operation, source.block)).unwrap_or_default();
        let label = esc(&format!("{location}{}: {}", f.kind, f.detail));
        if let Some(line) = f.line {
            html.push_str(&format!("<li><a href=\"#L{line}\">Line {line}: {label}</a></li>"));
        } else {
            html.push_str(&format!("<li>{label}</li>"));
        }
    }
    html.push_str("</ul><pre>");
    for (i, line) in code.lines().enumerate() {
        let n = i + 1;
        html.push_str(&format!("<a id=\"L{n}\" href=\"#L{n}\">{n:6}  <code>{}</code></a>", esc(line)));
    }
    html.push_str("</pre>");
    html
}
fn stock_overlay(input: &JobInput, validation: &verify::Report) -> String {
    let min = input.setup.stock.min;
    let max = input.setup.stock.max;
    let mut svg = format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"{} {} {} {}\"><title>Sampled stock height above protected geometry; diagnostic only. Orange: remaining stock; red: definite gouge; blue: within tolerance.</title>",
        min.x,
        min.y,
        max.x - min.x,
        max.y - min.y
    );
    let step = validation.cells.len().div_ceil(20_000).max(1);
    for c in validation.cells.iter().step_by(step) {
        let excess = c.low - c.target_high;
        let gouge = c.target_low - c.high;
        let color = if gouge > validation.tolerance {
            "#e33"
        } else if excess > validation.tolerance {
            "#fa3"
        } else {
            "#29a6d2"
        };
        svg.push_str(&format!("<rect x=\"{}\" y=\"{}\" width=\"{}\" height=\"{}\" fill=\"{color}\"><title>Height [{:.6}, {:.6}] mm; excess at least {:.6} mm</title></rect>",c.min[0],c.min[1],c.max[0]-c.min[0],c.max[1]-c.min[1],c.low,c.high,excess));
    }
    svg.push_str("</svg>");
    svg
}
