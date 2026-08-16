//! mixer_panel — per-track balance for the arrangement.
//!
//! Three controls a track needs before an export is a mix and not a sum:
//! a fader (`Op::SetTrackGain`), mute (`Op::SetTrackFlags` — persisted, on
//! the wire, in undo), and solo — which is deliberately NOT an op. Solo is
//! monitoring state, the same species as `studio-engine`'s cue level: it
//! answers "what am I listening to right now", never "what is this
//! project", so it lives on the local `Studio` and only dirties the render.
//! A solo that replayed into a collaborator's project — or into your own
//! undo history — is the classic DAW-clone mistake this codebase's command
//! docs already warn about.
//!
//! ## Faders commit on release
//!
//! A slider emits a value every frame of a drag; an op per frame would be
//! sixty undo entries and sixty packets per gesture — the exact flood the
//! grid's `Paint` exists to prevent, one layer up. So the drag paints a
//! LOCAL preview (`Studio::mixer_drag`), the render follows it live via the
//! dirty flag, and one `SetTrackGain` lands when the finger lifts: one
//! gesture, one op, one undo step.

use uikit_egui::PaletteColors;

use crate::Studio;

pub fn show(ui: &mut egui::Ui, s: &mut Studio, c: &PaletteColors) {
    let tracks: Vec<studio_core::Track> = s.session.doc.project.tracks.clone();
    if tracks.is_empty() {
        ui.colored_label(c.text_weak, "No tracks.");
        return;
    }
    for t in &tracks {
        ui.horizontal(|ui| {
            let kind = match t.kind {
                studio_core::TrackKind::Audio => "♪",
                studio_core::TrackKind::Video => "▣",
            };
            ui.label(format!("{kind} {}", t.name));

            // Mute is a project fact: an op, so it undoes and travels.
            let mut audible = !t.muted;
            if ui.checkbox(&mut audible, "M").on_hover_text("muted (an edit)").changed() {
                s.edit(studio_core::Op::SetTrackFlags {
                    id: t.id, muted: !audible, locked: t.locked,
                }, if audible { "unmuted" } else { "muted" });
            }

            // Solo is a listening fact: local, dirty-only.
            let soloed = s.solo.contains(&t.id);
            if ui.add(egui::Button::selectable(soloed, "S"))
                .on_hover_text("solo (monitoring only — not an edit)").clicked()
            {
                if soloed { s.solo.retain(|id| *id != t.id); }
                else { s.solo.push(t.id); }
                s.arrangement_dirty = true;
            }

            // The fader: preview locally while dragging, one op on release.
            let mut shown = match s.mixer_drag {
                Some((id, v)) if id == t.id => v,
                _ => t.gain,
            };
            let resp = ui.add(egui::Slider::new(&mut shown, 0.0..=1.5)
                .text("gain").custom_formatter(|v, _| {
                    if v <= 0.0 { "-inf dB".into() }
                    else { format!("{:+.1} dB", 20.0 * v.log10()) }
                }));
            if resp.dragged() {
                // Visual preview only: the AUDIO follows on release, because
                // "audible while dragging" here would mean a full arrangement
                // re-render per frame (this shell's whole-buffer trade), and
                // the project wouldn't even hold the preview value to render
                // from. A live fader ride is the streaming DeviceEngine's
                // job, named as future work — not something to fake badly.
                s.mixer_drag = Some((t.id, shown));
            }
            if resp.drag_stopped() {
                s.mixer_drag = None;
                s.edit(studio_core::Op::SetTrackGain { id: t.id, gain: shown },
                       "gain");
            } else if resp.changed() && !resp.dragged() {
                // Keyboard / click-set: no drag lifecycle, commit at once.
                s.edit(studio_core::Op::SetTrackGain { id: t.id, gain: shown },
                       "gain");
            }
        });
    }
    if !s.solo.is_empty() {
        ui.add_space(4.0);
        ui.colored_label(c.warning,
            format!("solo: {} track(s) — monitoring only, exports follow it",
                    s.solo.len()));
    }
}
