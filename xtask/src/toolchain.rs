//! Check cheap toolchain prerequisites before compiling and running the full test suite.

use std::path::Path;
use std::process::Command;

pub const WASM_TARGET: &str = "wasm32-unknown-unknown";

pub fn require_wasm_target() -> Result<(), String> {
    let rustc = std::env::var_os("RUSTC").unwrap_or_else(|| "rustc".into());
    let output = Command::new(rustc)
        .args(["--print", "target-libdir", "--target", WASM_TARGET])
        .output()
        .map_err(|e| format!("checking {WASM_TARGET}: could not run rustc: {e}"))?;
    if !output.status.success() {
        return Err(format!("checking {WASM_TARGET}: {}", String::from_utf8_lossy(&output.stderr).trim()));
    }
    let directory = std::str::from_utf8(&output.stdout).map_err(|e| format!("rustc target library path: {e}"))?.trim();
    let install = match std::env::var("RUSTUP_TOOLCHAIN").ok().filter(|name| !name.is_empty()) {
        Some(name) => format!("rustup target add --toolchain {name} {WASM_TARGET}"),
        None => format!("rustup target add {WASM_TARGET}"),
    };
    require_stdlib(Path::new(directory), &install)
}

fn require_stdlib(directory: &Path, install: &str) -> Result<(), String> {
    let missing =
        || format!("{WASM_TARGET} standard library is not installed for the active Rust toolchain; run `{install}` before `cargo xtask ci`");
    let entries = match std::fs::read_dir(directory) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Err(missing()),
        Err(e) => return Err(format!("cannot read {}: {e}", directory.display())),
    };
    let (mut core, mut std) = (false, false);
    for entry in entries {
        let entry = entry.map_err(|e| format!("cannot read {}: {e}", directory.display()))?;
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name.ends_with(".rlib") {
            core |= name.starts_with("libcore-");
            std |= name.starts_with("libstd-");
        }
    }
    if core && std { Ok(()) } else { Err(missing()) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicUsize, Ordering};

    static NEXT: AtomicUsize = AtomicUsize::new(0);

    struct Scratch(PathBuf);

    impl Scratch {
        fn new() -> Self {
            Self(std::env::temp_dir().join(format!("solidcraft-toolchain-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed))))
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
            let _ = std::fs::remove_file(&self.0);
        }
    }

    #[test]
    fn missing_target_reports_how_to_install_for_the_active_toolchain() {
        let scratch = Scratch::new();
        let install = "rustup target add --toolchain stable wasm32-unknown-unknown";
        let error = require_stdlib(&scratch.0, install).unwrap_err();
        assert!(error.contains(install), "{error}");
    }

    #[test]
    fn an_incomplete_install_is_refused() {
        let scratch = Scratch::new();
        std::fs::create_dir(&scratch.0).unwrap();
        std::fs::write(scratch.0.join("libcore-test.rlib"), []).unwrap();
        assert!(require_stdlib(&scratch.0, "install").is_err());
        std::fs::write(scratch.0.join("libstd-test.rlib"), []).unwrap();
        assert!(require_stdlib(&scratch.0, "install").is_ok());
    }

    #[test]
    fn an_invalid_library_path_reports_the_filesystem_error() {
        let scratch = Scratch::new();
        std::fs::write(&scratch.0, []).unwrap();
        let error = require_stdlib(&scratch.0, "install").unwrap_err();
        assert!(error.contains("cannot read"), "{error}");
    }
}
