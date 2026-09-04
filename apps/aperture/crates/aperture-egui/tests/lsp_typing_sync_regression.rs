//! Regression test for a real bug found while building the hover overlay:
//! `type_text`'s plain single-cursor path inlined three of `after_edit`'s
//! six responsibilities and silently never called the other three —
//! including `notify_lsp_changed`, meaning `didChange` was NEVER sent to
//! the LSP server for an ordinary keystroke. Every other LSP e2e test in
//! this session happened to still pass despite this, because they all ask
//! for completions/hover at a position whose SURROUNDING SCOPE is already
//! correct from `didOpen` alone — an unfiltered "what's in scope here"
//! answer doesn't need the exact typed prefix to have been communicated.
//!
//! This test is deliberately built so it CANNOT pass that way: it defines
//! a brand new local variable, by typing its name character by character,
//! then asks for completions against a prefix of THAT EXACT name from a
//! different line. The only way the server can know that name exists at
//! all is if the keystrokes that spelled it were actually synced.

use aperture_egui::state::{AppState, Ev};
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
fn plain_single_cursor_typing_actually_syncs_to_the_lsp_server() {
    if !rust_analyzer_available() {
        eprintln!("SKIPPED: rust-analyzer not on PATH");
        return;
    }

    let dir = std::env::temp_dir().join(format!("aperture-egui-typing-sync-e2e-{}", std::process::id()));
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::fs::write(
        dir.join("Cargo.toml"),
        "[package]\nname = \"aperture_egui_typing_sync_e2e\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .unwrap();
    let main_rs = dir.join("src/main.rs");
    std::fs::write(&main_rs, "fn main() {\n\n\n}\n").unwrap();

    let mut s = AppState::new(Some(main_rs.clone()), false);
    assert!(s.extra_cursors.is_empty(), "this test is specifically about the SINGLE-cursor typing path");

    // Line 1 (0-indexed): define a brand new local variable by typing its
    // FULL declaration one character at a time — a name that cannot
    // possibly already be known to the server from didOpen, since it did
    // not exist in the file written to disk.
    let decl_pos = s.editor.buffer.pos_to_char(aperture_core::buffer::Pos::new(1, 0));
    s.editor.sel = aperture_core::editor::Selection::caret(decl_pos);
    let mut t = 0u64;
    for ch in "let totally_novel_local_xyz = 5;".chars() {
        s.handle(Ev::Text(ch.to_string()), t);
        t += 20;
    }

    // Line 2: type a prefix of that SAME name. If (and only if) every one
    // of those keystrokes on line 1 was actually synced, the server knows
    // "totally_novel_local_xyz" is a local in scope here and offers it.
    let line2_pos = s.editor.buffer.pos_to_char(aperture_core::buffer::Pos::new(2, 0));
    s.editor.sel = aperture_core::editor::Selection::caret(line2_pos);
    for ch in "totally_novel".chars() {
        s.handle(Ev::Text(ch.to_string()), t);
        t += 20;
    }

    let deadline = Instant::now() + Duration::from_secs(15);
    let mut found = false;
    while Instant::now() < deadline {
        s.poll_lsp(0);
        if let Some(popup) = &s.popup {
            if popup.items.iter().any(|i| i.label.contains("totally_novel_local_xyz")) {
                found = true;
                break;
            }
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    assert!(
        found,
        "the server should know about a local variable whose FULL declaration was typed \
         character-by-character on the single-cursor path; popup: {:?}",
        s.popup.as_ref().map(|p| p.items.iter().map(|i| &i.label).collect::<Vec<_>>())
    );

    let _ = std::fs::remove_dir_all(&dir);
}
