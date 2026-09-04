//! End-to-end through AppState -> LspWorker (real thread) -> a REAL
//! rust-analyzer -> a caret jump, exercising the CROSS-FILE case
//! specifically (same-file jumps are the simpler path; this is the one
//! that also has to go through `open_file` and re-point `self.lsp_uri`).
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
fn f12_jumps_to_a_definition_in_a_different_file() {
    if !rust_analyzer_available() {
        eprintln!("SKIPPED: rust-analyzer not on PATH");
        return;
    }

    let dir = std::env::temp_dir().join(format!("aperture-egui-gotodef-e2e-{}", std::process::id()));
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::fs::write(
        dir.join("Cargo.toml"),
        "[package]\nname = \"aperture_egui_gotodef_e2e\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .unwrap();
    std::fs::write(
        dir.join("src/helpers.rs"),
        "pub fn cross_file_only_symbol() -> i32 { 7 }\n",
    )
    .unwrap();
    let main_rs = dir.join("src/main.rs");
    let source = "mod helpers;\nuse helpers::*;\n\nfn main() {\n    let x = cross_file_only_symbol();\n    let _ = x;\n}\n";
    std::fs::write(&main_rs, source).unwrap();

    let mut s = AppState::new(Some(main_rs.clone()), false);
    // Caret inside "cross_file_only_symbol(" on the call site, line 4,
    // character 14 (well inside the identifier).
    let call_line = source.lines().position(|l| l.contains("let x =")).unwrap();
    s.editor.sel = aperture_core::editor::Selection::caret(
        s.editor.buffer.pos_to_char(aperture_core::buffer::Pos::new(call_line, 14)),
    );

    s.handle(Ev::Key { key: K::F12, ctrl: false, shift: false, alt: false }, 0);

    let deadline = Instant::now() + Duration::from_secs(30);
    let mut jumped = false;
    let mut last_retry = Instant::now();
    while Instant::now() < deadline {
        s.poll_lsp(0);
        if s.path.as_ref().is_some_and(|p| p.ends_with("helpers.rs")) {
            jumped = true;
            break;
        }
        // A single F12 press fires exactly ONE request — this is
        // deliberate (see go_to_definition's own doc: it's a one-shot user
        // action, not a re-armed loop like completions). The very first
        // request right after didOpen can genuinely come back empty before
        // rust-analyzer has resolved the cross-file reference (confirmed
        // with an isolated raw-client repro: identical query, empty on the
        // first attempt, succeeds ~1s later) — a real person would just
        // press F12 again, so the test does the same rather than treating
        // one empty answer as final.
        if s.status.as_ref().is_some_and(|(msg, _)| msg.contains("nothing found"))
            && last_retry.elapsed() > Duration::from_millis(800)
        {
            s.handle(Ev::Key { key: K::F12, ctrl: false, shift: false, alt: false }, 0);
            last_retry = Instant::now();
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    assert!(jumped, "expected F12 to open helpers.rs; still at {:?}", s.path);
    assert_eq!(s.editor.caret_pos().line, 0, "the definition is on helpers.rs's first line");
    assert!(
        s.editor.buffer.to_string().contains("cross_file_only_symbol"),
        "sanity: the buffer that loaded is really helpers.rs"
    );

    let _ = std::fs::remove_dir_all(&dir);
}
