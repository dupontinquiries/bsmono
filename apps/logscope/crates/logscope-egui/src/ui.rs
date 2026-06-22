//! ui.rs — the app's stable widget surface, and the map of the UI "sandwich".
//!
//! The chrome is built in five layers, bottom to top. The point of the stack is that a *style* iteration
//! (the thing we do most) only ever touches the bottom-most style layer — never the call sites — so it costs
//! a few edits instead of a sweep, and the names the app calls don't rot:
//!
//! ```text
//!   L0  egui / eframe            the raw immediate-mode toolkit
//!   L1  egui-pattern helpers     ui::row, ui::combo, ui::cluster — the repetitive egui idioms, written once
//!   L2  widget rendering         theme::paint_carved / carved_label — the carved-material look (the HOW)
//!   L3  app widget surface  ◄──  ui::button / button_accent / primary / pill / chip / panel  (what app calls)
//!   L4  style tokens             theme::Skin — WHAT the look is, in one swappable struct (the stable API)
//! ```
//!
//! Call sites live against **L3** (`ui::…`). When we restyle, we edit **L4** (`theme::Skin`) — values only;
//! L2 reads them, L3 is unchanged, call sites are unchanged. New look, no ripple.
//!
//! Migration is incremental: existing `theme::carved_button(…)` call sites keep working, and can move to
//! `ui::button(…)` whenever they're touched. Both resolve to the same L2 implementation.

#![allow(dead_code)] // some surface is provided ahead of its first call site

use egui::Ui;

// ── L3 · stable semantic widgets ────────────────────────────────────────────────────────────────────────
// Thin aliases over the L2 implementations. These NAMES are the contract the rest of the app codes against;
// the implementations behind them can be re-skinned freely (via L4) without changing a single call site.
pub use crate::theme::{
    action_button as primary,         // the prominent call-to-action (label, accent, enabled) -> Response
    carved_button as button,          // a plain carved button (text) -> Response
    carved_button_tinted as button_accent, // a carved button tinted toward an accent (text, accent) -> Response
    chip,                             // a small static tag (text, color)
    panel,                            // a titled, optionally-copyable content panel
    pill,                             // a rounded toggle-ish label (text, color, filled) -> Response
};

// ── L4 · style tokens (re-exported so callers can read/swap the look through one name) ───────────────────
pub use crate::theme::{set_skin, skin, Skin};

// ── L1 · common egui patterns, written once ─────────────────────────────────────────────────────────────

/// A standard left-to-right control cluster; returns the body's value.
pub fn row<R>(ui: &mut Ui, add: impl FnOnce(&mut Ui) -> R) -> R {
    ui.horizontal(add).inner
}

/// A right-to-left cluster (for header trailing controls); returns the body's value.
pub fn row_rtl<R>(ui: &mut Ui, add: impl FnOnce(&mut Ui) -> R) -> R {
    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), add).inner
}

/// A labelled dropdown with the app's standard sizing. `selected` is the text shown collapsed; `body` fills
/// the menu (typically a column of `ui.selectable_value(…)`). Folds away the `ComboBox::from_id_salt` ritual.
pub fn combo(ui: &mut Ui, id: &str, selected: impl Into<String>, width: f32, body: impl FnOnce(&mut Ui)) {
    egui::ComboBox::from_id_salt(id)
        .selected_text(selected.into())
        .width(width)
        .show_ui(ui, body);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn carved_skin_tokens_are_sane() {
        let s = Skin::carved();
        // opacity rises rest -> hover, and the pressed/disabled states are within range
        assert!(s.fill_rest < s.fill_hover);
        assert!(s.fill_off <= s.fill_rest && s.fill_down <= 255);
        // relief, grain, sheen and engraving are all live (non-zero) for the carved look
        assert!(s.radius > 0 && s.grain_alpha > 0 && s.bevel_lit > 0 && s.bevel_shadow > 0);
        assert!(s.sheen > 0.0 && s.sheen_speed > 0.0 && s.vein > 0.0);
        assert!(s.engrave_shadow > 0 && s.engrave_lip > 0 && (0.0..=1.0).contains(&s.accent_tint));
    }

    #[test]
    fn skin_is_swappable() {
        let mut flat = Skin::carved();
        flat.sheen = 0.0; flat.vein = 0.0; flat.radius = 2;
        set_skin(flat);
        assert_eq!(skin().sheen, 0.0);
        assert_eq!(skin().radius, 2);
        set_skin(Skin::carved()); // restore the shipped look for other tests
        assert!(skin().sheen > 0.0);
    }
}
