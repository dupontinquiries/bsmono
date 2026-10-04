//! A gate between the pupil detector and the filter: refuse a detection that cannot be the pupil, so a blink or an
//! eyelid highlight does not drag the cursor across the screen. The detector always returns its best blob, even when
//! the best blob is a half-closed lid; this decides.
//!
//! Three rules, each a fact about pupils and none tuned to a clip: a pupil is roughly round (a lid-clipped one is
//! elongated), roughly the size it was a moment ago, and cannot jump more than a fraction of the frame in one frame.
//! A run of refusals ends: after `REACQUIRE_AFTER` in a row the next detection is believed (the eye really did move,
//! or lighting changed) and the history is restarted from it.

use orlosky_kit::Pupil;

/// Longer than this over shorter is a lid-clipped blob, not a pupil.
pub const MAX_ASPECT: f64 = 2.0;
/// Accepted radius, as a multiple of the recent median.
pub const SIZE_BAND: (f64, f64) = (0.6, 1.6);
/// The most a pupil may move between consecutive frames, as a fraction of the frame width.
pub const MAX_JUMP_FRAC: f64 = 0.35;
pub const REACQUIRE_AFTER: usize = 4;
const HISTORY: usize = 15;
/// Radii needed before the size rule applies.
const MIN_HISTORY: usize = 5;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    Accept,
    /// Refused, and why.
    Reject(Reason),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reason { TooElongated, WrongSize, Jumped }

#[derive(Debug, Clone, Default)]
pub struct PupilGate {
    radii: Vec<f64>,
    last: Option<(f64, f64)>,
    refused_in_a_row: usize,
}

impl PupilGate {
    pub fn new() -> PupilGate { PupilGate::default() }

    pub fn reset(&mut self) { *self = PupilGate::default(); }

    fn median_radius(&self) -> Option<f64> {
        if self.radii.len() < MIN_HISTORY { return None; }
        let mut v = self.radii.clone();
        v.sort_by(|a, b| a.total_cmp(b));
        Some(v[v.len() / 2])
    }

    /// Judge one detection in a frame `frame_w` pixels wide.
    pub fn judge(&mut self, p: &Pupil, frame_w: f64) -> Verdict {
        let reacquire = self.refused_in_a_row >= REACQUIRE_AFTER;
        let verdict = if p.minor <= 0.0 || p.major / p.minor > MAX_ASPECT {
            Verdict::Reject(Reason::TooElongated)
        } else if reacquire {
            Verdict::Accept
        } else if self.median_radius().is_some_and(|m| p.radius() < SIZE_BAND.0 * m || p.radius() > SIZE_BAND.1 * m) {
            Verdict::Reject(Reason::WrongSize)
        } else if self.last.is_some_and(|(x, y)| (p.cx - x).hypot(p.cy - y) > MAX_JUMP_FRAC * frame_w) {
            Verdict::Reject(Reason::Jumped)
        } else {
            Verdict::Accept
        };
        match verdict {
            Verdict::Accept => {
                if reacquire { self.radii.clear(); }
                self.refused_in_a_row = 0;
                self.last = Some((p.cx, p.cy));
                self.radii.push(p.radius());
                if self.radii.len() > HISTORY { self.radii.remove(0); }
            }
            Verdict::Reject(_) => self.refused_in_a_row += 1,
        }
        verdict
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pupil(cx: f64, cy: f64, major: f64, minor: f64) -> Pupil {
        Pupil { cx, cy, major, minor, angle_deg: 0.0, area: 1000, darkest: (0, 0), threshold: 0 }
    }
    const W: f64 = 640.0;

    fn warm(g: &mut PupilGate) {
        for i in 0..8 { assert_eq!(g.judge(&pupil(300.0 + i as f64, 220.0, 120.0, 116.0), W), Verdict::Accept); }
    }

    #[test]
    fn a_pupil_drifting_and_changing_size_a_little_is_accepted() {
        let mut g = PupilGate::new();
        warm(&mut g);
        assert_eq!(g.judge(&pupil(330.0, 240.0, 130.0, 118.0), W), Verdict::Accept);
        assert_eq!(g.judge(&pupil(150.0, 240.0, 124.0, 118.0), W), Verdict::Accept, "a 180 px saccade is under the jump limit");
    }

    #[test]
    fn a_lid_clipped_blob_a_wrong_size_and_a_teleport_are_each_refused_for_their_own_reason() {
        let mut g = PupilGate::new();
        warm(&mut g);
        assert_eq!(g.judge(&pupil(656.0, 228.0, 280.0, 78.0), W), Verdict::Reject(Reason::TooElongated));
        assert_eq!(g.judge(&pupil(310.0, 220.0, 50.0, 48.0), W), Verdict::Reject(Reason::WrongSize), "less than 0.6 of the median");
        assert_eq!(g.judge(&pupil(310.0, 220.0, 240.0, 230.0), W), Verdict::Reject(Reason::WrongSize), "more than 1.6 of it");
        assert_eq!(g.judge(&pupil(590.0, 250.0, 118.0, 112.0), W), Verdict::Reject(Reason::Jumped), "280 px in one frame");
    }

    #[test]
    fn a_run_of_refusals_ends_and_the_new_place_is_believed() {
        let mut g = PupilGate::new();
        warm(&mut g);
        for _ in 0..REACQUIRE_AFTER { assert_eq!(g.judge(&pupil(590.0, 250.0, 118.0, 112.0), W), Verdict::Reject(Reason::Jumped)); }
        assert_eq!(g.judge(&pupil(590.0, 250.0, 118.0, 112.0), W), Verdict::Accept, "it really moved");
        assert_eq!(g.judge(&pupil(595.0, 252.0, 120.0, 114.0), W), Verdict::Accept, "and the history restarted there");
        // a different size from before the move is now fine, because the median restarted
        let mut g2 = PupilGate::new();
        warm(&mut g2);
        for _ in 0..REACQUIRE_AFTER { g2.judge(&pupil(300.0, 220.0, 240.0, 230.0), W); }
        assert_eq!(g2.judge(&pupil(300.0, 220.0, 240.0, 230.0), W), Verdict::Accept, "the pupil really got bigger (a lighting change)");
    }

    #[test]
    fn nothing_is_refused_for_size_or_jump_before_there_is_history_and_an_elongated_one_never_passes() {
        let mut g = PupilGate::new();
        assert_eq!(g.judge(&pupil(100.0, 100.0, 40.0, 38.0), W), Verdict::Accept);
        assert_eq!(g.judge(&pupil(500.0, 400.0, 200.0, 190.0), W), Verdict::Reject(Reason::Jumped), "only the jump rule needs just the last point");
        let mut g = PupilGate::new();
        for _ in 0..(REACQUIRE_AFTER + 3) { assert_eq!(g.judge(&pupil(300.0, 200.0, 300.0, 100.0), W), Verdict::Reject(Reason::TooElongated), "even after a run of refusals"); }
        assert_eq!(g.judge(&pupil(300.0, 200.0, 100.0, 0.0), W), Verdict::Reject(Reason::TooElongated), "a degenerate ellipse");
    }
}
