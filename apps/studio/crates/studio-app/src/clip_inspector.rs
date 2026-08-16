//! clip_inspector — what the selected clip is, and what it runs through.
//!
//! `Clip` has carried `gain` and `effects` since the beginning and
//! `mixdown` has always rendered both (per-clip instances, cached per clip
//! id), but nothing in the shell could edit them: the whole per-clip half of
//! the signal path was reachable only by writing ops in a test. This panel
//! is that door.
//!
//! ## Why per-clip effects exist at all, next to bus chains
//!
//! A bus chain treats everything that reaches it the same way — that is what
//! a bus IS. But "this one ad-lib needs a filter sweep and the other twelve
//! do not" is a per-clip fact, and routing that ad-lib to its own bus to
//! express it would be inventing structure to hold a single decision. The
//! model already made this distinction; the UI just never showed it.
//!
//! ## Same discipline as everywhere else here
//!
//! Faders commit one op on release (a value per drag frame would be an op
//! and a packet per frame — `Paint`'s rationale, three panels running). The
//! effect catalog is `mixer_panel::ADDABLE_FX`, deliberately shared rather
//! than re-listed: two catalogs would drift, and the one in the mixer is
//! already the curated door into `studio_fx::instantiate`'s string registry.

use uikit_egui::PaletteColors;

use crate::mixer_panel::{effect, ADDABLE_FX};
use crate::Studio;

/// Draw the inspector for the current selection. Renders nothing when
/// nothing is selected — an empty inspector panel is a permanent reminder
/// that you have not clicked anything, which is not information.
pub fn show(ui: &mut egui::Ui, s: &mut Studio, c: &PaletteColors) {
    let Some(id) = s.selection.first().copied() else { return };
    let Some(clip) = s.session.doc.project.clips.get(&id).cloned() else { return };

    let source_name = s.session.doc.project.sources.get(&clip.source)
        .map(|src| src.path.rsplit('/').next().unwrap_or(&src.path).to_string())
        .unwrap_or_else(|| format!("source {}", clip.source.0));

    ui.horizontal(|ui| {
        ui.label(format!("clip {} — {}", id.0, source_name));
        if s.selection.len() > 1 {
            // Multi-selection edits one clip, and says so rather than
            // pretending: applying a chain to twelve clips is a batch op
            // this panel has no shape for yet.
            ui.colored_label(c.text_weak,
                format!("(+{} more selected; editing the first)", s.selection.len() - 1));
        }
    });

    ui.horizontal(|ui| {
        let mut shown = match s.clip_gain_drag {
            Some((d, v)) if d == id => v,
            _ => clip.gain,
        };
        let resp = ui.add(egui::Slider::new(&mut shown, 0.0..=1.5).text("clip gain")
            .custom_formatter(|v, _| if v <= 0.0 { "-inf dB".into() }
                              else { format!("{:+.1} dB", 20.0 * v.log10()) }));
        if resp.dragged() { s.clip_gain_drag = Some((id, shown)); }
        if resp.drag_stopped() {
            s.clip_gain_drag = None;
            s.edit(studio_core::Op::SetGain { id, gain: shown }, "clip gain");
        } else if resp.changed() && !resp.dragged() {
            s.edit(studio_core::Op::SetGain { id, gain: shown }, "clip gain");
        }
    });

    ui.horizontal_wrapped(|ui| {
        ui.label("fx:");
        let mut effects = clip.effects.clone();
        let mut changed = false;
        let mut remove: Option<usize> = None;
        for (i, fx) in effects.iter_mut().enumerate() {
            // A disabled effect stays in the chain at its position — the
            // A/B a person actually wants is "with and without this one",
            // not "delete it and add it back at the end".
            let mut on = fx.enabled;
            if ui.checkbox(&mut on, &fx.kind).changed() {
                fx.enabled = on;
                changed = true;
            }
            if ui.small_button("✕").clicked() { remove = Some(i); }
        }
        if let Some(i) = remove { effects.remove(i); changed = true; }
        ui.menu_button("+ fx", |ui| {
            for (kind, params) in ADDABLE_FX {
                if ui.button(*kind).clicked() {
                    effects.push(effect(kind, params));
                    changed = true;
                    ui.close();
                }
            }
        });
        if changed {
            s.edit(studio_core::Op::SetEffects { id, effects }, "clip fx");
        }
    });
}
