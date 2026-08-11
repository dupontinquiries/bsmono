//! cardinal_gesture — the 11-gesture key classifier from `gestures::cardinal`.
//!
//! In the cardinal-grid layout each key carries **eleven** distinct gestures:
//! tap, eight cardinal swipes, a hairpin (out-and-back) for capitalisation, and
//! CW/CCW rotations for digits and symbols. That is what lets a phone keyboard
//! reach the whole character set without layers.
//!
//! ## The classification order is the algorithm
//!
//! Tap → **Hairpin → Rotation** → Cardinal, and the middle pair is the part
//! that is easy to get backwards. The source says why:
//!
//! > *"Hairpin check goes before rotation because a hairpin technically has
//! > angular change, but its end-position-near-start signal is more specific."*
//!
//! Run rotation first and every hairpin becomes a rotation — capitalisation
//! would emit a digit. Specificity has to beat generality, and here the more
//! specific test is the one that also looks at where the finger *ended*.
//!
//! Cardinal is the **fallback**, not the first check, because a swipe angle can
//! be computed from any two points and would therefore match everything.
//!
//! ## Rotation is measured by regions, not summed angle
//!
//! > *"Rotation detection uses region-sequence analysis rather than summed
//! > angular delta because the latter is fragile on short paths."*
//!
//! A short path near the centroid produces enormous angular deltas from tiny
//! movements — the angle is undefined at the centre — so summing it makes a
//! jittery tap look like a full turn.
//!
//! ## The tap/swipe boundary is an interlock, not a threshold
//!
//! `tap_max_excursion_px` *"MUST stay equal to the UI-layer eviction threshold
//! so the click handler and the classifier agree on the tap/swipe boundary...
//! When the thresholds are scaled for small cells, BOTH scale by the same
//! factor to preserve that equality."*
//!
//! If they disagree, a movement is a tap to one layer and a swipe to the other:
//! the key fires *and* the gesture fires, or neither does.

/// A sampled point while the finger was on one key.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct SwipePoint {
    pub x: f64,
    pub y: f64,
    pub t_ms: u32,
}

/// One of eight cardinal directions.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Direction { N, NE, E, SE, S, SW, W, NW }

impl Direction {
    pub const ALL: [Direction; 8] = [Direction::N, Direction::NE, Direction::E, Direction::SE,
                                     Direction::S, Direction::SW, Direction::W, Direction::NW];

    /// Snap an angle in degrees (0 = east, counter-clockwise positive, screen
    /// y already inverted) to the nearest of eight.
    pub fn from_angle(deg: f64) -> Direction {
        let norm = ((deg % 360.0) + 360.0) % 360.0;
        // +22.5 so each sector is centred on its direction rather than starting
        // at it — without it every swipe biases one sector clockwise.
        let idx = (((norm + 22.5) / 45.0).floor() as usize) % 8;
        [Direction::E, Direction::NE, Direction::N, Direction::NW,
         Direction::W, Direction::SW, Direction::S, Direction::SE][idx]
    }

    pub fn label(self) -> &'static str {
        match self {
            Direction::N => "n", Direction::NE => "ne", Direction::E => "e",
            Direction::SE => "se", Direction::S => "s", Direction::SW => "sw",
            Direction::W => "w", Direction::NW => "nw",
        }
    }
}

/// Rotation sense.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Rotation { Cw, Ccw }

/// What a path on one key meant.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum CardinalGesture {
    Tap,
    Cardinal(Direction),
    /// Out-and-back: capitalisation.
    Hairpin(Direction),
    Rotation(Rotation),
    /// Recognised as nothing. Treated as a missed tap by the engine rather
    /// than silently doing something else.
    Unrecognized,
}

/// Tunables. Pixel values scale with cell size; durations do not.
#[derive(Clone, Copy, Debug)]
pub struct CardinalThresholds {
    pub tap_radius_px: f64,
    pub tap_max_ms: u32,
    pub hairpin_return_tolerance_px: f64,
    pub hairpin_min_excursion_px: f64,
    pub hairpin_max_ms: u32,
    pub rotation_threshold_deg: f64,
    /// **Must equal the UI's tap-eviction threshold.** See the module docs.
    pub tap_max_excursion_px: f64,
}

impl Default for CardinalThresholds {
    fn default() -> Self {
        CardinalThresholds {
            tap_radius_px: 20.0,
            tap_max_ms: 250,
            hairpin_return_tolerance_px: 25.0,
            hairpin_min_excursion_px: 60.0,
            hairpin_max_ms: 600,
            rotation_threshold_deg: 180.0,
            tap_max_excursion_px: 13.0,
        }
    }
}

impl CardinalThresholds {
    /// Scale pixel thresholds for a smaller cell, leaving durations alone.
    ///
    /// Durations are human reaction times and do not change with cell size;
    /// scaling them would make a small keyboard demand faster fingers.
    pub fn scaled(&self, factor: f64) -> CardinalThresholds {
        CardinalThresholds {
            tap_radius_px: self.tap_radius_px * factor,
            hairpin_return_tolerance_px: self.hairpin_return_tolerance_px * factor,
            hairpin_min_excursion_px: self.hairpin_min_excursion_px * factor,
            tap_max_excursion_px: self.tap_max_excursion_px * factor,
            ..*self
        }
    }
}

fn dist(a: &SwipePoint, b: &SwipePoint) -> f64 {
    ((a.x - b.x).powi(2) + (a.y - b.y).powi(2)).sqrt()
}

/// Furthest the path ever got from its start.
fn max_excursion(path: &[SwipePoint]) -> f64 {
    let Some(first) = path.first() else { return 0.0 };
    path.iter().map(|p| dist(first, p)).fold(0.0, f64::max)
}

/// Which quadrant a point sits in relative to a centre, 0..4.
///
/// The basis of region-sequence rotation: a path that visits quadrants in
/// order has turned, regardless of how much angle it accumulated near the
/// middle.
fn region(p: &SwipePoint, cx: f64, cy: f64) -> u8 {
    match (p.x >= cx, p.y >= cy) {
        (true, false) => 0,
        (false, false) => 1,
        (false, true) => 2,
        (true, true) => 3,
    }
}

/// Rotation from the sequence of quadrants visited.
///
/// Region-based rather than summed angle: near the centroid the angle is
/// undefined, so summing it turns a jittery tap into a full turn.
pub fn analyze_rotation(path: &[SwipePoint], threshold_deg: f64) -> Option<Rotation> {
    if path.len() < 4 { return None; }
    let n = path.len() as f64;
    let cx = path.iter().map(|p| p.x).sum::<f64>() / n;
    let cy = path.iter().map(|p| p.y).sum::<f64>() / n;

    let mut steps: i32 = 0;
    let mut prev = region(&path[0], cx, cy);
    for p in &path[1..] {
        let r = region(p, cx, cy);
        if r == prev { continue; }
        // Adjacent quadrants only: a diagonal jump is noise crossing the
        // centre, not a turn, and counting it would double the measured angle.
        let fwd = (r + 4 - prev) % 4;
        match fwd {
            1 => steps += 1,
            3 => steps -= 1,
            _ => {}
        }
        prev = r;
    }
    let degrees = steps.abs() as f64 * 90.0;
    if degrees < threshold_deg { return None; }
    Some(if steps > 0 { Rotation::Ccw } else { Rotation::Cw })
}

/// Classify a path. The order is the algorithm — see the module docs.
pub fn classify(path: &[SwipePoint], t: &CardinalThresholds) -> CardinalGesture {
    let (Some(first), Some(last)) = (path.first(), path.last()) else {
        return CardinalGesture::Unrecognized;
    };
    let duration = last.t_ms.saturating_sub(first.t_ms);
    let excursion = max_excursion(path);

    // 1. Tap — did not travel far enough to be anything else.
    if excursion <= t.tap_max_excursion_px { return CardinalGesture::Tap; }

    let end_offset = dist(first, last);

    // 2. Hairpin BEFORE rotation. A hairpin has angular change too, but its
    //    end-near-start signal is more specific, and rotation-first would turn
    //    every capitalisation into a digit.
    if end_offset <= t.hairpin_return_tolerance_px
        && excursion >= t.hairpin_min_excursion_px
        && duration < t.hairpin_max_ms
    {
        // Direction of the excursion, not of the (near-zero) net travel.
        let peak = path.iter().max_by(|a, b| {
            dist(first, a).partial_cmp(&dist(first, b)).unwrap_or(std::cmp::Ordering::Equal)
        }).unwrap_or(last);
        return CardinalGesture::Hairpin(direction_between(first, peak));
    }

    // 3. Rotation.
    if let Some(r) = analyze_rotation(path, t.rotation_threshold_deg) {
        return CardinalGesture::Rotation(r);
    }

    // 4. Cardinal — the fallback, because an angle can be computed from any two
    //    points and would otherwise match everything.
    if end_offset > t.tap_max_excursion_px {
        return CardinalGesture::Cardinal(direction_between(first, last));
    }

    CardinalGesture::Unrecognized
}

fn direction_between(a: &SwipePoint, b: &SwipePoint) -> Direction {
    // Screen y grows downward, so it is negated to give a conventional angle.
    Direction::from_angle((-(b.y - a.y)).atan2(b.x - a.x).to_degrees())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(x: f64, y: f64, t: u32) -> SwipePoint { SwipePoint { x, y, t_ms: t } }

    #[test]
    fn a_still_finger_is_a_tap() {
        let path = [p(100.0, 100.0, 0), p(102.0, 101.0, 40), p(101.0, 100.0, 80)];
        assert_eq!(classify(&path, &CardinalThresholds::default()), CardinalGesture::Tap);
    }

    #[test]
    fn a_straight_swipe_up_is_north() {
        // Screen y grows downward, so "up" is a NEGATIVE dy — getting this
        // inversion wrong flips every vertical gesture.
        let path = [p(100.0, 200.0, 0), p(100.0, 140.0, 60), p(100.0, 100.0, 120)];
        assert_eq!(classify(&path, &CardinalThresholds::default()),
                   CardinalGesture::Cardinal(Direction::N));
    }

    #[test]
    fn a_swipe_right_is_east_and_left_is_west() {
        let t = CardinalThresholds::default();
        let e = [p(100.0, 100.0, 0), p(180.0, 100.0, 100)];
        assert_eq!(classify(&e, &t), CardinalGesture::Cardinal(Direction::E));
        let w = [p(180.0, 100.0, 0), p(100.0, 100.0, 100)];
        assert_eq!(classify(&w, &t), CardinalGesture::Cardinal(Direction::W));
    }

    #[test]
    fn a_hairpin_is_not_classified_as_a_rotation() {
        // The ordering the source calls out: "hairpin check goes before
        // rotation because a hairpin technically has angular change, but its
        // end-position-near-start signal is more specific." Rotation-first
        // would turn every CAPITALISATION into a DIGIT.
        let path = [p(100.0, 200.0, 0), p(100.0, 150.0, 60), p(100.0, 120.0, 120),
                    p(100.0, 160.0, 200), p(100.0, 205.0, 280)];
        match classify(&path, &CardinalThresholds::default()) {
            CardinalGesture::Hairpin(Direction::N) => {}
            other => panic!("expected a north hairpin, got {other:?}"),
        }
    }

    #[test]
    fn a_hairpin_takes_its_direction_from_the_excursion_not_the_net_travel() {
        // Net travel is near zero by definition, so a direction taken from
        // start-to-end would be noise — and capitalisation would land on a
        // random key face.
        let path = [p(100.0, 100.0, 0), p(180.0, 100.0, 80), p(102.0, 100.0, 160)];
        assert_eq!(classify(&path, &CardinalThresholds::default()),
                   CardinalGesture::Hairpin(Direction::E));
    }

    #[test]
    fn a_slow_out_and_back_is_not_a_hairpin() {
        // Beyond hairpin_max_ms it is a deliberate two-part movement rather
        // than the flick capitalisation is meant to be.
        let t = CardinalThresholds::default();
        let path = [p(100.0, 200.0, 0), p(100.0, 120.0, 400), p(100.0, 202.0, 900)];
        assert_ne!(classify(&path, &t), CardinalGesture::Hairpin(Direction::N));
    }

    #[test]
    fn a_short_excursion_out_and_back_is_not_a_hairpin_either() {
        // Below hairpin_min_excursion_px it is a wobbly tap, and treating it as
        // capitalisation would shift letters at random.
        let t = CardinalThresholds::default();
        let path = [p(100.0, 100.0, 0), p(100.0, 80.0, 60), p(100.0, 101.0, 120)];
        assert!(!matches!(classify(&path, &t), CardinalGesture::Hairpin(_)));
    }

    #[test]
    fn rotation_is_measured_by_regions_so_a_jittery_tap_is_not_a_turn() {
        // "Summed angular delta is fragile on short paths" — near the centroid
        // the angle is undefined, so tiny movements produce enormous deltas.
        let jitter: Vec<SwipePoint> = (0..20)
            .map(|i| p(100.0 + (i % 3) as f64, 100.0 + (i % 2) as f64, i * 10))
            .collect();
        assert_eq!(analyze_rotation(&jitter, 180.0), None);
    }

    #[test]
    fn a_full_circle_is_a_rotation_with_a_sense() {
        // Four quadrants in order is 360 degrees, well past the threshold.
        let circle: Vec<SwipePoint> = (0..24)
            .map(|i| {
                let a = i as f64 / 24.0 * std::f64::consts::TAU;
                p(100.0 + 50.0 * a.cos(), 100.0 + 50.0 * a.sin(), i * 20)
            })
            .collect();
        let r = analyze_rotation(&circle, 180.0);
        assert!(r.is_some(), "a full circle was not a rotation");
    }

    #[test]
    fn the_two_rotation_senses_are_opposite() {
        let mk = |rev: bool| -> Vec<SwipePoint> {
            (0..24).map(|i| {
                let step = if rev { 23 - i } else { i };
                let a = step as f64 / 24.0 * std::f64::consts::TAU;
                p(100.0 + 50.0 * a.cos(), 100.0 + 50.0 * a.sin(), i * 20)
            }).collect()
        };
        let a = analyze_rotation(&mk(false), 180.0).expect("cw or ccw");
        let b = analyze_rotation(&mk(true), 180.0).expect("cw or ccw");
        assert_ne!(a, b, "both directions classified the same way");
    }

    #[test]
    fn a_path_too_short_to_have_turned_is_not_a_rotation() {
        // Three points cannot describe half a turn, and accepting them would
        // make a fast diagonal swipe emit a digit.
        let path = [p(100.0, 100.0, 0), p(150.0, 100.0, 50), p(150.0, 150.0, 100)];
        assert_eq!(analyze_rotation(&path, 180.0), None);
    }

    #[test]
    fn every_sector_is_centred_on_its_direction() {
        // Without the +22.5 offset every swipe biases one sector clockwise, so
        // a dead-straight north swipe would register as north-west.
        assert_eq!(Direction::from_angle(90.0), Direction::N);
        assert_eq!(Direction::from_angle(0.0), Direction::E);
        assert_eq!(Direction::from_angle(180.0), Direction::W);
        assert_eq!(Direction::from_angle(270.0), Direction::S);
        assert_eq!(Direction::from_angle(45.0), Direction::NE);
        // and the sector edges land where they should
        assert_eq!(Direction::from_angle(80.0), Direction::N);
        assert_eq!(Direction::from_angle(100.0), Direction::N);
    }

    #[test]
    fn angles_outside_zero_to_360_still_classify() {
        // atan2 returns negatives, and an unnormalised index would panic or
        // wrap to the wrong direction.
        assert_eq!(Direction::from_angle(-90.0), Direction::S);
        assert_eq!(Direction::from_angle(450.0), Direction::N);
    }

    #[test]
    fn scaling_moves_pixels_but_not_durations() {
        // Durations are human reaction times; scaling them would make a small
        // keyboard demand faster fingers.
        let t = CardinalThresholds::default();
        let s = t.scaled(0.5);
        assert_eq!(s.tap_max_excursion_px, t.tap_max_excursion_px * 0.5);
        assert_eq!(s.hairpin_min_excursion_px, t.hairpin_min_excursion_px * 0.5);
        assert_eq!(s.tap_max_ms, t.tap_max_ms);
        assert_eq!(s.hairpin_max_ms, t.hairpin_max_ms);
    }

    #[test]
    fn the_tap_boundary_scales_with_everything_else() {
        // The interlock: tap_max_excursion_px "MUST stay equal to the UI-layer
        // eviction threshold... BOTH scale by the same factor to preserve that
        // equality." If they diverge, a movement is a tap to one layer and a
        // swipe to the other — the key fires AND the gesture fires, or neither
        // does.
        let t = CardinalThresholds::default();
        for f in [0.5, 0.75, 1.0, 1.5] {
            let s = t.scaled(f);
            assert!((s.tap_max_excursion_px - t.tap_max_excursion_px * f).abs() < 1e-9,
                    "the tap boundary drifted at factor {f}");
        }
    }

    #[test]
    fn an_empty_path_is_unrecognized_rather_than_a_tap() {
        // A tap would emit a character for a touch that produced no samples.
        assert_eq!(classify(&[], &CardinalThresholds::default()),
                   CardinalGesture::Unrecognized);
    }

    #[test]
    fn all_eight_directions_are_reachable() {
        // A layout assigns a character to each; an unreachable one is a
        // character nobody can type.
        let t = CardinalThresholds::default();
        let mut seen = Vec::new();
        for (dx, dy) in [(0.0, -80.0), (60.0, -60.0), (80.0, 0.0), (60.0, 60.0),
                         (0.0, 80.0), (-60.0, 60.0), (-80.0, 0.0), (-60.0, -60.0)] {
            let path = [p(200.0, 200.0, 0), p(200.0 + dx, 200.0 + dy, 100)];
            match classify(&path, &t) {
                CardinalGesture::Cardinal(d) => seen.push(d),
                other => panic!("({dx},{dy}) gave {other:?}"),
            }
        }
        for d in Direction::ALL {
            assert!(seen.contains(&d), "{d:?} is unreachable");
        }
    }
}
