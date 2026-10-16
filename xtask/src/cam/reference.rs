//! Completeness and provenance of local reference captures; no vendor API dependency.
use super::*;

#[derive(Debug)]
pub(super) struct Capture {
    pub simulation_status: verify::Status,
    pub provenance: Value,
}

pub(super) fn validate(dir: &Path, manifest: &Value, input: &JobInput) -> Result<Capture, String> {
    for field in ["post_identity", "source_geometry"] {
        if manifest[field].as_str().is_none_or(|s| s.trim().is_empty()) {
            return Err(format!("missing reference provenance: {field}"));
        }
    }
    if manifest["capabilities"]["license"].as_str().is_none_or(|s| s.trim().is_empty())
        || manifest["capabilities"]["strategies"]
            .as_array()
            .is_none_or(|v| v.is_empty() || v.iter().any(|s| s.as_str().is_none_or(|s| s.trim().is_empty())))
    {
        return Err("capture must record Fusion license/capabilities".into());
    }
    let source = manifest["source_geometry"].as_str().ok_or("missing source geometry")?;
    if Path::new(source).components().count() != 1 || !matches!(Path::new(source).components().next(), Some(std::path::Component::Normal(_))) {
        return Err("source geometry must name a file in the reference package".into());
    }
    let extension = Path::new(source).extension().and_then(|s| s.to_str()).unwrap_or_default().to_ascii_lowercase();
    if !matches!(extension.as_str(), "step" | "stp" | "stl" | "f3d" | "iges" | "igs" | "obj") {
        return Err("source geometry must be a captured CAD or mesh artifact".into());
    }
    let artifacts = manifest["artifact_sha256"].as_object().ok_or("missing captured artifact hashes")?;
    for name in [
        source,
        "input.json",
        "reference-input.json",
        "reference.nc",
        "setup-sheet.html",
        "setup-sheet-values.json",
        "stock.stl",
        "simulation.json",
        "post-properties.json",
    ] {
        let bytes = read(&dir.join(name))?;
        if bytes.is_empty() {
            return Err(format!("empty artifact {name}"));
        }
        let actual = format!("{:x}", Sha256::digest(bytes));
        if artifacts.get(name).and_then(Value::as_str) != Some(actual.as_str()) {
            return Err(format!("missing or mismatched captured artifact hash: {name}"));
        }
    }
    let properties = json_file(&dir.join("post-properties.json"))?;
    if !properties.is_object() && !properties.is_array() {
        return Err("post properties must be a captured JSON object or array".into());
    }
    let simulation = json_file(&dir.join("simulation.json"))?;
    if simulation["complete"] != true || simulation["at_program_end"] != true {
        return Err("reference simulation/end-position capture is incomplete".into());
    }
    let findings = simulation["findings"].as_array().ok_or("reference simulation findings were not captured")?;
    let order: Vec<_> = input.operations.iter().map(|op| op.id).collect();
    if simulation["completed_operations"] != json!(order) {
        return Err("reference simulation operation order or completeness differs".into());
    }
    if simulation["stock_after_operation"] != json!(order.last()) {
        return Err("reference stock is not captured after the final operation".into());
    }
    if simulation["accuracy"]["setting"].as_str().is_none_or(|s| s.trim().is_empty()) {
        return Err("reference simulation accuracy setting was not captured".into());
    }
    let resolution = &simulation["accuracy"]["resolution_mm"];
    if !resolution.as_f64().is_some_and(|v| v.is_finite() && v > 0.)
        && !(resolution.is_null() && simulation["accuracy"]["resolution_status"] == "not_reported")
    {
        return Err("reference simulation resolution must be positive or explicitly not_reported".into());
    }
    let mut simulation_status =
        serde_json::from_value(simulation["status"].clone()).map_err(|e| format!("invalid reference simulation status: {e}"))?;
    for finding in findings {
        if finding["detail"].as_str().is_none_or(|s| s.trim().is_empty()) {
            return Err("reference simulation finding lacks detail".into());
        }
        let status = match finding["severity"].as_str() {
            Some("error") => verify::Status::Fail,
            Some("unresolved") => verify::Status::Inconclusive,
            Some("information") => verify::Status::Pass,
            _ => return Err("reference simulation finding has unknown severity".into()),
        };
        simulation_status = combined_status([simulation_status, status]);
    }
    Ok(Capture {
        simulation_status,
        provenance: json!({
            "fusion_version":manifest["fusion_version"], "captured_at":manifest["captured_at"],
            "input_fingerprint":input.fingerprint, "generator_version":cam::VERSION,
            "solve_post":nc::DIALECT, "reference_post":manifest["post_identity"],
            "reference_post_sha256":manifest["post_sha256"], "post_properties":properties,
            "capabilities":manifest["capabilities"], "source_geometry":source,
            "artifact_sha256":artifacts, "reference_simulation":simulation,
            "verifier_uncertainty_mm":cam::VERIFY_ERROR, "post_error_budget_mm":cam::POST_ERROR,
            "comparison_tolerance_mm":cam::TOLERANCE,
        }),
    })
}
