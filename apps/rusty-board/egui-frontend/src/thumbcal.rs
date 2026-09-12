//! thumbcal — measuring the operator's actual thumb reach, instead of assuming it.
//!
//! `thumbarc::Arc::default_for` picks a compaction from the board's WIDTH: mild past 360pt,
//! strong past 480. That is a guess about a hand from a number about a screen, and it is
//! wrong in both directions — a small hand on a mild board still cannot reach the far top
//! corner, and a large hand on a strong one has the keys pulled in for no reason.
//!
//! This is the flow that replaces the guess with a measurement: anchor, sweep, confirm.
//!
//! ## Why three steps and not one
//!
//! A single "sweep your thumb" instruction produces a stroke with no known origin. The same
//! arc means different things depending on where the hand is braced, and the anchor is the one
//! thing the user knows precisely and the app cannot infer — so it is asked for first, by tap,
//! which is unambiguous.
//!
//! And CONFIRM exists because a sweep can be bad: a hand that slipped, a stroke that stopped
//! short, a tap registered as a one-point drag. Committing that silently gives someone a
//! keyboard they cannot use and no idea what changed. Showing the fitted shape and asking is
//! the difference between a calibration and an accident.
//!
//! ## Normalised, like the arc it produces
//!
//! Samples come in as board-relative 0..1, so the result survives a rotation, a resize and a
//! different device. A calibration in points would be re-run every time the keyboard changed
//! size, which is exactly when someone would least want to.

use crate::thumbarc::{Arc, Corner};

/// Where the flow is.
#[derive(Debug, Clone, PartialEq)]
pub enum Step {
    /// Waiting for the user to tap the corner their thumb is braced at.
    Anchor,
    /// Recording the sweep from that anchor.
    Sweep { anchor: Corner, samples: Vec<(f32, f32)> },
    /// A shape was fitted; waiting for yes or no.
    Confirm { anchor: Corner, arc: Arc, reach: f32 },
    /// The sweep was not usable. `why` is for the user, not the log.
    Rejected { anchor: Corner, why: String },
}

/// The calibration in progress.
#[derive(Debug, Clone)]
pub struct Calibration {
    pub step: Step,
}

/// Fewest samples a sweep must have.
///
/// A tap that dragged a pixel is not a sweep, and fitting an arc to three points produces a
/// shape that says more about the noise than the hand.
pub const MIN_SAMPLES: usize = 8;

/// Shortest total travel, as a fraction of the board's diagonal.
///
/// A stroke that never left the corner measured the corner. Refused rather than fitted,
/// because the resulting arc would compact the board to almost nothing and the user would
/// have to find this screen again to undo it — on a keyboard that is now unusable.
pub const MIN_TRAVEL: f32 = 0.25;

impl Default for Calibration {
    fn default() -> Self {
        Calibration { step: Step::Anchor }
    }
}

impl Calibration {
    /// The user tapped a corner.
    pub fn anchor(&mut self, corner: Corner) {
        self.step = Step::Sweep { anchor: corner, samples: Vec::new() };
    }

    /// One point of the sweep, board-relative.
    pub fn sample(&mut self, p: (f32, f32)) {
        if let Step::Sweep { samples, .. } = &mut self.step {
            let p = (p.0.clamp(0.0, 1.0), p.1.clamp(0.0, 1.0));
            // Skip a repeat: a finger held still emits the same point every frame, and a
            // hundred copies of it would satisfy `MIN_SAMPLES` without being a sweep.
            if samples.last().is_none_or(|l| dist(*l, p) > 0.005) {
                samples.push(p);
            }
        }
    }

    /// The finger lifted: fit, or explain why not.
    pub fn finish(&mut self) {
        let Step::Sweep { anchor, samples } = &self.step else { return };
        let (anchor, samples) = (*anchor, samples.clone());
        if samples.len() < MIN_SAMPLES {
            self.step = Step::Rejected {
                anchor,
                why: "that was a tap, not a sweep — hold and draw the far edge of your reach"
                    .into(),
            };
            return;
        }
        let travel: f32 = samples.windows(2).map(|w| dist(w[0], w[1])).sum();
        if travel < MIN_TRAVEL {
            self.step = Step::Rejected {
                anchor,
                why: "that barely left the corner — sweep out as far as your thumb comfortably \
                      goes"
                    .into(),
            };
            return;
        }
        let reach = reach_of(anchor, &samples);
        self.step = Step::Confirm { anchor, arc: Arc::default_shape(anchor, reach), reach };
    }

    /// Accept the fitted shape.
    pub fn accept(&self) -> Option<Arc> {
        match &self.step {
            Step::Confirm { arc, .. } => Some(arc.clone()),
            _ => None,
        }
    }

    /// Try again from the anchor that was already given — a rejected sweep should not make
    /// the user re-tap a corner they got right.
    pub fn retry(&mut self) {
        let anchor = match &self.step {
            Step::Sweep { anchor, .. } | Step::Confirm { anchor, .. } | Step::Rejected { anchor, .. } => {
                Some(*anchor)
            }
            Step::Anchor => None,
        };
        self.step = match anchor {
            Some(a) => Step::Sweep { anchor: a, samples: Vec::new() },
            None => Step::Anchor,
        };
    }
}

/// How far along the TOP the sweep reached, as `top_width` for `Arc::default_shape`.
///
/// The top row is the one compaction is for — the bottom is under the thumb whatever happens
/// — so the fit reads the sample nearest the top edge and asks how far across it got.
///
/// Measured from the anchor's side, and clamped: an arc narrower than 0.35 leaves too few keys
/// to type on, and one wider than 1.0 is not compaction at all. Someone whose sweep says 0.2
/// has mis-swept or has a keyboard too wide for one hand, and the answer to both is the same
/// floor rather than an unusable board.
pub fn reach_of(anchor: Corner, samples: &[(f32, f32)]) -> f32 {
    let top = samples.iter().min_by(|a, b| a.1.total_cmp(&b.1)).copied().unwrap_or((1.0, 0.0));
    let across = match anchor {
        Corner::BottomRight => 1.0 - top.0,
        Corner::BottomLeft => top.0,
    };
    across.clamp(0.35, 1.0)
}

fn dist(a: (f32, f32), b: (f32, f32)) -> f32 {
    ((a.0 - b.0).powi(2) + (a.1 - b.1).powi(2)).sqrt()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A sweep from the bottom-right that reaches two thirds of the way across the top.
    fn sweep(anchor: Corner, top_x: f32) -> Vec<(f32, f32)> {
        (0..12)
            .map(|i| {
                let t = i as f32 / 11.0;
                let start = match anchor {
                    Corner::BottomRight => 1.0,
                    Corner::BottomLeft => 0.0,
                };
                (start + (top_x - start) * t, 1.0 - t)
            })
            .collect()
    }

    #[test]
    fn a_good_sweep_fits_an_arc_and_asks_before_committing() {
        let mut c = Calibration::default();
        c.anchor(Corner::BottomRight);
        for p in sweep(Corner::BottomRight, 0.3) {
            c.sample(p);
        }
        c.finish();
        // CONFIRM, not commit. A sweep can be bad, and committing one silently gives someone a
        // keyboard they cannot use and no idea what changed.
        match &c.step {
            Step::Confirm { reach, .. } => assert!((*reach - 0.7).abs() < 0.05, "{reach}"),
            other => panic!("{other:?}"),
        }
        assert!(c.accept().is_some());
    }

    /// A stroke that never left the corner measured the corner. Fitting it would compact the
    /// board to almost nothing, and undoing that means finding this screen again on a keyboard
    /// that is now unusable.
    #[test]
    fn a_stroke_that_barely_moved_is_refused_with_a_reason() {
        let mut c = Calibration::default();
        c.anchor(Corner::BottomRight);
        for i in 0..12 {
            c.sample((1.0 - i as f32 * 0.005, 1.0 - i as f32 * 0.005));
        }
        c.finish();
        match &c.step {
            Step::Rejected { why, .. } => assert!(why.contains("barely left"), "{why}"),
            other => panic!("{other:?}"),
        }
        assert!(c.accept().is_none(), "a refused sweep must not be committable");
    }

    /// A tap that dragged a pixel is not a sweep, and an arc fitted to three points describes
    /// the noise.
    #[test]
    fn a_tap_is_not_a_sweep() {
        let mut c = Calibration::default();
        c.anchor(Corner::BottomLeft);
        c.sample((0.0, 1.0));
        c.sample((0.05, 0.95));
        c.finish();
        assert!(matches!(&c.step, Step::Rejected { why, .. } if why.contains("tap")), "{:?}", c.step);
    }

    /// A finger held still emits the same point every frame; a hundred copies would satisfy
    /// the count without being a sweep.
    #[test]
    fn repeated_samples_do_not_pad_the_count() {
        let mut c = Calibration::default();
        c.anchor(Corner::BottomRight);
        for _ in 0..50 {
            c.sample((1.0, 1.0));
        }
        match &c.step {
            Step::Sweep { samples, .. } => assert_eq!(samples.len(), 1, "{} kept", samples.len()),
            other => panic!("{other:?}"),
        }
        c.finish();
        assert!(matches!(c.step, Step::Rejected { .. }));
    }

    /// A rejected sweep should not make the user re-tap a corner they got right.
    #[test]
    fn retry_keeps_the_anchor() {
        let mut c = Calibration::default();
        c.anchor(Corner::BottomLeft);
        c.sample((0.0, 1.0));
        c.finish();
        assert!(matches!(c.step, Step::Rejected { .. }));
        c.retry();
        match &c.step {
            Step::Sweep { anchor, samples } => {
                assert_eq!(*anchor, Corner::BottomLeft);
                assert!(samples.is_empty(), "the old samples came back");
            }
            other => panic!("{other:?}"),
        }
    }

    /// Both hands, and the reach is measured from the anchor's own side — a mirrored sweep
    /// must not read as a tiny reach.
    #[test]
    fn the_reach_is_measured_from_the_anchor_side() {
        assert!((reach_of(Corner::BottomRight, &sweep(Corner::BottomRight, 0.3)) - 0.7).abs() < 0.05);
        assert!((reach_of(Corner::BottomLeft, &sweep(Corner::BottomLeft, 0.7)) - 0.7).abs() < 0.05);

        // Clamped: a sweep claiming almost no reach leaves too few keys to type on, and one
        // claiming the whole board is not compaction.
        assert_eq!(reach_of(Corner::BottomRight, &[(0.98, 0.0)]), 0.35);
        assert_eq!(reach_of(Corner::BottomRight, &[(0.0, 0.0)]), 1.0);
    }
}
