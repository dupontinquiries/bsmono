//! End-to-end through AppState -> LspWorker (real thread) -> a REAL
//! rust-analyzer -> `AppState.status`. aperture-lsp's own
//! `hover_reports_a_type_signature_for_a_real_function` proves the client
//! and parsing; this proves the shell's Ctrl+I binding actually reaches it.
//!
//! Skips (doesn't fail) when rust-analyzer isn't on PATH, same convention
//! as every other real-server test in this session.

use aperture_egui::state::{AppState, Ev, K};
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
fn ctrl_i_shows_a_type_signature_in_the_status_bar() {
    if !rust_analyzer_available() {
        eprintln!("SKIPPED: rust-analyzer not on PATH");
        return;
    }

    let dir = std::env::temp_dir().join(format!("aperture-egui-hover-e2e-{}", std::process::id()));
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::fs::write(
        dir.join("Cargo.toml"),
        "[package]\nname = \"aperture_egui_hover_e2e\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .unwrap();
    let main_rs = dir.join("src/main.rs");
    let source = "fn compute_total() -> i32 { 42 }\n\nfn main() {\n    let x = compute_total();\n    let _ = x;\n}\n";
    std::fs::write(&main_rs, source).unwrap();

    let mut s = AppState::new(Some(main_rs.clone()), false);
    // Caret inside "compute_total(" on the call site, line 3, character 14.
    s.editor.sel = aperture_core::editor::Selection::caret(
        s.editor.buffer.pos_to_char(aperture_core::buffer::Pos::new(3, 14)),
    );

    s.handle(Ev::Key { key: K::I, ctrl: true, shift: false, alt: false }, 0);

    let deadline = Instant::now() + Duration::from_secs(30);
    let mut found = false;
    let mut last_retry = Instant::now();
    while Instant::now() < deadline {
        s.poll_lsp(0);
        if s.status.as_ref().is_some_and(|(msg, _)| msg.contains("i32") && !msg.contains("nothing found")) {
            found = true;
            break;
        }
        // Same "the very first request right after didOpen can race ahead
        // of the server" characteristic already confirmed for completions,
        // definitions, and now here — retry the actual keypress rather
        // than treat one empty/failed answer as final.
        if s.status.as_ref().is_some_and(|(msg, _)| msg.contains("nothing found"))
            && last_retry.elapsed() > Duration::from_millis(800)
        {
            s.handle(Ev::Key { key: K::I, ctrl: true, shift: false, alt: false }, 0);
            last_retry = Instant::now();
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    assert!(found, "expected the status bar to show a hover with 'i32' in it; last status: {:?}", s.status);

    let _ = std::fs::remove_dir_all(&dir);
}
