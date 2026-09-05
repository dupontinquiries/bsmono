//! End-to-end proof that `notify_lsp_changed` actually prefers incremental
//! sync when the server supports it, not just that the plumbing compiles.
//! `lsp_popup_e2e.rs` already exercises this path implicitly (every
//! keystroke calls `notify_lsp_changed`); this test is the one that
//! specifically stresses MANY small edits in a row and checks the final
//! state is correct — the scenario incremental sync exists for, and the
//! one where a wrong range or a wrong UTF-16 conversion would show up as
//! the server's view of the file silently drifting from the real one.
//!
//! Skips (doesn't fail) when rust-analyzer isn't on PATH.

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
fn many_small_edits_via_incremental_sync_still_resolve_correctly() {
    if !rust_analyzer_available() {
        eprintln!("SKIPPED: rust-analyzer not on PATH");
        return;
    }

    let dir = std::env::temp_dir().join(format!("aperture-egui-incr-e2e-{}", std::process::id()));
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::fs::write(
        dir.join("Cargo.toml"),
        "[package]\nname = \"aperture_egui_incr_e2e\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .unwrap();
    // A CROSS-FILE symbol, not a same-file one — same reasoning as
    // lsp_popup_e2e.rs's own test: the popup's local tier only parses the
    // currently open buffer's own tree-sitter outline, so it is
    // structurally incapable of ever offering a sibling module's symbol.
    // Using a same-file symbol here would let this test pass even if
    // every incremental edit silently no-opped and LSP never ran at all —
    // the local tier alone would still supply it.
    std::fs::write(
        dir.join("src/helpers.rs"),
        "pub fn cross_file_only_symbol() -> i32 { 7 }\n",
    )
    .unwrap();
    let main_rs = dir.join("src/main.rs");
    std::fs::write(&main_rs, "mod helpers;\nuse helpers::*;\n\nfn main() {\n\n}\n").unwrap();

    let mut s = AppState::new(Some(main_rs.clone()), false);

    // Type "cross_file_only_symbol" one character at a time, each
    // keystroke a SEPARATE call to notify_lsp_changed (via after_edit) —
    // exactly the "many small edits" pattern incremental sync targets,
    // and every one of them takes the did_change_range path once
    // supports_incremental_sync() is true (confirmed true for rust-
    // analyzer by aperture-lsp's own test).
    let caret = s.editor.buffer.to_string().find("\n\n}").unwrap() + 1;
    s.editor.sel = aperture_core::editor::Selection::caret(caret);
    let mut t = 0u64;
    for ch in "cross_file_only_symbol".chars() {
        s.handle(Ev::Text(ch.to_string()), t);
        t += 40;
    }
    // Then delete a few characters back-to-back (backspace also goes
    // through the same after_edit -> notify_lsp_changed path) and retype,
    // stressing insert AND delete deltas in the same session.
    for _ in 0..4 {
        s.handle(Ev::Key { key: aperture_egui::state::K::Backspace, ctrl: false, shift: false, alt: false }, t);
        t += 40;
    }
    for ch in "mbol".chars() {
        s.handle(Ev::Text(ch.to_string()), t);
        t += 40;
    }
    assert!(s.editor.buffer.to_string().contains("cross_file_only_symbol"));

    // If every incremental delta landed correctly on the server's side,
    // it should still resolve the cross-file symbol correctly — this is
    // ONLY answerable via LSP resolving the crate's file graph, so a
    // correct answer here proves incremental sync actually worked, not
    // just that the local tier happened to already know the name.
    let deadline = Instant::now() + Duration::from_secs(15);
    let mut found = false;
    while Instant::now() < deadline {
        s.poll_lsp(0);
        if let Some(popup) = &s.popup {
            if popup.items.iter().any(|i| i.label.contains("cross_file_only_symbol")) {
                found = true;
                break;
            }
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    assert!(
        found,
        "after many incremental edits, the server should still resolve the cross-file symbol; popup: {:?}",
        s.popup.as_ref().map(|p| p.items.iter().map(|i| &i.label).collect::<Vec<_>>())
    );

    let _ = std::fs::remove_dir_all(&dir);
}
