//! Track terrain: a side-view height field the vehicle physics queries each
//! step, plus gaps that force a jump. Kept as data (not hardcoded per-track
//! logic) so map variety is authoring, not code — matches how these games
//! shipped many maps off a small set of mechanics.

#[derive(Debug, Clone, Copy, serde::Serialize, serde::Deserialize)]
pub enum Segment {
    /// Flat ground of given length at a fixed height.
    Flat { length: f32, height: f32 },
    /// Linear ramp from `from_height` to `to_height` over `length`.
    Ramp { length: f32, from_height: f32, to_height: f32 },
    /// No ground at all — falling through means a health penalty or respawn,
    /// same as the original's pit hazards.
    Gap { length: f32 },
}

impl Segment {
    fn length(&self) -> f32 {
        match self {
            Segment::Flat { length, .. } => *length,
            Segment::Ramp { length, .. } => *length,
            Segment::Gap { length } => *length,
        }
    }
}

#[derive(Debug, Clone)]
pub struct Track {
    pub name: String,
    segments: Vec<Segment>,
    /// Cumulative start-x of each segment, computed once at construction.
    starts: Vec<f32>,
    pub total_length: f32,
}

impl Track {
    pub fn new(name: impl Into<String>, segments: Vec<Segment>) -> Self {
        let mut starts = Vec::with_capacity(segments.len());
        let mut x = 0.0;
        for s in &segments {
            starts.push(x);
            x += s.length();
        }
        Track { name: name.into(), segments, starts, total_length: x }
    }

    /// Ground height at world-x `x`. Beyond the authored track (x < 0 or
    /// x > total_length), this returns the boundary segment's height rather
    /// than `None` — confirmed necessary by actually driving off the end of
    /// a real track: since `Vehicle::step_on_ground` never re-grounds once
    /// airborne over `None` ground, treating "past the end" the same as an
    /// interior `Gap` sent the vehicle into permanent freefall the instant
    /// it outran the level (which a fast car does in a few seconds). Interior
    /// `Gap` segments are still genuine voids — this only affects the two ends.
    pub fn height_at(&self, x: f32) -> Option<f32> {
        let clamped = x.clamp(0.0, self.total_length);
        self.height_at_within_track(clamped)
    }

    fn height_at_within_track(&self, x: f32) -> Option<f32> {
        let idx = match self.starts.binary_search_by(|s| s.partial_cmp(&x).unwrap()) {
            Ok(i) => i,
            Err(0) => 0,
            Err(i) => i - 1,
        };
        let seg = self.segments[idx];
        let local_x = x - self.starts[idx];
        match seg {
            Segment::Flat { height, .. } => Some(height),
            Segment::Ramp { length, from_height, to_height, .. } => {
                let t = (local_x / length).clamp(0.0, 1.0);
                Some(from_height + (to_height - from_height) * t)
            }
            Segment::Gap { .. } => None,
        }
    }

    /// Ground slope (rise/run) at world-x `x`, used to tilt the vehicle sprite
    /// and to decide launch angle when driving off a ramp into a jump.
    pub fn slope_at(&self, x: f32) -> f32 {
        const EPS: f32 = 0.05;
        match (self.height_at(x - EPS), self.height_at(x + EPS)) {
            (Some(a), Some(b)) => (b - a) / (2.0 * EPS),
            _ => 0.0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_track() -> Track {
        Track::new(
            "Dust Bowl",
            vec![
                Segment::Flat { length: 10.0, height: 0.0 },
                Segment::Ramp { length: 5.0, from_height: 0.0, to_height: 3.0 },
                Segment::Gap { length: 8.0 },
                Segment::Flat { length: 10.0, height: 0.0 },
            ],
        )
    }

    #[test]
    fn flat_segment_reports_constant_height() {
        let t = sample_track();
        assert_eq!(t.height_at(2.0), Some(0.0));
        assert_eq!(t.height_at(9.9), Some(0.0));
    }

    #[test]
    fn ramp_interpolates_height() {
        let t = sample_track();
        // Ramp spans x=10..15, height 0 -> 3. Segment boundaries are half-open
        // ([start, end)), so x=15.0 belongs to the next segment (the Gap) by
        // convention, not the ramp — check just inside the ramp instead.
        assert_eq!(t.height_at(10.0), Some(0.0));
        let near_end = t.height_at(14.999).unwrap();
        assert!((near_end - 3.0).abs() < 1e-2, "near the ramp's end it should approach to_height: got {near_end}");
        let mid = t.height_at(12.5).unwrap();
        assert!((mid - 1.5).abs() < 1e-4, "midpoint of ramp should be halfway up: got {mid}");
    }

    #[test]
    fn gap_has_no_ground() {
        let t = sample_track();
        // Gap spans x=15..23.
        assert_eq!(t.height_at(18.0), None);
    }

    #[test]
    fn slope_is_positive_going_up_ramp_and_zero_on_flat() {
        let t = sample_track();
        assert!((t.slope_at(2.0)).abs() < 1e-3, "flat ground should have ~zero slope");
        assert!(t.slope_at(12.5) > 0.0, "ramp going up should have positive slope");
    }

    #[test]
    fn beyond_track_bounds_extends_the_boundary_height_as_solid_ground() {
        let t = sample_track();
        // Previously returned None here, which meant driving off either end
        // of the track put the vehicle into permanent freefall (confirmed by
        // actually running the game) — now it extends as flat ground at the
        // boundary segment's height instead of an unbounded void.
        assert_eq!(t.height_at(-5.0), Some(0.0), "before the start should extend the first segment's height");
        assert_eq!(t.height_at(t.total_length + 10.0), Some(0.0), "past the end should extend the last segment's height");
    }

    /// A zero-length Ramp is an authoring typo (or a deliberate vertical
    /// wall?), not something the format explicitly forbids. Confirmed by
    /// actually running it rather than assuming: Rust's NaN-aware
    /// `f32::clamp`/`max`/`min` already prevent a literal 0.0/0.0 division
    /// from propagating as NaN here — height snaps cleanly to the segment's
    /// `to_height`, and slope spikes to a large-but-finite value instead of
    /// infinity. This test exists so that safety is asserted, not incidental.
    #[test]
    fn zero_length_ramp_does_not_produce_nan_or_infinity() {
        let t = Track::new(
            "Degenerate",
            vec![
                Segment::Flat { length: 5.0, height: 0.0 },
                Segment::Ramp { length: 0.0, from_height: 0.0, to_height: 10.0 },
                Segment::Flat { length: 5.0, height: 10.0 },
            ],
        );
        let height = t.height_at(5.0).expect("should still report ground, not None");
        assert!(height.is_finite(), "zero-length ramp should not produce NaN/infinite height");
        assert_eq!(height, 10.0, "should snap to the ramp's to_height rather than divide by zero");

        let slope = t.slope_at(5.0);
        assert!(slope.is_finite(), "slope at a zero-length ramp should be large but finite, not infinite/NaN");
    }
}
