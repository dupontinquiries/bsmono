//! lanes — per-step modifier editing under the grid.
//!
//! `studio-live`'s [`StepMod`] carries the trap half of a pattern: velocity
//! accents, hat-roll ratchets, 808 retunes and slides, probability. The grid
//! shows *whether* a step fires; a lane shows and edits *how*, one property at
//! a time for the selected channel — FL's graph editor, cut down to the shape
//! this rack actually stores.
//!
//! ## One lane at a time, on purpose
//!
//! Showing five stacked lanes per channel would quintuple the grid's height
//! and bury the pattern on a phone screen — the same constraint that made
//! `touch::Panel` a one-at-a-time stack. A selector costs one tap; a page of
//! sliders costs the pattern.
//!
//! ## Same rules as the grid
//!
//! Nothing here owns the pattern: every interaction returns a [`LaneEdit`]
//! for the host to apply, for exactly the reason `grid`'s module docs give —
//! an edit with no representation is invisible to a collaborator. And the
//! mapping from a pointer's height to a value lives in plain functions with
//! no `Ui` in sight, because that is the part that is easy to get subtly
//! wrong and it is only testable if nothing needs a window.

use studio_live::{Pattern, StepMod, SynthKind};

use crate::grid::{is_beat, CELL, LABEL_W};

/// Which per-step property the lane is editing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Lane {
    #[default]
    Vel,
    Ratchet,
    Tune,
    Slide,
    Prob,
}

impl Lane {
    pub const ALL: [Lane; 5] = [Lane::Vel, Lane::Ratchet, Lane::Tune, Lane::Slide, Lane::Prob];

    pub fn label(self) -> &'static str {
        match self {
            Lane::Vel => "Vel",
            Lane::Ratchet => "Roll",
            Lane::Tune => "Tune",
            Lane::Slide => "Slide",
            Lane::Prob => "Prob",
        }
    }

    /// Does this lane mean anything for the channel's synth? Slide and Tune
    /// are pitch operations; `studio-live` defines them as no-ops for
    /// `Sampler` and `Beatspace`, and a lane that edits a no-op teaches the
    /// user a lie about what the knob does.
    pub fn applies_to(self, kind: &SynthKind) -> bool {
        match self {
            Lane::Vel | Lane::Ratchet | Lane::Prob => true,
            // Tune reaches samplers too now: render_pattern repitches a
            // sampler voice's output (Placed::pitch), so the lane stopped
            // being a documented no-op there. Slide stays where glide is
            // defined — a sampler has no pitch trajectory to glide along.
            Lane::Tune => matches!(
                kind,
                SynthKind::Sub { .. } | SynthKind::Sine { .. } | SynthKind::Drum { .. }
                    | SynthKind::Sampler { .. }
            ),
            Lane::Slide => matches!(
                kind,
                SynthKind::Sub { .. } | SynthKind::Sine { .. } | SynthKind::Drum { .. }
            ),
        }
    }
}

/// One lane interaction, for the host to apply via `Pattern::set_mod`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LaneEdit {
    pub channel: usize,
    pub step: u32,
    pub m: StepMod,
}

/// The lane's value as a 0..=1 bar height, for drawing.
pub fn frac_of(lane: Lane, m: &StepMod) -> f32 {
    match lane {
        Lane::Vel => m.vel.clamp(0.0, 1.0),
        Lane::Prob => m.prob.clamp(0.0, 1.0),
        Lane::Ratchet => (m.ratchet.clamp(1, 4) - 1) as f32 / 3.0,
        Lane::Tune => (m.tune.clamp(-12, 12) as f32 + 12.0) / 24.0,
        Lane::Slide => if m.slide { 1.0 } else { 0.0 },
    }
}

/// A 0..=1 bar height back into the lane's value, applied onto `m`.
///
/// Discrete lanes snap: ratchet to its four counts, tune to semitones. The
/// snap happens HERE, not at draw time, so the value stored is the value
/// shown — a lane that displays 3 while the pattern holds 2.7 rolls would be
/// the grid disagreeing with the audio, the exact failure `grid`'s playhead
/// docs exist to prevent.
pub fn mod_with(lane: Lane, mut m: StepMod, frac: f32) -> StepMod {
    let frac = frac.clamp(0.0, 1.0);
    match lane {
        Lane::Vel => m.vel = frac,
        Lane::Prob => m.prob = frac,
        Lane::Ratchet => m.ratchet = 1 + (frac * 3.999) as u8,
        Lane::Tune => m.tune = ((frac * 24.0).round() - 12.0) as i8,
        Lane::Slide => m.slide = frac >= 0.5,
    }
    m
}

/// Slide is a toggle, not a level — clicking flips it.
pub fn toggled(lane: Lane, mut m: StepMod) -> StepMod {
    if lane == Lane::Slide {
        m.slide = !m.slide;
    }
    m
}

/// Draw one channel's lane. Returns edits for the host to apply.
pub fn lane(
    ui: &mut egui::Ui,
    pattern: &Pattern,
    channel: usize,
    lane: Lane,
    c: &uikit_egui::PaletteColors,
) -> Vec<LaneEdit> {
    let mut edits = Vec::new();
    if pattern.steps == 0 {
        return edits;
    }
    let h = 56.0;
    let width = LABEL_W + pattern.steps as f32 * CELL;
    let (rect, resp) =
        ui.allocate_exact_size(egui::vec2(width, h), egui::Sense::click_and_drag());
    let painter = ui.painter();

    painter.text(
        egui::pos2(rect.left() + 4.0, rect.top() + h * 0.5),
        egui::Align2::LEFT_CENTER,
        lane.label(),
        egui::FontId::proportional(12.0),
        c.text_weak,
    );

    for s in 0..pattern.steps {
        let x = rect.left() + LABEL_W + s as f32 * CELL;
        let col = egui::Rect::from_min_size(
            egui::pos2(x + 1.0, rect.top() + 1.0),
            egui::vec2(CELL - 2.0, h - 2.0),
        );
        painter.rect_filled(col, 2.0, if is_beat(s) { c.bg_raised } else { c.bg_base });
        let m = pattern.mod_at(channel, s);
        let frac = frac_of(lane, &m);
        let on = pattern.is_set(channel, s);
        // Off steps keep their (possibly customised) value visible but dim:
        // a mod outlives the step it was set on, and hiding it would make a
        // re-enabled step's loudness look like a surprise.
        let fill = if on { c.accent } else { c.text_weak };
        if frac > 0.0 {
            let bar_h = (h - 4.0) * frac;
            let bar = egui::Rect::from_min_max(
                egui::pos2(col.left() + 2.0, col.bottom() - 2.0 - bar_h),
                egui::pos2(col.right() - 2.0, col.bottom() - 2.0),
            );
            painter.rect_filled(bar, 2.0, fill);
        }
        // The centre line for Tune, so "no retune" reads as a resting bar
        // rather than a half-full one.
        if lane == Lane::Tune {
            let mid = col.bottom() - 2.0 - (h - 4.0) * 0.5;
            painter.line_segment(
                [egui::pos2(col.left(), mid), egui::pos2(col.right(), mid)],
                egui::Stroke::new(1.0_f32, c.border),
            );
        }
    }

    if let Some(pos) = resp.interact_pointer_pos() {
        let rel = pos - rect.min;
        if rel.x >= LABEL_W {
            let s = ((rel.x - LABEL_W) / CELL) as u32;
            if s < pattern.steps {
                let cur = pattern.mod_at(channel, s);
                let next = if lane == Lane::Slide {
                    // Click-to-toggle, and only on the click, or a drag
                    // across the lane would flip a step sixty times a second.
                    if resp.clicked() || resp.drag_started() {
                        toggled(lane, cur)
                    } else {
                        cur
                    }
                } else {
                    let frac = 1.0 - ((rel.y - 2.0) / (h - 4.0)).clamp(0.0, 1.0);
                    mod_with(lane, cur, frac)
                };
                if next != cur {
                    edits.push(LaneEdit { channel, step: s, m: next });
                }
            }
        }
    }
    edits
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_full_bar_is_the_value_it_draws() {
        // Store-what-you-show: frac -> value -> frac must round-trip for the
        // continuous lanes, or the bar height lies about the pattern.
        for lane in [Lane::Vel, Lane::Prob] {
            for frac in [0.0, 0.25, 0.5, 1.0] {
                let m = mod_with(lane, StepMod::default(), frac);
                assert!((frac_of(lane, &m) - frac).abs() < 1e-6, "{lane:?} at {frac}");
            }
        }
    }

    #[test]
    fn ratchet_snaps_to_its_four_counts() {
        let r = |f: f32| mod_with(Lane::Ratchet, StepMod::default(), f).ratchet;
        assert_eq!(r(0.0), 1);
        assert_eq!(r(0.3), 2);
        assert_eq!(r(0.6), 3);
        assert_eq!(r(1.0), 4, "the top of the lane is a 4-roll, never 5");
    }

    #[test]
    fn tune_snaps_to_semitones_with_the_centre_at_zero() {
        let t = |f: f32| mod_with(Lane::Tune, StepMod::default(), f).tune;
        assert_eq!(t(0.5), 0, "the resting bar means no retune");
        assert_eq!(t(0.0), -12);
        assert_eq!(t(1.0), 12);
        // Snapped, not stored fractionally: the value shown is the value kept.
        let m = mod_with(Lane::Tune, StepMod::default(), 0.52);
        assert_eq!(m.tune, 0);
    }

    #[test]
    fn slide_toggles_and_ignores_drag_height() {
        let on = toggled(Lane::Slide, StepMod::default());
        assert!(on.slide);
        assert!(!toggled(Lane::Slide, on).slide);
        // toggled() on any other lane is a no-op, so a stray call cannot
        // corrupt a value lane.
        assert_eq!(toggled(Lane::Vel, StepMod::default()), StepMod::default());
    }

    #[test]
    fn editing_back_to_defaults_is_the_default() {
        // Pattern::set_mod removes an all-defaults entry to keep the map
        // sparse; that only works if a lane can actually produce the default
        // again. Full velocity IS the default, so dragging a bar back to the
        // top must not leave a phantom customisation behind.
        let m = mod_with(Lane::Vel, StepMod::default(), 1.0);
        assert_eq!(m, StepMod::default());
        let mut p = Pattern::new("A", 16, 1);
        p.set_mod(0, 3, mod_with(Lane::Vel, StepMod::default(), 0.4));
        assert_eq!(p.mods.len(), 1);
        p.set_mod(0, 3, mod_with(Lane::Vel, p.mod_at(0, 3), 1.0));
        assert_eq!(p.mods.len(), 0, "the sparse map stays sparse");
    }

    #[test]
    fn pitch_lanes_refuse_synths_that_have_no_pitch() {
        // studio-live defines tune/slide as no-ops for Sampler and Beatspace;
        // a lane that edits a no-op teaches a lie.
        let sampler = SynthKind::Sampler { source: studio_core::SourceId(1) };
        assert!(Lane::Tune.applies_to(&sampler), "samplers pitch at render time now");
        assert!(!Lane::Slide.applies_to(&sampler));
        assert!(Lane::Vel.applies_to(&sampler));
        let sub = SynthKind::Sub { note: 0, glide_from: None, glide_ms: 50.0, drive: 0.0, decay_ms: 300.0 };
        assert!(Lane::Slide.applies_to(&sub));
    }
}
