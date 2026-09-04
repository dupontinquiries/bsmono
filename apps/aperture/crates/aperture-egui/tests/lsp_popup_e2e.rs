//! End-to-end through AppState -> LspWorker (real thread) -> a REAL
//! rust-analyzer -> the symbol popup. The unit tests in aperture-lsp prove
//! the client and framing; completion_loop.rs proves the AI-completion
//! wiring the same way. This is that pattern applied to the third
//! completion source the popup now has.
//!
//! Skips (doesn't fail) when rust-analyzer isn't on PATH, same convention
//! as aperture-lsp's own real-server test — a missing optional external
//! tool is not this crate's regression.

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
fn opening_a_rust_file_and_typing_a_prefix_surfaces_a_cross_file_lsp_completion() {
    if !rust_analyzer_available() {
        eprintln!("SKIPPED: rust-analyzer not on PATH");
        return;
    }

    // A CROSS-FILE symbol, not a std type: the popup's local tier
    // (`gather_symbols`) parses only the CURRENTLY OPEN buffer's own
    // tree-sitter outline (confirmed by reading it — `self.syntax.
    // outline(&self.editor.buffer)`, nothing workspace-wide), so a
    // function defined in a SIBLING module main.rs never opens is
    // structurally invisible to that tier. Seeing it in the popup is only
    // possible through the LSP tier actually resolving the crate graph.
    //
    // Deliberately NOT a std type (an earlier version of this test used
    // "String"): that requires the `rust-src` component to resolve at
    // all, which this sandbox's toolchain doesn't have installed — found
    // by checking `rustc --print sysroot`'s `lib/rustlib/src`, not
    // guessed. A same-crate cross-file symbol needs no sysroot source,
    // only the crate's own file graph, which every `cargo`-shaped project
    // has by construction.
    let dir = std::env::temp_dir().join(format!("aperture-egui-lsp-e2e-{}", std::process::id()));
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::fs::write(
        dir.join("Cargo.toml"),
        "[package]\nname = \"aperture_egui_lsp_e2e\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .unwrap();
    std::fs::write(
        dir.join("src/helpers.rs"),
        "pub fn cross_file_only_symbol() -> i32 { 7 }\n",
    )
    .unwrap();
    let main_rs = dir.join("src/main.rs");
    std::fs::write(&main_rs, "mod helpers;\nuse helpers::*;\n\nfn main() {\n\n}\n").unwrap();

    let mut s = AppState::new(Some(main_rs.clone()), false);
    s.open_file(main_rs.clone(), 0);

    let caret = s.editor.buffer.to_string().find("\n\n}").unwrap() + 1;
    s.editor.sel = aperture_core::editor::Selection::caret(caret);
    let mut t = 0u64;
    for ch in "cross_file".chars() {
        s.handle(Ev::Text(ch.to_string()), t);
        t += 40;
    }

    // rust-analyzer's completion can race ahead of its own reanalysis when
    // fired immediately after a rapid burst of didChange notifications —
    // confirmed by a direct repro (aperture-lsp's own
    // debug_incremental_didchange_repro): the identical sequence of edits
    // returns zero items with no pause, and succeeds with a 1s pause
    // before the query. A real editor's typing-then-idle debounce (the
    // same one the AI ghost-completion path already uses) naturally
    // avoids this; this test has no such pause by construction (each
    // keystroke fires immediately), so it adds the equivalent one
    // explicitly rather than assume the very first answer is authoritative.
    std::thread::sleep(Duration::from_secs(2));
    s.handle(Ev::Key { key: aperture_egui::state::K::Backspace, ctrl: false, shift: false, alt: false }, t);
    t += 40;
    s.handle(Ev::Text("e".to_string()), t);

    // No std indexing needed for this one — same-crate file-graph
    // resolution is part of what rust-analyzer's initial project load
    // does regardless, so this should land closer to the "local symbol"
    // timing (~2s) than the std-indexing path, but give real margin over
    // a cold spawn plus initial project load on a loaded machine.
    let deadline = Instant::now() + Duration::from_secs(30);
    let mut found = false;
    while Instant::now() < deadline {
        s.poll_lsp();
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
        "expected 'cross_file_only_symbol' (defined in a sibling module the local \
         single-file tier can't see) in the popup; got: {:?}",
        s.popup.as_ref().map(|p| p.items.iter().map(|i| &i.label).collect::<Vec<_>>())
    );

    let _ = std::fs::remove_dir_all(&dir);
}
