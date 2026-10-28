//! `export-snapshot`: a design saved by the CLI, written as the exchange snapshot at the
//! parameter values given, the same bytes every time; a failing feature or a bad parameter
//! fails the command and writes nothing.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};

use serde_json::{Value, json};

static NEXT: AtomicUsize = AtomicUsize::new(0);

struct Scratch(PathBuf);

impl Scratch {
    fn new() -> Self {
        let dir = std::env::temp_dir().join(format!("solidcraft-export-snapshot-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
        std::fs::create_dir(&dir).unwrap();
        Self(dir)
    }

    /// A `w` × 20 × 10 mm block with a 2 mm fillet radius `r` on one edge, saved as a design.
    fn design(&self) -> PathBuf {
        let script = self.0.join("block.json");
        let commands = json!({"commands": [
            {"command": "parameters.add", "params": {"name": "w", "expression": "20 mm"}},
            {"command": "parameters.add", "params": {"name": "r", "expression": "2 mm"}},
            {"command": "solid.box", "params": {"length": "w", "width": 20, "height": 10}},
            {"command": "solid.fillet", "params": {"edges": [[10, 0, 10]], "radius": "r"}},
        ]});
        std::fs::write(&script, commands.to_string()).unwrap();
        let saved = self.0.join("block.sc");
        let out = cli().arg("run").arg(&script).arg("--quiet").arg("--save").arg(&saved).output().unwrap();
        assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
        saved
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn cli() -> Command {
    Command::new(env!("CARGO_BIN_EXE_solidcraft-cli"))
}

fn export(design: &Path, out: &Path, extra: &[&str]) -> std::process::Output {
    cli().arg("export-snapshot").arg(design).arg("--out").arg(out).args(extra).output().unwrap()
}

fn largest_x(snapshot: &Value) -> f64 {
    snapshot["bodies"][0]["positions"].as_array().unwrap().iter().map(|p| p[0].as_f64().unwrap()).fold(f64::MIN, f64::max)
}

#[test]
fn export_snapshot_writes_the_design_at_the_values_given() {
    let dir = Scratch::new();
    let design = dir.design();
    let (a, b, wide) = (dir.0.join("a.json"), dir.0.join("b.json"), dir.0.join("wide.json"));
    for out in [&a, &b] {
        let o = export(&design, out, &[]);
        assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    }
    let text = std::fs::read(&a).unwrap();
    assert_eq!(text, std::fs::read(&b).unwrap(), "the same bytes every time");
    let snapshot: Value = serde_json::from_slice(&text).unwrap();
    assert_eq!((snapshot["schema"].as_u64(), snapshot["unit"].as_str()), (Some(1), Some("mm")));
    let faces = snapshot["bodies"][0]["faces"].as_array().unwrap();
    assert_eq!(faces.len(), 7, "six planes and the fillet");
    assert!(faces.iter().any(|f| f["surface"]["type"] == "cylinder" && (f["surface"]["radius"].as_f64().unwrap() - 2.0).abs() < 1e-9));
    assert_eq!(largest_x(&snapshot), 20.0);

    let o = export(&design, &wide, &["--param", "w=35", "--param", "r=1.5 mm", "--relative-tolerance", "1e-4"]);
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    let wide: Value = serde_json::from_slice(&std::fs::read(&wide).unwrap()).unwrap();
    assert_eq!(largest_x(&wide), 35.0);
    let names = |s: &Value| s["bodies"][0]["faces"].as_array().unwrap().iter().map(|f| f["name"].as_str().unwrap().to_string()).collect::<Vec<_>>();
    assert_eq!(names(&snapshot), names(&wide), "face names stay");
    let w = wide["parameters"].as_array().unwrap().iter().find(|p| p["name"] == "w").unwrap();
    assert_eq!(w["value"].as_f64(), Some(35.0));
}

#[test]
fn export_snapshot_refuses_what_it_cannot_write() {
    let dir = Scratch::new();
    let design = dir.design();
    let out = dir.0.join("out.json");
    for extra in [
        &["--param", "r=50 mm"][..],
        &["--param", "nope=1"][..],
        &["--param", "w"][..],
        &["--param", "w=NaN"][..],
        &["--tolerance", "0"][..],
        &["--tolerance", "0.1", "--relative-tolerance", "1e-4"][..],
        &["--body", "Body7"][..],
        &["--color", "red"][..],
    ] {
        let o = export(&design, &out, extra);
        assert_eq!(o.status.code(), Some(1), "{extra:?}");
        assert!(!out.exists(), "{extra:?}");
    }
    // The failing fillet is named.
    let o = export(&design, &out, &["--param", "r=50 mm"]);
    assert!(String::from_utf8_lossy(&o.stderr).contains("Fillet1"), "{}", String::from_utf8_lossy(&o.stderr));
    let o = cli().arg("export-snapshot").arg(&design).output().unwrap();
    assert_eq!(o.status.code(), Some(1), "--out is required");
}
