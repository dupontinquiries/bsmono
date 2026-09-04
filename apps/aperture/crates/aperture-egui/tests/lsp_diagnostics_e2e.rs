//! End-to-end through AppState -> LspWorker (real thread) -> a REAL
//! rust-analyzer -> `AppState::lsp_diagnostics`. aperture-lsp's own
//! `a_real_type_error_produces_a_real_diagnostic` proves the client and
//! parsing; this proves the worker actually surfaces what it pushes, using
//! its own per-tick poll rather than the client's cache directly.
//!
//! Deliberately does NOT test that fixing the error clears the
//! diagnostic -- aperture-lsp's own commit adding diagnostics support
//! investigated exactly that with an isolated repro and found a SECOND
//! didChange's content genuinely not reflected in a later diagnostics
//! update in this sandbox, even after 40+ seconds. Shipping a test on top
//! of an already-named, unresolved gap would just be a second flaky test
//! standing on the first one's unstable ground.
//!
//! Skips (doesn't fail) when rust-analyzer isn't on PATH, same convention
//! as every other real-server test in this session.

use aperture_egui::state::AppState;
use std::time::{Duration, Instant};

fn rust_analyzer_available() -> bool {
    std::process::Command::new("rust-analyzer")
        .arg("--version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

#[test]
fn a_real_type_error_surfaces_in_appstate_lsp_diagnostics() {
    if !rust_analyzer_available() {
        eprintln!("SKIPPED: rust-analyzer not on PATH");
        return;
    }

    let dir = std::env::temp_dir().join(format!("aperture-egui-diag-e2e-{}", std::process::id()));
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::fs::write(
        dir.join("Cargo.toml"),
        "[package]\nname = \"aperture_egui_diag_e2e\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .unwrap();
    let main_rs = dir.join("src/main.rs");
    std::fs::write(&main_rs, "fn main() {\n    let x: i32 = \"not a number\";\n    let _ = x;\n}\n").unwrap();

    let mut s = AppState::new(Some(main_rs.clone()), false);

    let deadline = Instant::now() + Duration::from_secs(30);
    let mut found_error = false;
    while Instant::now() < deadline {
        s.poll_lsp();
        if s.lsp_diagnostics.iter().any(|d| d.severity == aperture_lsp::DiagnosticSeverity::Error) {
            found_error = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    assert!(
        found_error,
        "expected an Error-severity diagnostic to reach AppState::lsp_diagnostics; got: {:?}",
        s.lsp_diagnostics
    );
    assert!(
        s.lsp_diagnostics.iter().any(|d| d.start_line == 1),
        "expected it anchored on the mismatched assignment's line; got: {:?}",
        s.lsp_diagnostics
    );

    let _ = std::fs::remove_dir_all(&dir);
}
