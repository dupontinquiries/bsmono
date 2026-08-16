//! Confirms the FULL egui_kittest snapshot pipeline (harness → render →
//! compare-or-write a baseline PNG) actually works in this sandbox, not
//! just that the harness constructs. Deliberately TEXT-FREE: a font
//! substitution difference on a different machine (this baseline was
//! generated in one specific sandboxed environment) would make a
//! text-bearing snapshot flaky in a way that has nothing to do with a
//! real regression — a colored rectangle has no such risk, so this proves
//! the mechanism itself before anyone trusts it with real UI content.
//!
//! To update the baseline after an intentional change: `UPDATE_SNAPSHOTS=1
//! cargo test --ignore-rust-version -p aperture-egui --test kittest_snapshot`.
use egui::{Color32, Rect, Sense, Vec2};
use egui_kittest::Harness;

#[test]
fn a_solid_rect_matches_its_committed_baseline() {
    let mut harness = Harness::new_ui(|ui| {
        let (rect, _) = ui.allocate_exact_size(Vec2::new(64.0, 64.0), Sense::hover());
        ui.painter().rect_filled(Rect::from_min_size(rect.min, Vec2::new(64.0, 64.0)), 0.0, Color32::from_rgb(80, 140, 220));
    });
    harness.run();
    harness.snapshot("solid_rect");
}
