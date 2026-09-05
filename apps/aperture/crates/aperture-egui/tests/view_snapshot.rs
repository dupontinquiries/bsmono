//! Snapshots of `view::frame` itself — the thing `kittest_snapshot.rs`'s
//! solid-rectangle test deliberately stopped short of, to prove the
//! harness→render→compare pipeline worked at all before trusting it with
//! real content.
//!
//! Text-bearing, unlike that one, and on purpose: `FontId::monospace` here
//! resolves to egui's own EMBEDDED font (Hack), not a system font — bundled
//! bytes shipped inside the `egui` crate itself, not present on the search
//! path a `fontconfig` lookup would walk. Two machines on the same `egui`
//! version render the same glyph bitmaps; the risk the other test was
//! avoiding (a system-font substitution differing across machines) doesn't
//! apply to a `FontId::monospace` galley. If this ever proves flaky in CI,
//! that's worth learning — it would mean the assumption above is wrong
//! somewhere, not just a reason to give up and go text-free.
//!
//! To update a baseline after an intentional view change: `UPDATE_SNAPSHOTS=1
//! cargo test --ignore-rust-version -p aperture-egui --test view_snapshot`.
use aperture_egui::state::AppState;
use aperture_egui::view;
use egui_kittest::Harness;

fn harness_for(state: AppState) -> Harness<'static, AppState> {
    let mut harness = Harness::new_ui_state(
        |ui, state: &mut AppState| view::frame(ui, state),
        state,
    );
    // Small and fixed: a snapshot's whole value is a stable pixel count.
    // Big enough to show the gutter, a few lines, and the status bar
    // without the scrollbar's thumb geometry (itself a function of
    // viewport height) becoming part of what's being compared.
    harness.set_size(egui::Vec2::new(400.0, 200.0));
    harness
}

#[test]
fn an_empty_buffer_matches_its_committed_baseline() {
    // The state a brand new scratch buffer starts in: one empty line, caret
    // home, no file on disk. Cheapest possible real (non-synthetic) frame.
    let mut harness = harness_for(AppState::new(None, false));
    harness.run();
    harness.snapshot("empty_buffer");
}

#[test]
fn a_small_rust_file_matches_its_committed_baseline() {
    // Enough surface to exercise syntax spans, the gutter's line numbers,
    // and the status bar's language label together — the combination the
    // solid-rectangle test can't stand in for.
    let mut state = AppState::new(None, false);
    state.lang = aperture_core::syntax::Language::Rust;
    state.syntax.set_language(state.lang);
    state.editor = aperture_core::editor::Editor::from_str(
        "fn add(a: i32, b: i32) -> i32 {\n    a + b\n}\n",
    );
    let mut harness = harness_for(state);
    harness.run();
    harness.snapshot("small_rust_file");
}

#[test]
fn a_multi_cursor_selection_matches_its_committed_baseline() {
    // The one piece of this SESSION's work with no coverage at all under
    // egui_kittest: two selections drawn at once (primary + secondary),
    // the thing the interactive Xvfb screenshots checked by eye and this
    // makes into a standing regression test instead of a one-time look.
    let mut state = AppState::new(None, false);
    state.editor = aperture_core::editor::Editor::from_str("let x = 1;\nlet x = 2;\n");
    state.editor.sel = aperture_core::editor::Selection { anchor: 4, head: 5 }; // first "x"
    state.extra_cursors = vec![aperture_core::editor::Selection { anchor: 15, head: 16 }]; // second "x"
    let mut harness = harness_for(state);
    harness.run();
    harness.snapshot("multi_cursor_selection");
}

#[test]
fn the_status_bar_hint_still_shows_at_a_normal_desktop_width() {
    // The narrow-width tests above deliberately use 400px to catch the
    // overlap; this one confirms the fix didn't just hide the hint
    // everywhere. 700px is a modest, not generous, desktop width.
    let mut harness = Harness::new_ui_state(
        |ui, state: &mut AppState| view::frame(ui, state),
        AppState::new(None, false),
    );
    harness.set_size(egui::Vec2::new(700.0, 200.0));
    harness.run();
    harness.snapshot("status_bar_wide");
}
