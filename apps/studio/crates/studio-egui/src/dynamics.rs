//! dynamics — the compressor's transfer curve and gain-reduction meter.
//!
//! `studio_fx::Compressor` exposes `curve_db` ("static curve: input level in dB
//! → output level in dB") and `reduction_db`, whose doc comment reads **"For the
//! meter"** — a view the crate was written for and which did not exist. The EQ
//! and the spectrum both have one; dynamics did not.
//!
//! ## The curve is the control, not an illustration
//!
//! Threshold, ratio and knee are three numbers that interact, and nobody sets
//! them by reading the numbers. You set them by watching where the line bends
//! and how hard. A compressor UI without the curve is three sliders and a guess,
//! which is why every hardware unit that could afford a screen drew one.
//!
//! ## The meter reads DOWNWARD and that is not a style choice
//!
//! Gain reduction is negative by definition — `reduction_db` is `.min(0.0)` at
//! the source. A meter that grew upward with "more compression" would invert the
//! one mapping every engineer already has: the needle falls when the compressor
//! works. Drawing it upward makes a familiar instrument read backwards.
//!
//! ## Peak hold, because the eye misses the transient that matters
//!
//! Reduction on a snare lasts a few milliseconds. At 60fps it is one frame, and
//! whether you saw it is luck. The peak holds and then falls back, so "did that
//! clip?" is answerable after the fact rather than only during.

use studio_fx::Compressor;

/// Decibel range the curve is drawn over.
///
/// -60 to 0: below -60 nothing audible is happening to the signal, and above 0
/// the curve is off the top of any sane meter. Wider would compress the region
/// where the knee actually lives into a few pixels.
pub const MIN_DB: f32 = -60.0;
pub const MAX_DB: f32 = 0.0;

/// How far the reduction meter reads.
///
/// 24 dB. Beyond that the compressor is being used as a limiter and the exact
/// figure stops mattering — what matters is that it is pinned.
pub const METER_FLOOR_DB: f32 = -24.0;

/// How long a peak stays before it starts falling, in seconds.
pub const PEAK_HOLD_S: f32 = 1.2;
/// How fast it falls afterwards, in dB per second.
pub const PEAK_FALL_DB_S: f32 = 12.0;

/// Sample the transfer curve into `points` (input_db, output_db) pairs.
///
/// Sampled rather than solved because the knee makes the curve piecewise and a
/// caller drawing it wants points, not a formula. `points < 2` returns empty:
/// one point is not a line, and a renderer given one would draw nothing anyway
/// while looking like it had data.
pub fn curve(comp: &Compressor, points: usize) -> Vec<(f32, f32)> {
    if points < 2 { return Vec::new(); }
    (0..points).map(|i| {
        let x = MIN_DB + (MAX_DB - MIN_DB) * i as f32 / (points - 1) as f32;
        (x, comp.curve_db(x))
    }).collect()
}

/// Where the curve departs from unity, in dB — the visible bend.
///
/// Not simply the threshold: with a soft knee the curve starts bending
/// `knee_db / 2` early, and a UI that drew the threshold marker at the bend
/// would disagree with the number in the field beside it.
pub fn knee_start_db(comp: &Compressor) -> f32 {
    comp.threshold_db - comp.knee_db * 0.5
}

/// Is this compressor doing anything at all?
///
/// A ratio at or below 1:1 passes signal through, and drawing a "curve" that is
/// exactly the unity line invites the user to hunt for a bug in their routing.
/// The caller can say "bypassed" instead.
pub fn is_transparent(comp: &Compressor) -> bool { comp.ratio <= 1.0 }

/// The gain-reduction meter's state.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ReductionMeter {
    /// Current reduction, dB, negative or zero.
    current_db: f32,
    /// Held peak, dB.
    peak_db: f32,
    /// Seconds the peak has been held.
    held_s: f32,
}

impl Default for ReductionMeter {
    fn default() -> Self {
        ReductionMeter { current_db: 0.0, peak_db: 0.0, held_s: 0.0 }
    }
}

impl ReductionMeter {
    pub fn current_db(&self) -> f32 { self.current_db }
    pub fn peak_db(&self) -> f32 { self.peak_db }

    /// Feed a reading and advance time.
    pub fn feed(&mut self, reduction_db: f32, dt_s: f32) {
        // A positive reading is a bug upstream — reduction is negative by
        // definition — and clamping is better than drawing a meter that grows
        // the wrong way while someone tries to work out what they changed.
        let r = if reduction_db.is_finite() { reduction_db.min(0.0) } else { 0.0 };
        self.current_db = r;

        if r < self.peak_db {
            // A new deeper peak: hold it from now.
            self.peak_db = r;
            self.held_s = 0.0;
            return;
        }
        self.held_s += dt_s;
        if self.held_s > PEAK_HOLD_S {
            // Falls back toward zero, never past it.
            self.peak_db = (self.peak_db + PEAK_FALL_DB_S * dt_s).min(0.0);
        }
    }

    /// Meter fill as 0..=1, where 1 is the floor.
    pub fn fill(&self) -> f32 {
        (self.current_db / METER_FLOOR_DB).clamp(0.0, 1.0)
    }

    /// Is the meter pinned — reduction past the scale?
    ///
    /// Worth showing distinctly: a meter sitting at the bottom looks identical
    /// whether it is at the floor or ten dB past it, and those are different
    /// problems.
    pub fn is_pinned(&self) -> bool { self.current_db <= METER_FLOOR_DB }

    pub fn reset(&mut self) { *self = ReductionMeter::default(); }
}

/// Draw the curve and the meter.
pub fn dynamics_ui(ui: &mut egui::Ui, comp: &Compressor, meter: &ReductionMeter,
                   c: &uikit_egui::PaletteColors) {
    let size = egui::vec2(ui.available_width().min(240.0), 180.0);
    let (rect, _) = ui.allocate_exact_size(size, egui::Sense::hover());
    // Cloned rather than borrowed: the meter allocates more space below, and a
    // borrow of ui held across that allocation does not compile. Painter is a
    // cheap handle, which is why egui makes it clonable.
    let painter = ui.painter().clone();
    painter.rect_filled(rect, 4.0, c.bg_raised);

    let to_pos = |x_db: f32, y_db: f32| {
        let nx = (x_db - MIN_DB) / (MAX_DB - MIN_DB);
        let ny = (y_db - MIN_DB) / (MAX_DB - MIN_DB);
        egui::pos2(rect.left() + nx * rect.width(),
                   rect.bottom() - ny.clamp(0.0, 1.0) * rect.height())
    };

    // Unity reference first, underneath: the curve is read as a DEPARTURE from
    // it, and without the reference a gentle ratio looks like no ratio.
    painter.line_segment([to_pos(MIN_DB, MIN_DB), to_pos(MAX_DB, MAX_DB)],
                         egui::Stroke::new(1.0_f32, c.border));

    if is_transparent(comp) {
        painter.text(rect.center(), egui::Align2::CENTER_CENTER, "bypassed (1:1)",
                     egui::FontId::proportional(12.0), c.text_weak);
    } else {
        let pts: Vec<egui::Pos2> = curve(comp, 96).into_iter()
            .map(|(x, y)| to_pos(x, y)).collect();
        painter.add(egui::Shape::line(pts, egui::Stroke::new(2.0_f32, c.accent)));
        // The bend, not the threshold — see knee_start_db.
        let k = knee_start_db(comp);
        painter.line_segment([to_pos(k, MIN_DB), to_pos(k, MAX_DB)],
                             egui::Stroke::new(1.0_f32, c.text_weak));
    }

    ui.add_space(4.0);
    let (mrect, _) = ui.allocate_exact_size(egui::vec2(size.x, 14.0),
                                            egui::Sense::hover());
    painter.rect_filled(mrect, 2.0, c.bg_base);
    // Grows DOWNWARD from the top — the needle falls when the compressor works.
    let w = mrect.width() * meter.fill();
    let bar = egui::Rect::from_min_size(mrect.min, egui::vec2(w, mrect.height()));
    let col = if meter.is_pinned() { c.danger } else { c.warning };
    painter.rect_filled(bar, 2.0, col);
    let px = mrect.left() + mrect.width() * (meter.peak_db() / METER_FLOOR_DB).clamp(0.0, 1.0);
    painter.line_segment([egui::pos2(px, mrect.top()), egui::pos2(px, mrect.bottom())],
                         egui::Stroke::new(1.5_f32, c.text));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn comp() -> Compressor { Compressor::new(48_000.0, -20.0, 4.0, 10.0, 100.0) }

    #[test]
    fn below_the_threshold_the_curve_is_unity() {
        // A compressor that changed quiet signal would be a bug, and the curve
        // is how you see that it does not.
        let c = comp();
        assert!((c.curve_db(-50.0) - -50.0).abs() < 1.0);
    }

    #[test]
    fn above_the_threshold_output_rises_more_slowly_than_input() {
        // The definition of compression, and the thing the picture shows.
        let c = comp();
        let a = c.curve_db(-10.0);
        let b = c.curve_db(-5.0);
        assert!(b > a, "still monotonic");
        assert!(b - a < 5.0, "but compressed: {} dB out for 5 dB in", b - a);
    }

    #[test]
    fn the_curve_is_sampled_across_the_whole_range() {
        let pts = curve(&comp(), 64);
        assert_eq!(pts.len(), 64);
        assert!((pts[0].0 - MIN_DB).abs() < 1e-3);
        assert!((pts[63].0 - MAX_DB).abs() < 1e-3);
    }

    #[test]
    fn fewer_than_two_points_is_no_curve_rather_than_one_dot() {
        // A renderer given one point draws nothing while looking like it had
        // data.
        assert!(curve(&comp(), 1).is_empty());
        assert!(curve(&comp(), 0).is_empty());
    }

    #[test]
    fn the_bend_is_earlier_than_the_threshold_when_the_knee_is_soft() {
        // A UI drawing the threshold marker AT the bend would disagree with the
        // number in the field beside it.
        let mut c = comp();
        c.knee_db = 6.0;
        assert!((knee_start_db(&c) - -23.0).abs() < 1e-3);
        c.knee_db = 0.0;
        assert!((knee_start_db(&c) - -20.0).abs() < 1e-3, "a hard knee bends at the threshold");
    }

    #[test]
    fn a_unity_ratio_is_reported_as_transparent() {
        // Drawing a "curve" that is exactly the unity line invites the user to
        // hunt for a bug in their routing.
        let mut c = comp();
        c.ratio = 1.0;
        assert!(is_transparent(&c));
        c.ratio = 0.5;
        assert!(is_transparent(&c), "sub-unity is not expansion here either");
        c.ratio = 2.0;
        assert!(!is_transparent(&c));
    }

    #[test]
    fn the_meter_reads_downward_and_never_upward() {
        // Reduction is negative by definition; a positive reading is an
        // upstream bug, and clamping beats drawing a meter that grows the wrong
        // way while someone works out what they changed.
        let mut m = ReductionMeter::default();
        m.feed(3.0, 0.016);
        assert_eq!(m.current_db(), 0.0);
        m.feed(-6.0, 0.016);
        assert_eq!(m.current_db(), -6.0);
    }

    #[test]
    fn a_non_finite_reading_does_not_poison_the_meter() {
        // One NaN would make fill() NaN and the bar vanish for the rest of the
        // session.
        let mut m = ReductionMeter::default();
        m.feed(f32::NAN, 0.016);
        assert_eq!(m.current_db(), 0.0);
        assert!(m.fill().is_finite());
    }

    #[test]
    fn the_peak_holds_past_the_transient_that_caused_it() {
        // Reduction on a snare lasts a few milliseconds — one frame at 60fps,
        // and whether you saw it is luck.
        let mut m = ReductionMeter::default();
        m.feed(-12.0, 0.016);
        assert_eq!(m.peak_db(), -12.0);
        m.feed(0.0, 0.016);
        assert_eq!(m.current_db(), 0.0, "the transient is over");
        assert_eq!(m.peak_db(), -12.0, "but you can still see it happened");
    }

    #[test]
    fn the_peak_falls_back_after_the_hold() {
        let mut m = ReductionMeter::default();
        m.feed(-12.0, 0.016);
        for _ in 0..80 { m.feed(0.0, 0.05); }
        assert!(m.peak_db() > -12.0, "it decayed");
        assert!(m.peak_db() <= 0.0, "and never rose above zero");
    }

    #[test]
    fn a_deeper_peak_replaces_the_held_one_immediately() {
        // Waiting for the hold to expire would hide the worse event behind the
        // milder one.
        let mut m = ReductionMeter::default();
        m.feed(-6.0, 0.016);
        m.feed(-18.0, 0.016);
        assert_eq!(m.peak_db(), -18.0);
    }

    #[test]
    fn the_fill_is_a_fraction_of_the_floor() {
        let mut m = ReductionMeter::default();
        m.feed(METER_FLOOR_DB / 2.0, 0.016);
        assert!((m.fill() - 0.5).abs() < 1e-3);
    }

    #[test]
    fn reduction_past_the_floor_pins_rather_than_overflowing() {
        // A meter at the bottom looks identical whether it is at the floor or
        // ten dB past it, and those are different problems.
        let mut m = ReductionMeter::default();
        m.feed(METER_FLOOR_DB - 10.0, 0.016);
        assert_eq!(m.fill(), 1.0);
        assert!(m.is_pinned());
    }

    #[test]
    fn resetting_clears_the_held_peak() {
        // Between takes: the last take's worst moment must not sit over the new
        // one.
        let mut m = ReductionMeter::default();
        m.feed(-15.0, 0.016);
        m.reset();
        assert_eq!(m.peak_db(), 0.0);
        assert_eq!(m.current_db(), 0.0);
    }
}
