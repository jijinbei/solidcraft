//! Original analytic capture packages. These are not Fusion observations or parity evidence.
use super::*;
use std::sync::atomic::{AtomicU64, Ordering};

struct Package(PathBuf);
impl Drop for Package {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
fn box_stl(top: f64) -> String {
    let points = [[0., 0., 0.], [20., 0., 0.], [20., 20., 0.], [0., 20., 0.], [0., 0., top], [20., 0., top], [20., 20., top], [0., 20., top]];
    let mut text = String::from("solid original_analytic_box\n");
    for face in [[0, 2, 1], [0, 3, 2], [4, 5, 6], [4, 6, 7], [0, 1, 5], [0, 5, 4], [1, 2, 6], [1, 6, 5], [2, 3, 7], [2, 7, 6], [3, 0, 4], [3, 4, 7]] {
        text.push_str("facet normal 0 0 0\nouter loop\n");
        for index in face {
            let [x, y, z] = points[index];
            text.push_str(&format!("vertex {x} {y} {z}\n"));
        }
        text.push_str("endloop\nendfacet\n");
    }
    text.push_str("endsolid original_analytic_box\n");
    text
}
fn package(nc_height: f64, stock_height: f64) -> (Package, JobInput, Value) {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let path = std::env::temp_dir().join(format!("solidcraft-original-cam-oracle-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
    std::fs::create_dir(&path).unwrap();
    let package = Package(path);
    let input = cam::corpus::fixture("face").unwrap();
    let toolpath = cam::generate::generate(input.clone()).unwrap();
    let nc = nc::post(&toolpath, &input.setup).unwrap().replace("Z5.000000", &format!("Z{nc_height:.6}"));
    write(&package.0.join("reference.nc"), nc).unwrap();
    write(&package.0.join("input.json"), pretty(&json!(input)).unwrap()).unwrap();
    write(&package.0.join("reference-input.json"), pretty(&json!(input)).unwrap()).unwrap();
    write(&package.0.join("post.cps"), "original analytic test post marker; not executable vendor code").unwrap();
    write(&package.0.join("post-properties.json"), "{}").unwrap();
    write(&package.0.join("source.stl"), box_stl(5.)).unwrap();
    write(&package.0.join("stock.stl"), box_stl(stock_height)).unwrap();
    write(&package.0.join("setup-sheet.html"), "<!doctype html><title>Original analytic test setup</title>").unwrap();
    write(
        &package.0.join("setup-sheet-values.json"),
        pretty(&json!({"setup":input.setup,"tools":input.tools,"operations":input.operations})).unwrap(),
    )
    .unwrap();
    write(
        &package.0.join("simulation.json"),
        pretty(&json!({
        "status":"pass", "complete":true, "at_program_end":true, "findings":[],
            "completed_operations":[1], "stock_after_operation":1,
            "accuracy":{"setting":"exact analytic box", "resolution_mm":null,"resolution_status":"not_reported"},
        }))
        .unwrap(),
    )
    .unwrap();
    let mut manifest = json!({
        "fusion_version":"original analytic test; not a Fusion capture", "captured_at":"2026-10-16T00:00:00Z",
        "input_fingerprint":input.fingerprint, "post_identity":"independently authored test post",
        "post_sha256":format!("{:x}",Sha256::digest(read(&package.0.join("post.cps")).unwrap())),
        "capture_complete":true, "capabilities":{"license":"analytic test", "strategies":["face"]},
        "source_geometry":"source.stl", "artifact_sha256":{},
    });
    for name in [
        "source.stl",
        "input.json",
        "reference-input.json",
        "reference.nc",
        "setup-sheet.html",
        "setup-sheet-values.json",
        "stock.stl",
        "simulation.json",
        "post-properties.json",
    ] {
        hash(&package, &mut manifest, name);
    }
    write(&package.0.join("manifest.json"), pretty(&manifest).unwrap()).unwrap();
    (package, input, manifest)
}
fn hash(package: &Package, manifest: &mut Value, name: &str) {
    manifest["artifact_sha256"][name] = json!(format!("{:x}", Sha256::digest(read(&package.0.join(name)).unwrap())));
}

#[test]
fn oracle_requires_direct_reference_nc_to_exported_stock_agreement() {
    let (package, _, _) = package(5.006, 4.994);
    let result = compare_case(&package.0, &package.0.join("comparison")).unwrap();
    // Both used to pass through a common midpoint at Z5, although the two
    // reference results differ by 0.012 mm, beyond the 0.01 mm budget.
    assert_eq!(result["stock"]["status"], "pass", "{result}");
    assert_eq!(result["reference_stl"]["status"], "pass", "{result}");
    assert_eq!(result["finished_dimensions"], json!(["pass", "pass"]), "{result}");
    assert_eq!(result["reference_nc_vs_reference_stl"]["status"], "fail", "{result}");
    assert_eq!(result["status"], "fail");
    assert_eq!(result["pass"], false);
}

#[test]
fn oracle_complete_analytic_capture_passes_and_retains_post_provenance() {
    let (package, _, manifest) = package(5., 5.);
    let out = package.0.join("comparison");
    let result = compare_case(&package.0, &out).unwrap();
    assert_eq!(result["pass"], true, "{result}");
    assert_eq!(result["reference_nc_vs_reference_stl"]["status"], "pass");
    assert_eq!(result["comparison_version"], COMPARISON_VERSION);
    assert_eq!(result["provenance"]["reference_post_sha256"], manifest["post_sha256"]);
    assert_eq!(result["provenance"]["reference_simulation"]["accuracy"]["resolution_status"], "not_reported");
    let validation = json_file(&out.join("fusion-nc-validation.json")).unwrap();
    assert!(validation["post_version"].as_str().unwrap().starts_with("independently authored test post; sha256="));
}

#[test]
fn oracle_rejects_incomplete_stale_and_unlocated_capture_evidence() {
    let (package, input, manifest) = package(5., 5.);
    for field in ["post_identity", "source_geometry", "capabilities", "artifact_sha256"] {
        let mut bad = manifest.clone();
        bad.as_object_mut().unwrap().remove(field);
        assert!(reference::validate(&package.0, &bad, &input).is_err(), "{field}");
    }
    let mut bad = manifest.clone();
    bad["source_geometry"] = json!("../source.stl");
    assert!(reference::validate(&package.0, &bad, &input).unwrap_err().contains("file in the reference package"));
    bad["source_geometry"] = json!("reference.nc");
    assert!(reference::validate(&package.0, &bad, &input).unwrap_err().contains("CAD or mesh"));
    let original = json_file(&package.0.join("simulation.json")).unwrap();
    for (field, value) in [
        ("complete", json!(false)),
        ("at_program_end", json!(false)),
        ("completed_operations", json!([])),
        ("stock_after_operation", json!(2)),
        ("accuracy", json!({})),
        ("accuracy", json!({"setting":"captured", "resolution_mm":0})),
        ("status", json!("unknown")),
        ("findings", json!(null)),
        ("findings", json!([{"severity":"unknown","detail":"test"}])),
    ] {
        let mut simulation = original.clone();
        simulation[field] = value;
        write(&package.0.join("simulation.json"), pretty(&simulation).unwrap()).unwrap();
        let mut updated = manifest.clone();
        hash(&package, &mut updated, "simulation.json");
        assert!(reference::validate(&package.0, &updated, &input).is_err(), "{field}");
    }
    write(&package.0.join("simulation.json"), pretty(&original).unwrap()).unwrap();
    write(&package.0.join("reference.nc"), "G21 G0 X99").unwrap();
    assert!(reference::validate(&package.0, &manifest, &input).unwrap_err().contains("artifact hash: reference.nc"));
}

#[test]
fn oracle_definite_failures_survive_inconclusive_checks() {
    use verify::Status::{Fail, Inconclusive, Pass};
    assert_eq!(combined_status([Pass, Inconclusive, Fail]), Fail);
    assert_eq!(combined_status([Fail, Inconclusive, Pass]), Fail);
    assert_eq!(combined_status([Pass, Inconclusive]), Inconclusive);
    let (package, _, mut manifest) = package(5., 5.);
    let mut simulation = json_file(&package.0.join("simulation.json")).unwrap();
    simulation["status"] = json!("fail");
    write(&package.0.join("simulation.json"), pretty(&simulation).unwrap()).unwrap();
    hash(&package, &mut manifest, "simulation.json");
    write(&package.0.join("manifest.json"), pretty(&manifest).unwrap()).unwrap();
    let result = compare_case(&package.0, &package.0.join("comparison")).unwrap();
    assert_eq!(result["status"], "fail");
    assert_eq!(result["pass"], false);
    for (severity, expected) in [("error", "fail"), ("unresolved", "inconclusive")] {
        simulation["status"] = json!("pass");
        simulation["findings"] = json!([{"severity":severity,"detail":"Original injected simulation finding"}]);
        write(&package.0.join("simulation.json"), pretty(&simulation).unwrap()).unwrap();
        hash(&package, &mut manifest, "simulation.json");
        write(&package.0.join("manifest.json"), pretty(&manifest).unwrap()).unwrap();
        let result = compare_case(&package.0, &package.0.join("comparison")).unwrap();
        assert_eq!(result["status"], expected);
        assert_eq!(result["pass"], false);
    }
}
