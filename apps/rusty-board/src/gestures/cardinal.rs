//! Cardinal-grid gesture classification.
//!
//! In the cardinal-grid layout style, each key supports up to 11 distinct
//! gestures: tap, 8 cardinal swipes (N/NE/E/SE/S/SW/W/NW), hairpin
//! (swipe-out-and-back) for capitalization, and CW/CCW rotation gestures
//! for digits/symbols.
//!
//! All four gesture variants (Tap, Cardinal, Hairpin, Rotation) are
//! implemented and tested. The classification algorithm runs in the
//! strict order described below; rotation detection uses region-
//! sequence analysis (`analyze_rotation` in this module) rather than
//! summed angular delta because the latter is fragile on short paths.
//!
//! # Classification algorithm
//!
//! Given a path of `(x, y, t)` samples collected while the finger was
//! on a single key, run checks in this strict order:
//!
//! 1. **Tap** — total path length < `tap_radius_px` AND duration <
//!    `tap_max_ms` → return `Tap`. This must come first because a tap
//!    can technically meet every later criterion as well.
//!
//! 2. **Hairpin** — endpoint within `hairpin_return_tolerance_px` of
//!    start AND path reached at least `hairpin_min_excursion_px` from
//!    start at some midpoint AND duration < `hairpin_max_ms` →
//!    return `Hairpin(direction)`. Hairpin check goes before rotation
//!    because a hairpin technically has angular change, but its
//!    end-position-near-start signal is more specific.
//!
//! 3. **Rotation** — accumulated angular change around path centroid
//!    is > `rotation_threshold_deg` (in absolute value) → return
//!    `Rotation(direction)`. Sign tells CW vs CCW.
//!
//! 4. **Cardinal swipe** — fallback. Snap the start-to-end angle to
//!    the nearest of 8 (or 4) cardinal directions → return `Cardinal`.
//!
//! Failing all four (e.g. a curlicue that doesn't quite close) yields
//! `Unrecognized` and the engine treats it as a missed tap.

use crate::gestures::swipe::SwipePoint;
use tracing::debug;

/// Result of classifying a single-key gesture path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CardinalGesture {
    /// Finger didn't move enough to constitute a swipe. The key's tap
    /// action fires.
    Tap,
    /// Linear swipe in one of 8 cardinal directions.
    Cardinal(Direction),
    /// Out-and-back motion (capitalization gesture).
    Hairpin(Direction),
    /// Rotational motion around a centroid.
    Rotation(RotationDir),
    /// Path didn't match any known gesture.
    Unrecognized,
}

/// 8-way compass direction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    N,
    NE,
    E,
    SE,
    S,
    SW,
    W,
    NW,
}

/// Rotation direction. Clockwise = increasing screen-space angle
/// (remember y grows down in screen coordinates, so CW corresponds to
/// positive `atan2` deltas).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RotationDir {
    Clockwise,
    CounterClockwise,
}

/// Thresholds for the classifier. Defaults are heuristics; tunable from
/// config in a future revision.
#[derive(Debug, Clone, Copy)]
pub struct CardinalThresholds {
    pub tap_radius_px: f64,
    pub tap_max_ms: u32,
    pub hairpin_return_tolerance_px: f64,
    pub hairpin_min_excursion_px: f64,
    pub hairpin_max_ms: u32,
    pub rotation_threshold_deg: f64,
}

impl Default for CardinalThresholds {
    fn default() -> Self {
        Self {
            tap_radius_px: 20.0,
            tap_max_ms: 250,
            hairpin_return_tolerance_px: 25.0,
            hairpin_min_excursion_px: 60.0,
            hairpin_max_ms: 600,
            rotation_threshold_deg: 180.0,
        }
    }
}

impl CardinalThresholds {
    /// Lenient variant — accepts sloppier, faster, less precise
    /// gestures. Used when the user enables "lenient swipe" in
    /// settings. The key looseners for the rotation (CCW = capital)
    /// gesture are a lower angular requirement and a more generous
    /// loop-close, so a hurried capital arc that doesn't return
    /// exactly to the start still commits instead of being dropped
    /// as Unrecognized. (The loop-close tolerance itself is applied
    /// in `classify` via `lenient_loop_close`, which reads the same
    /// flag — the threshold struct only carries the scalar knobs.)
    pub fn lenient() -> Self {
        Self {
            // Slightly larger tap radius so tiny wobble stays a tap
            // rather than a 5px "swipe".
            tap_radius_px: 26.0,
            tap_max_ms: 320,
            // Hairpin: accept a looser return and a shorter excursion.
            hairpin_return_tolerance_px: 40.0,
            hairpin_min_excursion_px: 45.0,
            hairpin_max_ms: 800,
            // Rotation: 130° of accumulated arc is enough (vs 180°),
            // so a quick three-quarter loop counts as a full rotation.
            rotation_threshold_deg: 130.0,
        }
    }
}

/// Process-wide lenient-swipe flag for cardinal gestures. Mirrors
/// the config setting; set at boot and on settings change. Read by
/// the cardinal dispatcher to pick the threshold variant and the
/// loop-close tolerance.
static CARDINAL_LENIENT: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

pub fn set_cardinal_lenient(on: bool) {
    CARDINAL_LENIENT.store(on, std::sync::atomic::Ordering::Relaxed);
}

pub fn cardinal_lenient() -> bool {
    CARDINAL_LENIENT.load(std::sync::atomic::Ordering::Relaxed)
}

/// Process-wide cardinal-swipe diagnostic flag. When ON, every
/// cardinal-swipe FAILURE posts a microterm status with the reason
/// — visible on-device so users can see why their swipes aren't
/// firing without needing journal access. Off by default; turn on
/// when investigating reports of "cardinal swipes don't work."
///
/// Successful swipes are silent; the diagnostic only surfaces the
/// cases where the gesture went into the pipeline and came out
/// without committing anything (source-key not found, gesture
/// classified as Tap/Unrecognized, no mapping for the direction).
static CARDINAL_DIAGNOSTIC: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

pub fn set_cardinal_diagnostic(on: bool) {
    CARDINAL_DIAGNOSTIC.store(on, std::sync::atomic::Ordering::Relaxed);
}

pub fn cardinal_diagnostic_enabled() -> bool {
    CARDINAL_DIAGNOSTIC.load(std::sync::atomic::Ordering::Relaxed)
}

/// Process-wide 8-way-diagonals flag. When false (default), a
/// recognized cardinal swipe is snapped to the nearest of the 4
/// primary directions (N/E/S/W) — diagonals are folded in, which is
/// forgiving for users who don't aim precisely. When true, the full
/// 8-way direction is preserved so diagonal swipes (NE/SE/SW/NW) can
/// map to their own characters.
static CARDINAL_DIAGONALS: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

pub fn set_cardinal_diagonals(on: bool) {
    CARDINAL_DIAGONALS.store(on, std::sync::atomic::Ordering::Relaxed);
}

pub fn cardinal_diagonals() -> bool {
    CARDINAL_DIAGONALS.load(std::sync::atomic::Ordering::Relaxed)
}

/// Fold an 8-way direction to the nearest 4-way cardinal using the
/// raw end-displacement to decide the dominant axis. Used when
/// diagonals are disabled: a NE swipe that leaned mostly-east folds
/// to E, mostly-north folds to N. When the two components are
/// near-equal (a true 45° diagonal where intent is genuinely
/// ambiguous) we bias to the vertical axis, matching the common
/// expectation that an up/down-ish flick means up/down. Primary
/// cardinals pass through unchanged.
fn fold_to_four_way(d: Direction, dx: f64, dy: f64) -> Direction {
    match d {
        Direction::NE | Direction::NW | Direction::SE | Direction::SW => {
            // Dominant-axis pick. |dx| vs |dy|; ties (within 15%)
            // go vertical.
            let adx = dx.abs();
            let ady = dy.abs();
            if adx > ady * 1.15 {
                if dx > 0.0 { Direction::E } else { Direction::W }
            } else {
                // Vertical dominant or near-tie. Screen coords:
                // dy<0 is up (N), dy>0 is down (S).
                if dy < 0.0 { Direction::N } else { Direction::S }
            }
        }
        other => other,
    }
}

/// Classify a gesture path. The path is expected to be collected while
/// the finger was within a single key's bounds (the caller decides what
/// counts as "within").
pub fn classify(path: &[SwipePoint], thresholds: &CardinalThresholds) -> CardinalGesture {
    if path.len() < 2 {
        return CardinalGesture::Tap;
    }

    let start = path[0];
    let end = *path.last().unwrap();
    let duration_ms = (end.timestamp_ms - start.timestamp_ms).max(0) as u32;

    // Total euclidean distance traveled.
    let total_length: f64 = path
        .windows(2)
        .map(|w| {
            let dx = w[1].x - w[0].x;
            let dy = w[1].y - w[0].y;
            (dx * dx + dy * dy).sqrt()
        })
        .sum();

    // Max excursion from start. Computed ONCE here because the Tap,
    // Hairpin, and FLICK branches all need it. (Previously also
    // recomputed identically as a second `max_excursion` variable
    // further down — same data, same algorithm, wasted iteration.)
    let max_excursion: f64 = path
        .iter()
        .map(|p| {
            let dx = p.x - start.x;
            let dy = p.y - start.y;
            (dx * dx + dy * dy).sqrt()
        })
        .fold(0.0, f64::max);

    // 1. Tap check. Needs:
    //   - short duration (< tap_max_ms)
    //   - small max excursion (< TAP_MAX_EXCURSION_PX)
    //
    // The excursion gate is critical for the steno "short flick"
    // case: a finger that went OUT 15px and recoiled to within
    // 5px has total_length ~25 but is clearly a directional
    // flick, not a tap. Without the excursion gate, lenient
    // mode's wider tap_radius (26) eats these flicks as taps and
    // the FLICK branch (step 4b) — which is below this Tap check
    // — never sees them. Pre-fix, this manifested as "steno
    // swipes not working" because steno cells need cardinal
    // direction recognition for sub-threshold flicks.
    //
    // Threshold: 13px is just above UI-layer TAP_EVICTION_PX (12),
    // so anything that triggered tap eviction in the drag handler
    // can't be reclassified as a tap here either. 1px safety
    // margin against floating-point edge cases.
    //
    // The `total_length < tap_radius_px` check was previously in
    // this clause as a third AND condition, but the swipe sim
    // (sim/scenarios.py::tap_with_jitter_classified_as_tap)
    // revealed that real touch-sensor jitter accumulates cumulative
    // path length that can exceed tap_radius (20px) even when the
    // finger's spatial dispersion (max_excursion) is tiny — a
    // 5px-radius random walk over 100ms easily totals 30-40px.
    // Such taps were failing the Tap check and falling through
    // every other branch to Unrecognized → silent no-op. The
    // max_excursion check captures "user didn't really move" more
    // robustly than total_length for jitter; total was redundant
    // for the recoil-flick rejection (max_excursion catches it).
    const TAP_MAX_EXCURSION_PX: f64 = 13.0;
    if max_excursion < TAP_MAX_EXCURSION_PX
        && duration_ms < thresholds.tap_max_ms
    {
        debug!(
            "cardinal classify: TAP (len={:.1}, dt={}ms, max_excursion={:.1})",
            total_length, duration_ms, max_excursion
        );
        return CardinalGesture::Tap;
    }

    // 2. Rotation check FIRST — region-sequence analysis.
    //
    // Why before hairpin: a true 360° loop has its endpoint NEAR
    // the start (=closed loop) and a high max-excursion (=radius),
    // which also satisfies hairpin's criteria. Without this
    // ordering, every full rotation gets misclassified as a hairpin
    // (Direction picked by max-excursion-point). Rotation's
    // region-sequence test is the discriminator: a hairpin visits
    // ≤ 2-3 angular regions (out, then back), a rotation visits 4+
    // in monotonic CW/CCW order.
    //
    // The path is decomposed into the sequence of 8-way angular
    // regions it visits around its OWN starting point (not centroid,
    // which has the degenerate-on-path problem the old impl hit).
    // Region order matters:
    //
    //   CW visits regions in the order  N → NE → E → SE → S → SW → W → NW → N
    //   CCW visits in the reverse order N → NW → W → SW → S → SE → E → NE → N
    //
    // We require:
    //   - The path visits ≥ 4 distinct regions (a meaningful arc).
    //   - The visit ORDER is monotonic CW or CCW (allowing wrap-around
    //     at N — going from NW to N is a valid CW step).
    //   - The end point returns reasonably close to the start
    //     (loop_close). Without this, a partial arc would type a
    //     rotation that the user didn't complete; we want them to
    //     close the loop to commit, otherwise abort.
    let end_dist_from_start = {
        let dx = end.x - start.x;
        let dy = end.y - start.y;
        (dx * dx + dy * dy).sqrt()
    };
    // max_excursion already computed at the top of this function;
    // both Tap and the branches below use the same value, so the
    // previous duplicated iteration here is removed.

    let start_x = start.x;
    let start_y = start.y;
    let rotation_result = analyze_rotation(path, start_x, start_y);

    // The user's gesture explicitly RELIES on returning near the
    // start point. If the path traced lots of regions but doesn't
    // close, that's an aborted rotation — don't fall through to
    // Cardinal (which would type the wrong character for what
    // looks like a partial loop).
    let bbox_max = {
        let mut min_x = f64::MAX;
        let mut max_x = f64::MIN;
        let mut min_y = f64::MAX;
        let mut max_y = f64::MIN;
        for p in path {
            min_x = min_x.min(p.x);
            max_x = max_x.max(p.x);
            min_y = min_y.min(p.y);
            max_y = max_y.max(p.y);
        }
        (max_x - min_x).max(max_y - min_y).max(1.0)
    };
    // Loop-close tolerance: the path must return within this
    // fraction of its bounding box to count as a closed rotation.
    // Lenient mode widens it (0.55 vs 0.4) so a hurried CCW capital
    // arc that doesn't quite return to the start still commits
    // rather than aborting to Unrecognized — the reported bug was
    // exactly this, sloppy capitals getting silently dropped.
    let loop_close_frac = if cardinal_lenient() { 0.55 } else { 0.4 };
    let loop_close = end_dist_from_start < bbox_max * loop_close_frac;

    if let Some((dir, n_regions)) = rotation_result {
        if loop_close {
            debug!(
                "cardinal classify: ROTATION {:?} (regions={}, end_dist={:.1}, bbox_max={:.1})",
                dir, n_regions, end_dist_from_start, bbox_max
            );
            return CardinalGesture::Rotation(dir);
        }
        debug!(
            "cardinal classify: rotation candidate {:?} but did not close loop \
             (regions={}, end_dist={:.1} >= 0.4*bbox_max={:.1}); falling through",
            dir, n_regions, end_dist_from_start, bbox_max
        );
        // Path looked like a rotation but didn't close → user
        // started a rotation and gave up. Treat as Unrecognized
        // so we abort silently rather than commit a wrong direction.
        return CardinalGesture::Unrecognized;
    }
    debug!(
        "cardinal classify: not a rotation (end_dist={:.1}, bbox_max={:.1})",
        end_dist_from_start, bbox_max
    );

    // 3. Hairpin check: endpoint near start, but path went substantially far.
    //    (If it went AROUND substantially, it would have matched rotation
    //    above; reaching here means analyze_rotation returned None — the
    //    path didn't visit enough regions in monotonic order.)
    if end_dist_from_start < thresholds.hairpin_return_tolerance_px
        && max_excursion >= thresholds.hairpin_min_excursion_px
        && duration_ms < thresholds.hairpin_max_ms
    {
        // Single max_excursion_point call (was calling twice — once
        // for .0 and once for .1 — which iterated the entire path
        // twice for the same answer).
        let (mx, my) = max_excursion_point(path, start);
        let dir = direction_from(start.x, start.y, mx, my);
        debug!("cardinal classify: HAIRPIN {:?} (excursion={:.1})", dir, max_excursion);
        return CardinalGesture::Hairpin(dir);
    }

    // 4. Cardinal swipe (start → end snapped to 8 directions).
    if end_dist_from_start >= thresholds.tap_radius_px {
        let mut dir = direction_from(start.x, start.y, end.x, end.y);
        // When diagonals are disabled, fold NE/SE/SW/NW into the
        // nearest primary cardinal so a sloppy diagonal still types
        // the intended N/E/S/W character rather than hitting an
        // unbound diagonal slot (which would emit nothing). The fold
        // uses the raw end displacement to pick the dominant axis.
        if !cardinal_diagonals() {
            dir = fold_to_four_way(dir, end.x - start.x, end.y - start.y);
        }
        debug!("cardinal classify: CARDINAL {:?} (dist={:.1})", dir, end_dist_from_start);
        return CardinalGesture::Cardinal(dir);
    }

    // 4b. FLICK swipe — short, quick, directional gesture that ended
    // near the start point. This catches the common "quick flick with
    // finger recoil" pattern: user flicks N decisively, finger goes
    // out 30-40px, releases, finger recoils back to within tap_radius
    // of the start. Without this branch the gesture falls through all
    // other checks (too long for a tap, end too close for cardinal,
    // excursion too short for hairpin) and registers as Unrecognized
    // — user perceives "I swiped and nothing happened."
    //
    // Direction is taken from the max-excursion point (where the
    // finger reached the farthest from start), since the end point
    // is too close to start to give a meaningful direction. Diagonals
    // fold to primaries when the user hasn't enabled 8-way.
    //
    // Threshold history:
    //   - Previously 15px, but a real 16px raw N-flick smooths down
    //     to ~13.8px stored, which fell in the gap [13, 15) between
    //     the Tap check (excursion < 13) and FLICK's lower bound.
    //     The gesture classified as Unrecognized → silent no-op,
    //     user perceived "cardinal swipes don't work." Confirmed
    //     by `smoothed_16px_flick_gap_detection_lenient` test in
    //     this module.
    //   - 13px now: matches TAP_MAX_EXCURSION_PX exactly so every
    //     gesture is either Tap (< 13) or directional (>= 13). No
    //     gap, no overlap. Anything jittery enough to NOT classify
    //     as Tap is at least a FLICK; the direction is derived from
    //     the max-excursion point, which for jitter is essentially
    //     random — but a jittery gesture also stays under
    //     hairpin_max_ms easily, so the user gets a "best guess"
    //     direction rather than silence. Better UX: a wrong direction
    //     is correctable, silence isn't.
    const FLICK_MIN_EXCURSION_PX: f64 = 13.0;
    if max_excursion >= FLICK_MIN_EXCURSION_PX
        && duration_ms < thresholds.hairpin_max_ms
    {
        let (mx, my) = max_excursion_point(path, start);
        let mut dir = direction_from(start.x, start.y, mx, my);
        if !cardinal_diagonals() {
            dir = fold_to_four_way(dir, mx - start.x, my - start.y);
        }
        debug!(
            "cardinal classify: FLICK {:?} (excursion={:.1}, end_dist={:.1})",
            dir, max_excursion, end_dist_from_start
        );
        return CardinalGesture::Cardinal(dir);
    }

    debug!(
        "cardinal classify: UNRECOGNIZED (len={:.1}, end_dist={:.1})",
        total_length, end_dist_from_start
    );
    CardinalGesture::Unrecognized
}

/// Snap an angle (radians, measured in screen coordinates: y grows down,
/// 0 points east, positive angles rotate CW visually) to the nearest of 8
/// cardinal directions.
fn direction_from(x0: f64, y0: f64, x1: f64, y1: f64) -> Direction {
    let dx = x1 - x0;
    let dy = y1 - y0;
    // atan2 returns (-π, π]. Convert to degrees in [0, 360) with 0 = east.
    let theta = dy.atan2(dx).to_degrees();
    // Note y grows downward in screen coords, so "up" is negative dy,
    // which gives a negative angle. Normalize to [0, 360):
    let normalized = (theta + 360.0) % 360.0;
    // 8 wedges of 45° each, centered on 0/45/90/.../315.
    // Direction order (CW from east): E, SE, S, SW, W, NW, N, NE.
    let wedge = ((normalized + 22.5) / 45.0).floor() as i32 % 8;
    match wedge {
        0 => Direction::E,
        1 => Direction::SE,
        2 => Direction::S,
        3 => Direction::SW,
        4 => Direction::W,
        5 => Direction::NW,
        6 => Direction::N,
        7 => Direction::NE,
        _ => Direction::E,
    }
}

fn max_excursion_point(path: &[SwipePoint], start: SwipePoint) -> (f64, f64) {
    path.iter()
        .map(|p| {
            let dx = p.x - start.x;
            let dy = p.y - start.y;
            ((p.x, p.y), dx * dx + dy * dy)
        })
        .max_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal))
        .map(|(pt, _)| pt)
        .unwrap_or((start.x, start.y))
}

/// Region-sequence rotation analysis.
///
/// Given a path and an origin point (the start of the gesture), walk
/// the path and record which 8-way angular region each sample falls
/// into. Suppress noise via:
///
///   - A minimum-distance gate: samples within `MIN_RADIUS_PX` of the
///     origin are too close to have meaningful direction. Skipped.
///   - Hysteresis on region transitions: we only emit a transition
///     when the new region's angle is at least `HYSTERESIS_DEG` past
///     the boundary. This means jittering around a boundary doesn't
///     produce alternating-region noise.
///
/// Then check whether the sequence is monotonic CW or CCW (allowing
/// wrap-around at N). Return `Some((direction, n_distinct_regions))`
/// if the path traces at least 4 regions in a consistent rotational
/// direction; otherwise `None`.
///
/// We use the START of the gesture as the rotation origin instead of
/// the centroid because the centroid-based metric is degenerate when
/// the centroid lies on the path (which is the case for linear and
/// elongated paths). The start is always BEFORE the path, so the
/// finger always moves AROUND it in well-defined angular terms.
fn analyze_rotation(
    path: &[SwipePoint],
    origin_x: f64,
    origin_y: f64,
) -> Option<(RotationDir, usize)> {
    // Minimum radius from origin for a sample's angle to be meaningful.
    // For a finger gesture this is well under one key-width.
    const MIN_RADIUS_PX: f64 = 20.0;
    // Hysteresis when transitioning between regions. Without this, a
    // path that crosses a region boundary at a glancing angle could
    // oscillate between two regions on every sample.
    const HYSTERESIS_DEG: f64 = 8.0;
    // Minimum number of distinct regions visited to qualify as a
    // rotation. 4 corresponds to a quarter-circle covering 4 of the
    // 8 octants — a meaningful arc.
    const MIN_REGIONS: usize = 4;

    if path.len() < 4 {
        return None;
    }

    // Convert each sample to its 8-way angular region around origin,
    // skipping samples too close to origin.
    // Regions, in CW order starting from N: 0=N, 1=NE, 2=E, 3=SE,
    //                                      4=S, 5=SW, 6=W, 7=NW
    // (matches direction_from's wedge layout but starts at N).
    fn region_for(x: f64, y: f64, ox: f64, oy: f64) -> Option<(u8, f64)> {
        let dx = x - ox;
        let dy = y - oy;
        let r2 = dx * dx + dy * dy;
        if r2 < MIN_RADIUS_PX * MIN_RADIUS_PX {
            return None;
        }
        // atan2 with y growing down: 0 = east, +90 = south, -90 = north.
        // Convert to compass degrees: 0 = N, increasing CW.
        let theta_deg = dy.atan2(dx).to_degrees();
        let compass = (theta_deg + 90.0 + 360.0) % 360.0;
        // 8 wedges of 45° each, centered on 0/45/90/.../315 compass.
        let region = (((compass + 22.5) / 45.0).floor() as u8) % 8;
        Some((region, compass))
    }

    // Collect the sequence of distinct regions visited, with hysteresis.
    let mut sequence: Vec<u8> = Vec::with_capacity(16);
    let mut last_region: Option<u8> = None;
    let mut last_compass: f64 = 0.0;
    for p in path {
        let Some((region, compass)) = region_for(p.x, p.y, origin_x, origin_y) else {
            continue;
        };
        match last_region {
            None => {
                sequence.push(region);
                last_region = Some(region);
                last_compass = compass;
            }
            Some(prev) if prev == region => {
                last_compass = compass;
            }
            Some(prev) => {
                // Transitioning. Apply hysteresis: only accept the
                // new region if compass is more than HYSTERESIS_DEG
                // past the boundary into the new region.
                // The boundary between regions prev and region is at
                // mid-angle = (prev*45 + region*45)/2, modulo wrap.
                // Simpler check: is the new compass within
                // (region*45 - 22.5 + HYSTERESIS, region*45 + 22.5 - HYSTERESIS)?
                let region_center = region as f64 * 45.0;
                let inner_low = (region_center - 22.5 + HYSTERESIS_DEG + 360.0) % 360.0;
                let inner_high = (region_center + 22.5 - HYSTERESIS_DEG + 360.0) % 360.0;
                // Range may wrap.
                let in_inner = if inner_low <= inner_high {
                    compass >= inner_low && compass <= inner_high
                } else {
                    compass >= inner_low || compass <= inner_high
                };
                if in_inner {
                    sequence.push(region);
                    last_region = Some(region);
                    last_compass = compass;
                }
                // Otherwise stay in prev region, don't update.
                let _ = (prev, last_compass);
            }
        }
    }

    let _ = last_compass;

    if sequence.len() < MIN_REGIONS {
        return None;
    }

    // Analyze the direction of region transitions.
    // CW step: region b - a ≡ +1 mod 8  (e.g. N→NE, NE→E, ..., NW→N).
    // CCW step: region b - a ≡ -1 mod 8 (equiv +7).
    // Any other step (e.g. N → S) is a "jump" that doesn't fit either
    // direction; we tolerate a small number of jumps but require the
    // majority to be in a consistent direction.
    let mut cw_steps = 0u32;
    let mut ccw_steps = 0u32;
    let mut jumps = 0u32;
    for w in sequence.windows(2) {
        let a = w[0] as i32;
        let b = w[1] as i32;
        let diff = (b - a).rem_euclid(8);
        match diff {
            1 => cw_steps += 1,
            7 => ccw_steps += 1,
            0 => {}
            _ => jumps += 1,
        }
    }
    let directional = cw_steps + ccw_steps;
    let total = directional + jumps;
    if total == 0 {
        return None;
    }
    // At least 75% of transitions must be in one direction.
    let max_in_one_direction = cw_steps.max(ccw_steps);
    if max_in_one_direction * 4 < directional * 3 {
        return None;
    }
    // And at least 75% of all transitions (including jumps) must be
    // directional — large jump counts mean noise, not rotation.
    if directional * 4 < total * 3 {
        return None;
    }

    let dir = if cw_steps > ccw_steps {
        RotationDir::Clockwise
    } else {
        RotationDir::CounterClockwise
    };
    Some((dir, sequence.len()))
}

/// Given a `KeyGestures` map and a classified `CardinalGesture`, return
/// the string to commit (if any). Hairpins capitalize the direction's
/// lowercase output.
///
/// When the global `SHIFT_SWIPE_ENABLED` flag is false, hairpins
/// degrade to a regular cardinal swipe in the hairpin's direction
/// (no capitalization). This lets users who find the gesture too
/// sensitive disable it without losing the underlying swipe.
pub fn dispatch(
    gestures: &crate::keyboard::KeyGestures,
    classified: CardinalGesture,
) -> Option<String> {
    match classified {
        CardinalGesture::Tap | CardinalGesture::Unrecognized => None,
        CardinalGesture::Cardinal(d) => direction_to_output(gestures, d).cloned(),
        CardinalGesture::Hairpin(d) => {
            if shift_swipe_enabled() {
                direction_to_output(gestures, d).map(|s| s.to_uppercase())
            } else {
                // Hairpin treated as a plain cardinal in the same
                // direction — no capitalization.
                direction_to_output(gestures, d).cloned()
            }
        }
        CardinalGesture::Rotation(RotationDir::Clockwise) => gestures.cw.clone(),
        CardinalGesture::Rotation(RotationDir::CounterClockwise) => gestures.ccw.clone(),
    }
}

/// Process-wide toggle for the hairpin = capitalize gesture.
/// Defaults to true (the legacy behavior).
static SHIFT_SWIPE_ENABLED: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(true);

pub fn set_shift_swipe_enabled(on: bool) {
    SHIFT_SWIPE_ENABLED.store(on, std::sync::atomic::Ordering::Relaxed);
}

pub fn shift_swipe_enabled() -> bool {
    SHIFT_SWIPE_ENABLED.load(std::sync::atomic::Ordering::Relaxed)
}

fn direction_to_output(g: &crate::keyboard::KeyGestures, d: Direction) -> Option<&String> {
    match d {
        Direction::N => g.n.as_ref(),
        Direction::NE => g.ne.as_ref(),
        Direction::E => g.e.as_ref(),
        Direction::SE => g.se.as_ref(),
        Direction::S => g.s.as_ref(),
        Direction::SW => g.sw.as_ref(),
        Direction::W => g.w.as_ref(),
        Direction::NW => g.nw.as_ref(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pt(x: f64, y: f64, ms: i64) -> SwipePoint {
        SwipePoint { x, y, timestamp_ms: ms }
    }

    #[test]
    fn tiny_motion_classifies_as_tap() {
        let path = vec![pt(100.0, 100.0, 0), pt(102.0, 101.0, 50)];
        let t = CardinalThresholds::default();
        assert_eq!(classify(&path, &t), CardinalGesture::Tap);
    }

    #[test]
    fn east_swipe_classifies_as_cardinal_e() {
        // 100px to the right, fast.
        let path = vec![
            pt(100.0, 100.0, 0),
            pt(120.0, 100.0, 30),
            pt(150.0, 100.0, 60),
            pt(200.0, 100.0, 100),
        ];
        let t = CardinalThresholds::default();
        assert_eq!(classify(&path, &t), CardinalGesture::Cardinal(Direction::E));
    }

    #[test]
    fn north_swipe_classifies_as_cardinal_n() {
        // 100px upward (y decreasing in screen coords).
        let path = vec![
            pt(100.0, 200.0, 0),
            pt(100.0, 170.0, 30),
            pt(100.0, 130.0, 60),
            pt(100.0, 100.0, 100),
        ];
        let t = CardinalThresholds::default();
        assert_eq!(classify(&path, &t), CardinalGesture::Cardinal(Direction::N));
    }

    #[test]
    fn hairpin_east_then_back() {
        // Out 80px east, then back to start.
        let path = vec![
            pt(100.0, 100.0, 0),
            pt(150.0, 100.0, 80),
            pt(180.0, 100.0, 160),
            pt(150.0, 100.0, 240),
            pt(105.0, 102.0, 320),
        ];
        let t = CardinalThresholds::default();
        assert_eq!(classify(&path, &t), CardinalGesture::Hairpin(Direction::E));
    }

    #[test]
    fn dispatch_cardinal_returns_letter() {
        let g = crate::keyboard::KeyGestures {
            e: Some("e".into()),
            ..Default::default()
        };
        assert_eq!(
            dispatch(&g, CardinalGesture::Cardinal(Direction::E)),
            Some("e".into())
        );
    }

    #[test]
    fn dispatch_hairpin_capitalizes() {
        let g = crate::keyboard::KeyGestures {
            e: Some("e".into()),
            ..Default::default()
        };
        assert_eq!(
            dispatch(&g, CardinalGesture::Hairpin(Direction::E)),
            Some("E".into())
        );
    }

    #[test]
    fn long_flat_east_swipe_is_cardinal_not_rotation() {
        // Regression: a long horizontal east-swipe with tiny y-jitter
        // used to false-positive as Rotation because the path passed
        // through its own centroid (centroid y ≈ path y, so the
        // angle-from-centroid wrapped across atan2's ±π boundary,
        // accumulating ~±180°). The bbox aspect + end-near-start
        // gate now rejects rotation for paths that don't actually
        // loop.
        let path = vec![
            pt(240.0, 37.0, 0),
            pt(280.0, 36.0, 30),
            pt(330.0, 38.0, 60),
            pt(380.0, 37.0, 90),
            pt(430.0, 36.0, 120),
            pt(480.0, 38.0, 150),
            pt(530.0, 37.0, 180),
            pt(585.0, 31.0, 220),
        ];
        let t = CardinalThresholds::default();
        assert_eq!(
            classify(&path, &t),
            CardinalGesture::Cardinal(Direction::E)
        );
    }

    #[test]
    fn loop_path_still_classifies_as_rotation() {
        // Sanity: a path that genuinely loops back near its start
        // should still be recognized as rotation. ~360° clockwise.
        let mut path = Vec::new();
        let cx = 100.0;
        let cy = 100.0;
        let radius = 40.0;
        for i in 0..=24 {
            let theta = (i as f64 / 24.0) * 2.0 * std::f64::consts::PI;
            let x = cx + radius * theta.cos();
            let y = cy + radius * theta.sin();
            path.push(pt(x, y, (i * 20) as i64));
        }
        let t = CardinalThresholds::default();
        match classify(&path, &t) {
            CardinalGesture::Rotation(_) => {}
            other => panic!("expected Rotation, got {:?}", other),
        }
    }

    #[test]
    fn diagonal_flag_controls_ne_folding() {
        // Both directions of the flag in ONE test so the process-wide
        // atomic isn't raced by parallel test execution. A NE swipe
        // (up-and-right) folds to N when diagonals are off, stays NE
        // when on.
        let ne_path = vec![
            pt(100.0, 200.0, 0),
            pt(135.0, 165.0, 40),
            pt(170.0, 130.0, 90),
        ];
        let t = CardinalThresholds::default();

        set_cardinal_diagonals(false);
        assert_eq!(
            classify(&ne_path, &t),
            CardinalGesture::Cardinal(Direction::N),
            "diagonals off → NE should fold to N"
        );

        set_cardinal_diagonals(true);
        assert_eq!(
            classify(&ne_path, &t),
            CardinalGesture::Cardinal(Direction::NE),
            "diagonals on → NE preserved"
        );

        // Reset so other tests see the default.
        set_cardinal_diagonals(false);
    }

    #[test]
    fn fold_to_four_way_picks_dominant_axis() {
        // East-dominant NE (dx >> |dy|) → E.
        assert_eq!(fold_to_four_way(Direction::NE, 80.0, -20.0), Direction::E);
        // North-dominant NE (|dy| >> dx) → N.
        assert_eq!(fold_to_four_way(Direction::NE, 20.0, -80.0), Direction::N);
        // West-dominant SW → W.
        assert_eq!(fold_to_four_way(Direction::SW, -80.0, 20.0), Direction::W);
        // South-dominant SE → S.
        assert_eq!(fold_to_four_way(Direction::SE, 20.0, 80.0), Direction::S);
        // Near-45° tie biases vertical: up → N, down → S.
        assert_eq!(fold_to_four_way(Direction::NE, 50.0, -50.0), Direction::N);
        assert_eq!(fold_to_four_way(Direction::SE, 50.0, 50.0), Direction::S);
        // Primary cardinals pass through (displacement irrelevant).
        assert_eq!(fold_to_four_way(Direction::E, 0.0, 0.0), Direction::E);
        assert_eq!(fold_to_four_way(Direction::W, 0.0, 0.0), Direction::W);
        assert_eq!(fold_to_four_way(Direction::N, 0.0, 0.0), Direction::N);
        assert_eq!(fold_to_four_way(Direction::S, 0.0, 0.0), Direction::S);
    }

    #[test]
    fn lenient_thresholds_are_looser() {
        let strict = CardinalThresholds::default();
        let lenient = CardinalThresholds::lenient();
        assert!(lenient.tap_radius_px > strict.tap_radius_px);
        assert!(lenient.rotation_threshold_deg < strict.rotation_threshold_deg);
        assert!(lenient.hairpin_return_tolerance_px > strict.hairpin_return_tolerance_px);
    }

    // ──── FLICK recovery tests ──────────────────────────────────
    //
    // Regression bug: a quick directional flick with finger recoil
    // (finger goes out, then partly back toward start as user
    // releases) fell through ALL classifier branches:
    //   - Tap: total_length is too large
    //   - Rotation: doesn't visit 4+ angular regions
    //   - Hairpin: max_excursion is below the 60px minimum
    //   - Cardinal: end_dist is below tap_radius_px
    // Result: Unrecognized → no character typed → user perceives
    // "cardinal swipe broken in normal and lenient mode."
    //
    // The new FLICK branch recovers these by using max_excursion
    // direction when end_dist is small but excursion is meaningful.

    #[test]
    fn short_flick_north_with_recoil_classifies_as_cardinal() {
        // User flicks north 35px, finger recoils back to 8px from
        // start as they release. Total duration: 180ms (fast).
        let path = vec![
            pt(100.0, 100.0, 0),
            pt(100.0, 80.0, 30),
            pt(100.0, 65.0, 60),  // out 35px
            pt(100.0, 75.0, 100), // start recoiling
            pt(100.0, 92.0, 180), // ends 8px from start
        ];
        let t = CardinalThresholds::default();
        assert_eq!(classify(&path, &t), CardinalGesture::Cardinal(Direction::N));
    }

    #[test]
    fn short_flick_east_with_recoil_classifies_as_cardinal() {
        // East flick with recoil.
        let path = vec![
            pt(100.0, 100.0, 0),
            pt(125.0, 100.0, 40),  // out 25px
            pt(140.0, 100.0, 80),  // out 40px
            pt(125.0, 100.0, 140), // recoiling
            pt(108.0, 100.0, 200), // ends 8px from start
        ];
        let t = CardinalThresholds::default();
        assert_eq!(classify(&path, &t), CardinalGesture::Cardinal(Direction::E));
    }

    #[test]
    fn short_flick_works_in_lenient_mode() {
        // Same gesture; ensure lenient mode also recognizes it.
        let path = vec![
            pt(100.0, 100.0, 0),
            pt(100.0, 80.0, 30),
            pt(100.0, 65.0, 60),
            pt(100.0, 75.0, 100),
            pt(100.0, 92.0, 180),
        ];
        let t = CardinalThresholds::lenient();
        assert_eq!(classify(&path, &t), CardinalGesture::Cardinal(Direction::N));
    }

    #[test]
    fn tap_with_tiny_jitter_is_still_tap_not_flick() {
        // Critical regression: the flick branch must NOT bleed into
        // tap territory. A tap with ~5px of micro-jitter (typical
        // finger noise) is still a tap.
        let path = vec![
            pt(100.0, 100.0, 0),
            pt(102.0, 99.0, 30),
            pt(103.0, 101.0, 60),
            pt(101.0, 100.0, 100),
        ];
        let t = CardinalThresholds::default();
        assert_eq!(classify(&path, &t), CardinalGesture::Tap);
    }

    #[test]
    fn slow_deliberate_short_directional_swipe_recovered() {
        // User does a SLOW deliberate ~20px directional gesture that
        // ends ~10px from start. Without flick branch: Unrecognized.
        // With flick branch: Cardinal in direction of excursion.
        let path = vec![
            pt(100.0, 100.0, 0),
            pt(100.0, 95.0, 80),
            pt(100.0, 85.0, 160),
            pt(100.0, 80.0, 280), // out 20px (max_excursion)
            pt(100.0, 90.0, 400), // ends 10px from start
        ];
        let t = CardinalThresholds::default();
        // duration = 400ms, under hairpin_max_ms = 600 ✓
        // max_excursion = 20px >= 15px ✓
        // end_dist = 10px < tap_radius (20px) but flick recovers.
        assert_eq!(classify(&path, &t), CardinalGesture::Cardinal(Direction::N));
    }

    #[test]
    fn very_slow_flick_falls_through_to_unrecognized() {
        // Duration > hairpin_max_ms (600ms default). User is too
        // slow → not a flick. Stays Unrecognized.
        let path = vec![
            pt(100.0, 100.0, 0),
            pt(100.0, 85.0, 300),
            pt(100.0, 78.0, 700), // out 22px in 700ms
            pt(100.0, 95.0, 1200), // ends 5px from start, total 1200ms
        ];
        let t = CardinalThresholds::default();
        // Excursion is enough but duration exceeds threshold.
        assert_eq!(classify(&path, &t), CardinalGesture::Unrecognized);
    }

    // -------------------------------------------------------------------
    // Bug-reproduction tests for "cardinal swipes don't work on cardinal
    // and steno layouts". Hypothesis: production smoothing factor 0.3
    // applied per-sample by SwipeTracker reduces the stored
    // max_excursion to roughly 70% of the real finger motion. A real
    // ~15-18px N-flick — clearly a directional swipe to the user —
    // comes out with stored excursion ~10-12px, BELOW the Tap-check
    // threshold of TAP_MAX_EXCURSION_PX=13. Tap wins, dispatch
    // returns silently, user sees nothing.
    //
    // These tests synthesize realistic 15-20px N-flick paths, push
    // them through a SwipeTracker with smoothing=0.3 (the production
    // default), and run classify on the tracker's stored path —
    // the actual sequence on-device.
    // -------------------------------------------------------------------

    /// Push raw samples through SwipeTracker with the production
    /// smoothing factor, return the path the classifier would see.
    fn push_through_tracker(raw: &[SwipePoint], smoothing: f64) -> Vec<SwipePoint> {
        let mut t = crate::gestures::swipe::SwipeTracker::new(smoothing);
        t.start(raw[0].x, raw[0].y);
        for p in &raw[1..] {
            t.update(p.x, p.y);
        }
        t.finish()
    }

    /// Build a synthetic N-flick: finger goes straight up `excursion_px`
    /// over `total_ms` milliseconds at a 60Hz-ish sample rate, then
    /// releases (no recoil — clean directional flick).
    fn n_flick(excursion_px: f64, total_ms: i64) -> Vec<SwipePoint> {
        let start_x = 100.0;
        let start_y = 100.0;
        let n_samples = ((total_ms / 16).max(2)) as usize;
        let mut path = Vec::with_capacity(n_samples);
        for i in 0..n_samples {
            let f = i as f64 / (n_samples - 1) as f64;
            let y = start_y - excursion_px * f;
            let t = (f * total_ms as f64) as i64;
            path.push(SwipePoint::with_timestamp(start_x, y, t));
        }
        path
    }

    #[test]
    fn smoothed_18px_n_flick_classifies_as_cardinal_not_tap() {
        // A clean 18px N-flick over 80ms. Real users do this with
        // their thumbs on cardinal cells. Pre-smoothing the path is
        // 6 samples spanning 18px; after production smoothing it
        // should still have enough excursion to NOT be a Tap.
        let raw = n_flick(18.0, 80);
        let smoothed = push_through_tracker(&raw, 0.3);
        let t = CardinalThresholds::default();
        let classified = classify(&smoothed, &t);
        // Excursion threshold is 13px; with smoothing 0.3 the
        // stored excursion is some fraction of the raw 18px.
        // Compute what the classifier actually sees:
        let start = smoothed[0];
        let stored_excursion: f64 = smoothed
            .iter()
            .map(|p| {
                let dx = p.x - start.x;
                let dy = p.y - start.y;
                (dx * dx + dy * dy).sqrt()
            })
            .fold(0.0_f64, f64::max);
        assert!(
            !matches!(classified, CardinalGesture::Tap),
            "18px raw N-flick classified as Tap after smoothing! \
             stored_excursion={:.1}, classified={:?}. \
             Production TAP_MAX_EXCURSION_PX is 13; if stored is <13, \
             smoothing is the bug.",
            stored_excursion, classified
        );
    }

    #[test]
    fn smoothed_16px_n_flick_classifies_as_cardinal_not_tap() {
        // Slightly shorter — 16px, still clearly a directional flick
        // to the user. Pushing the limits.
        let raw = n_flick(16.0, 70);
        let smoothed = push_through_tracker(&raw, 0.3);
        let t = CardinalThresholds::default();
        let classified = classify(&smoothed, &t);
        let start = smoothed[0];
        let stored_excursion: f64 = smoothed
            .iter()
            .map(|p| {
                let dx = p.x - start.x;
                let dy = p.y - start.y;
                (dx * dx + dy * dy).sqrt()
            })
            .fold(0.0_f64, f64::max);
        assert!(
            !matches!(classified, CardinalGesture::Tap),
            "16px raw N-flick classified as Tap after smoothing. \
             stored_excursion={:.1}, classified={:?}",
            stored_excursion, classified
        );
    }

    #[test]
    fn smoothed_20px_n_flick_classifies_as_cardinal_not_tap() {
        // 20px — should DEFINITELY survive smoothing and classify as
        // a real swipe. If even this is Tap, the bug is severe.
        let raw = n_flick(20.0, 90);
        let smoothed = push_through_tracker(&raw, 0.3);
        let t = CardinalThresholds::default();
        let classified = classify(&smoothed, &t);
        let start = smoothed[0];
        let stored_excursion: f64 = smoothed
            .iter()
            .map(|p| {
                let dx = p.x - start.x;
                let dy = p.y - start.y;
                (dx * dx + dy * dy).sqrt()
            })
            .fold(0.0_f64, f64::max);
        assert!(
            matches!(classified, CardinalGesture::Cardinal(_)),
            "20px raw N-flick should classify as Cardinal, not {:?}. \
             stored_excursion={:.1}",
            classified, stored_excursion
        );
    }

    // -------------------------------------------------------------------
    // Threshold-gap regression guards. Originally panicked when the
    // gap was confirmed; now they verify it stays closed.
    //
    // With FLICK_MIN_EXCURSION_PX == TAP_MAX_EXCURSION_PX (both 13),
    // every gesture is either Tap or directional. No (TAP, FLICK)
    // gap means no short-flick that the user clearly meant as a
    // swipe gets silently dropped as Unrecognized.
    //
    // If a future change widens the gap again, these tests fail
    // with the stored_excursion value pinpointing the offender.
    // -------------------------------------------------------------------

    #[test]
    fn smoothed_15px_flick_gap_detection_strict() {
        // 15px raw → ~13.4px stored — historically in the gap.
        // After the FLICK threshold drop to 13, this is recognized.
        let raw = n_flick(15.0, 70);
        let smoothed = push_through_tracker(&raw, 0.3);
        let t = CardinalThresholds::default();
        let classified = classify(&smoothed, &t);
        let start = smoothed[0];
        let stored_excursion: f64 = smoothed
            .iter()
            .map(|p| {
                let dx = p.x - start.x;
                let dy = p.y - start.y;
                (dx * dx + dy * dy).sqrt()
            })
            .fold(0.0_f64, f64::max);
        assert!(
            !matches!(classified, CardinalGesture::Unrecognized),
            "REGRESSION: 15px raw N-flick → Unrecognized again. \
             stored_excursion={:.2}. The (TAP_MAX, FLICK_MIN) gap \
             has reopened. Check FLICK_MIN_EXCURSION_PX in classify.",
            stored_excursion
        );
    }

    #[test]
    fn smoothed_16px_flick_gap_detection_lenient() {
        // 16px raw → ~13.8px stored. Lenient mode's higher tap_radius
        // means short swipes have to go through FLICK; if FLICK_MIN
        // ever rises above 13.8 again, this test catches it.
        let raw = n_flick(16.0, 75);
        let smoothed = push_through_tracker(&raw, 0.3);
        let t = CardinalThresholds::lenient();
        let classified = classify(&smoothed, &t);
        let start = smoothed[0];
        let stored_excursion: f64 = smoothed
            .iter()
            .map(|p| {
                let dx = p.x - start.x;
                let dy = p.y - start.y;
                (dx * dx + dy * dy).sqrt()
            })
            .fold(0.0_f64, f64::max);
        assert!(
            !matches!(classified, CardinalGesture::Unrecognized),
            "REGRESSION: 16px raw N-flick → Unrecognized in lenient \
             mode. stored_excursion={:.2}. The gap between Tap and \
             FLICK reopened in lenient mode (tap_radius=26 amplifies \
             the gap). Check FLICK_MIN_EXCURSION_PX.",
            stored_excursion
        );
        assert!(
            matches!(classified, CardinalGesture::Cardinal(_)),
            "16px raw N-flick should classify as Cardinal direction, \
             got {:?} (stored={:.2})",
            classified, stored_excursion
        );
    }
}
