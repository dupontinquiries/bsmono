//! uikit-egui — the shared egui UI kit for the portfolio.
//!
//! Three things live here, each generalized out of an app that already proved
//! it:
//!
//! 1. **The carved widget surface** ([`skin`] + [`carved`]) — ported from
//!    `logscope-egui`. A five-layer sandwich (L0 egui → L1 helpers → L2 carved
//!    painters → L3 semantic widgets → L4 [`Skin`] tokens) so restyling edits
//!    values, never call sites.
//! 2. **The gesture layer** ([`gesture_layer`]) — adapts `egui::InputState` to
//!    `uikit-gesture::TwoTierGesture`, bringing snapclone's FP5-tuned swipe
//!    feel to any egui app with no per-app gesture code.
//! 3. **The theme bridge** ([`theme_bridge`]) — applies a
//!    `snapclone-theme::Palette` to an [`egui::Context`].
//!
//! See `apps/snapclone-egui` for the front-end built on this, and
//! `examples/logscope_touch.rs` for the reverse port (logscope's scene gaining
//! the touch layer).

pub mod a11y;
pub mod adaptive;
pub mod carved;
pub mod gesture_layer;
pub mod skin;
pub mod theme_bridge;
pub mod tokens;

pub use a11y::{Action as A11yAction, A11yScene, Outcome as A11yOutcome, Role, Semantics};
pub use adaptive::{Adaptive, DeviceClass, SafeArea};
pub use gesture_layer::{Axis, GestureFrame, GestureLayer};
pub use skin::Skin;
pub use theme_bridge::{apply_palette, PaletteColors};
pub use uikit_gesture::{Direction, GestureConfig, Outcome, ReleaseOutcome, TwoTierGesture};

/// Apply the theme + touch parts of cross-app [`settings_kit::Prefs`] to an egui context:
/// select the palette and, if set, widen the minimum interactive size. (The gesture part is
/// applied to a `GestureConfig` via `Prefs::apply_to_gesture`.) Call this after
/// `Settings::poll()` reports a change so the whole UI reflects new prefs live.
pub fn apply_prefs(ctx: &egui::Context, prefs: &settings_kit::Prefs) {
    let pid = snapclone_theme::PaletteId::from_str(&prefs.palette);
    apply_palette(ctx, snapclone_theme::palette_for(pid));
    if prefs.touch.min_target > 0.0 {
        let mut style = (*ctx.style()).clone();
        style.spacing.interact_size.y = style.spacing.interact_size.y.max(prefs.touch.min_target);
        ctx.set_style(style);
    }
}

use egui::{Color32, Pos2, Rect, Stroke, Ui, Vec2};

// ── L1 · common egui patterns, written once (mirrors logscope ui::row/combo) ──

/// A standard left-to-right control cluster; returns the body's value.
pub fn row<R>(ui: &mut Ui, add: impl FnOnce(&mut Ui) -> R) -> R {
    ui.horizontal(add).inner
}

/// A labelled dropdown with standard sizing; folds the ComboBox ritual.
pub fn combo(ui: &mut Ui, id: &str, selected: impl Into<String>, width: f32, body: impl FnOnce(&mut Ui)) {
    egui::ComboBox::from_id_salt(id)
        .selected_text(selected.into())
        .width(width)
        .show_ui(ui, body);
}

/// Paint the detent "barrier" — a ring on `edge` that fills with `fraction`
/// (0..1). This is the egui equivalent of snapclone's PagePeelHint/ChevronConfirm:
/// the visible "almost there" feedback as the accumulator climbs toward a
/// sub-page step. Drive it from [`GestureFrame::barrier_fraction`].
pub fn paint_barrier(ui: &Ui, surface: Rect, fraction: f32, edge: Direction, accent: Color32) {
    if fraction <= 0.01 {
        return;
    }
    let r = 14.0;
    let cx = match edge {
        Direction::Next => surface.right() - 28.0,
        Direction::Prev => surface.left() + 28.0,
    };
    let center = Pos2::new(cx, surface.center().y);
    let painter = ui.painter_at(surface);

    // faint track
    painter.circle_stroke(center, r, Stroke::new(3.0, accent.gamma_multiply(0.25)));
    // filled arc proxied by an alpha-scaled ring (egui has no arc primitive;
    // the app's real impl draws a true arc — this keeps the kit dependency-free)
    painter.circle_stroke(center, r, Stroke::new(3.0 * fraction, accent));

    // a chevron pointing the travel direction, brightening with fraction
    let dir = match edge {
        Direction::Next => 1.0,
        Direction::Prev => -1.0,
    };
    let col = accent.gamma_multiply(0.4 + 0.6 * fraction);
    let s = 5.0;
    let tip = center + Vec2::new(dir * s, 0.0);
    painter.line_segment([center + Vec2::new(-dir * s, -s), tip], Stroke::new(2.0, col));
    painter.line_segment([center + Vec2::new(-dir * s, s), tip], Stroke::new(2.0, col));
}
