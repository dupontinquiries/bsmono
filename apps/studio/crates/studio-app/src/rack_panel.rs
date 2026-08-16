//! rack_panel — the beatmaking surface, wired.
//!
//! `studio-egui` deliberately ships views that own nothing (`grid`, `lanes`
//! return edits); `studio-live` deliberately ships a model that plays nothing
//! (`render_pattern` is a pure function). This panel is where those halves
//! finally meet a user: it draws the views, applies their edits to the
//! `Studio`'s rack, and flags `loop_dirty` so the shell re-renders what the
//! sink is looping. Logic stays out — everything decidable (bounce ids, swing
//! math, mod semantics) already lives in a testable crate, and the one rule
//! `studio-app`'s own docs set is "the shell wires, it does not decide."

use studio_egui::grid;
use studio_egui::lanes::{self, Lane};
use uikit_egui::PaletteColors;

use crate::Studio;

/// One frame of the rack panel. `playhead` is the loop column the audition
/// sink reports (already wrapped), or `None` when stopped.
pub fn show(ui: &mut egui::Ui, s: &mut Studio, c: &PaletteColors, playhead: Option<u32>) {
    let before = s.rack.clone();

    // ── transport & pattern row ─────────────────────────────────────────
    ui.horizontal(|ui| {
        let label = if s.rack_playing { "stop" } else { "play loop" };
        if ui.button(label).clicked() {
            s.rack_playing = !s.rack_playing;
            s.status = if s.rack_playing { "loop playing".into() } else { "loop stopped".into() };
        }
        ui.separator();
        ui.label("bpm");
        ui.add(egui::DragValue::new(&mut s.rack.tempo_bpm).range(60.0..=200.0).speed(0.25));
        ui.label("swing");
        ui.add(egui::Slider::new(&mut s.rack.swing, 0.0..=0.5).show_value(false));
        ui.label(format!("{:.0}%", s.rack.swing * 100.0));
        ui.separator();

        let n = s.rack.patterns.len();
        if ui.button("◀").clicked() && n > 0 {
            s.current_pattern = (s.current_pattern + n - 1) % n;
        }
        let name = s.rack.patterns.get(s.current_pattern)
            .map(|p| p.name.clone()).unwrap_or_else(|| "—".into());
        ui.label(name);
        if ui.button("▶").clicked() && n > 0 {
            s.current_pattern = (s.current_pattern + 1) % n;
        }
        if ui.button("+").clicked() {
            let ch = s.rack.channels.len();
            s.rack.patterns.push(studio_live::Pattern::new(&format!("Pat {}", n + 1), 16, ch));
            s.current_pattern = s.rack.patterns.len() - 1;
        }
        ui.separator();
        if ui.button("bounce → timeline").clicked() {
            s.pending_bounce = true;
        }
    });

    let Some(pattern) = s.rack.patterns.get(s.current_pattern) else {
        ui.colored_label(c.text_weak, "No pattern. Press + to add one.");
        return;
    };
    let pattern = pattern.clone(); // the views take &Pattern; edits come back as values

    // ── the grid ────────────────────────────────────────────────────────
    let edits = grid::grid(ui, &pattern, &s.rack.channels, &mut s.paint, playhead, c);
    if let Some(p) = s.rack.patterns.get_mut(s.current_pattern) {
        for e in edits {
            match e {
                grid::GridEdit::SetStep { channel, step, on } => p.set(channel, step, on),
                grid::GridEdit::ClearChannel { channel } => {
                    if let Some(r) = p.rows.get_mut(channel) { *r = 0; }
                }
                grid::GridEdit::ToggleMute { channel } => {
                    if let Some(ch) = s.rack.channels.get_mut(channel) { ch.muted = !ch.muted; }
                }
            }
        }
    }

    // ── the mod lane for the selected channel ───────────────────────────
    ui.add_space(4.0);
    ui.horizontal(|ui| {
        ui.label("lane:");
        let kind = s.rack.channels.get(s.selected_channel).map(|ch| ch.kind.clone());
        for l in Lane::ALL {
            let usable = kind.as_ref().map(|k| l.applies_to(k)).unwrap_or(false);
            let sel = s.lane == l;
            if ui.add_enabled(usable, egui::Button::selectable(sel, l.label())).clicked() {
                s.lane = l;
            }
        }
    });
    // A pitch lane left selected while switching to a sampler channel would
    // edit a no-op; snap back to velocity instead of lying.
    if let Some(ch) = s.rack.channels.get(s.selected_channel) {
        if !s.lane.applies_to(&ch.kind) { s.lane = Lane::Vel; }
    }
    let lane_edits = lanes::lane(ui, &pattern, s.selected_channel, s.lane, c);
    if let Some(p) = s.rack.patterns.get_mut(s.current_pattern) {
        for e in lane_edits { p.set_mod(e.channel, e.step, e.m); }
    }

    // ── channel strip ───────────────────────────────────────────────────
    ui.add_space(6.0);
    let mut selected = s.selected_channel;
    for (i, ch) in s.rack.channels.iter_mut().enumerate() {
        ui.horizontal(|ui| {
            if ui.add(egui::Button::selectable(selected == i, &ch.name)).clicked() {
                selected = i;
            }
            let mut audible = !ch.muted;
            if ui.checkbox(&mut audible, "").on_hover_text("unmuted").changed() {
                ch.muted = !audible;
            }
            ui.add(egui::Slider::new(&mut ch.gain, 0.0..=1.2).show_value(false).text("gain"));
            // Choke group as a small number, 0 = none: channels sharing a
            // nonzero group cut each other (the hats ship in group 1).
            let mut grp = ch.choke_group.map(|g| g as i32 + 1).unwrap_or(0);
            ui.label("choke");
            if ui.add(egui::DragValue::new(&mut grp).range(0..=8)).changed() {
                ch.choke_group = (grp > 0).then(|| (grp - 1) as u8);
            }
            match &mut ch.kind {
                studio_live::SynthKind::Sub { note, glide_ms, drive, decay_ms, .. } => {
                    ui.label("note");
                    ui.add(egui::DragValue::new(note).range(-24..=24));
                    ui.label("glide");
                    ui.add(egui::DragValue::new(glide_ms).range(0.0..=400.0).suffix(" ms"));
                    ui.add(egui::Slider::new(drive, 0.0..=1.0).show_value(false).text("drive"));
                    ui.add(egui::DragValue::new(decay_ms).range(40.0..=2000.0).suffix(" ms"));
                }
                studio_live::SynthKind::Drum { tone, decay_ms, .. } => {
                    ui.add(egui::Slider::new(tone, 0.0..=1.0).show_value(false).text("tone"));
                    ui.add(egui::DragValue::new(decay_ms).range(20.0..=1500.0).suffix(" ms"));
                }
                studio_live::SynthKind::Sine { hz } => {
                    ui.add(egui::DragValue::new(hz).range(20.0..=12_000.0).suffix(" Hz"));
                }
                _ => {}
            }
        });
    }
    s.selected_channel = selected.min(s.rack.channels.len().saturating_sub(1));

    if s.rack != before {
        s.loop_dirty = true;
    }
}
