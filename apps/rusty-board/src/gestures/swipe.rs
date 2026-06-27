//! Swipe-typing path collection and decoding.
//!
//! As the user drags across the keyboard, we accumulate `SwipePoint`s along
//! the path. When the gesture ends, we walk the path, snap each sample to
//! the nearest letter key, and dedupe consecutive duplicates to produce a
//! candidate word.
//!
//! The result is a *raw* swipe word — typically not a valid dictionary
//! word. It's the dictionary lookup layer's job (in `recognizer.rs`) to
//! turn `helo` into `hello`.

use crate::keyboard::Key;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::OnceLock;
use std::time::Instant;

/// Absolute stop-velocity threshold (px/ms) in thousandths, used by
/// the swipe segmenter to decide whether a near-zero dwell counts as
/// a deliberate letter-marking stop. Tied to the "swipe speed mode"
/// setting:
///   - slow  → higher threshold (0.10): a careful swiper pauses
///     gently and never quite stops; count those gentle pauses.
///   - normal → 0.06 (the balanced default).
///   - fast  → lower threshold (0.035): a quick swiper only briefly
///     decelerates, so only a near-true-stop should mark a letter,
///     avoiding spurious letters from a fast glide's minor wobble.
/// Stored in thousandths so it fits an atomic int (0.06 → 60).
static STOP_VELOCITY_MILLI: AtomicU32 = AtomicU32::new(60);

/// Set the stop-velocity threshold from the speed-mode string.
/// Unknown values fall back to "normal".
pub fn set_swipe_speed_mode(mode: &str) {
    let milli = match mode {
        "slow" => 100,
        "fast" => 35,
        _ => 60, // "normal" and anything unrecognized
    };
    STOP_VELOCITY_MILLI.store(milli, Ordering::Relaxed);
}

/// Current absolute stop-velocity threshold in px/ms.
fn stop_velocity_threshold() -> f64 {
    STOP_VELOCITY_MILLI.load(Ordering::Relaxed) as f64 / 1000.0
}

/// Auto-space-between-swipes flag. When true, a swipe word that
/// follows another swipe (with no intervening word boundary) gets a
/// leading space inserted automatically. Mirrors the
/// `gestures.swipe_auto_space` config; set at boot and on settings
/// change. Read by the engine's SwipeWord handler.
static SWIPE_AUTO_SPACE: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(true);

pub fn set_swipe_auto_space(on: bool) {
    SWIPE_AUTO_SPACE.store(on, Ordering::Relaxed);
}

pub fn swipe_auto_space() -> bool {
    SWIPE_AUTO_SPACE.load(Ordering::Relaxed)
}

/// Swipe-typing enabled. When false, the freeform word-recognizer
/// path is skipped entirely — drags from the spacebar instead fall
/// through to the existing spacebar-arrow detection (Left/Right/Up/
/// Down), making the space key a pure cursor-arrow pad on drag.
/// Drags from regular letter keys become tap-or-cancel like any
/// Normal key. Mirrors `gestures.swipe_typing` config; set at boot
/// and on settings change. Read by the SwipeSource dispatch in
/// keyboard_view to decide whether to run the recognizer.
static SWIPE_TYPING_ENABLED: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(true);

pub fn set_swipe_typing_enabled(on: bool) {
    SWIPE_TYPING_ENABLED.store(on, Ordering::Relaxed);
}

pub fn swipe_typing_enabled() -> bool {
    SWIPE_TYPING_ENABLED.load(Ordering::Relaxed)
}

/// Process-start anchor for monotonic timestamps. Initialised on first
/// access. `SwipePoint::new` records `(now - start).as_millis()` as an
/// i64. Using `Instant` instead of `chrono::Utc::now()` avoids the
/// per-sample allocation/clock-syscall overhead that the old impl had,
/// and gives us monotonic timestamps (no clock-skew surprises during
/// long gestures crossing an NTP adjustment).
fn process_start() -> Instant {
    static START: OnceLock<Instant> = OnceLock::new();
    *START.get_or_init(Instant::now)
}

/// Milliseconds since the process started monotonic clock. Used as the
/// `timestamp_ms` field of `SwipePoint`. All timestamp consumers
/// (velocity, duration) work on relative differences only, so any
/// monotonic origin works.
pub fn now_ms() -> i64 {
    process_start().elapsed().as_millis() as i64
}

#[derive(Debug, Clone, Copy)]
pub struct SwipePoint {
    pub x: f64,
    pub y: f64,
    pub timestamp_ms: i64,
}

impl SwipePoint {
    pub fn new(x: f64, y: f64) -> Self {
        Self {
            x,
            y,
            timestamp_ms: now_ms(),
        }
    }

    /// Construct a SwipePoint with an explicit timestamp. Used by tests
    /// and any caller that wants reproducible velocity calculations.
    pub fn with_timestamp(x: f64, y: f64, timestamp_ms: i64) -> Self {
        Self { x, y, timestamp_ms }
    }
}

#[derive(Debug, Clone)]
pub struct KeyHitbox {
    pub key: Key,
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

impl KeyHitbox {
    pub fn contains(&self, x: f64, y: f64) -> bool {
        x >= self.x && x <= self.x + self.width && y >= self.y && y <= self.y + self.height
    }

    pub fn center(&self) -> (f64, f64) {
        (self.x + self.width / 2.0, self.y + self.height / 2.0)
    }

    pub fn distance_to(&self, x: f64, y: f64) -> f64 {
        let (cx, cy) = self.center();
        let dx = cx - x;
        let dy = cy - y;
        (dx * dx + dy * dy).sqrt()
    }

    /// Squared euclidean distance from `(x, y)` to this hitbox's
    /// center. Cheaper than `distance_to` (no sqrt) and produces
    /// identical orderings (sqrt is monotonic). Use for nearest-key
    /// lookups and min/max selection; use `distance_to` only when you
    /// need the value in real pixel units (e.g. for thresholds or
    /// arithmetic combining distance with other absolute-scale terms).
    pub fn distance_sq_to(&self, x: f64, y: f64) -> f64 {
        let (cx, cy) = self.center();
        let dx = cx - x;
        let dy = cy - y;
        dx * dx + dy * dy
    }
}

#[derive(Debug, Default)]
pub struct SwipeTracker {
    points: Vec<SwipePoint>,
    active: bool,
    /// Smoothing factor (0.0 = no smoothing, 1.0 = max smoothing).
    smoothing: f64,
    /// Minimum distance from the previous accepted point to record a new one.
    min_sample_distance: f64,
}

impl SwipeTracker {
    pub fn new(smoothing: f64) -> Self {
        Self {
            points: Vec::with_capacity(256),
            active: false,
            smoothing: smoothing.clamp(0.0, 1.0),
            min_sample_distance: 4.0,
        }
    }

    pub fn start(&mut self, x: f64, y: f64) {
        self.points.clear();
        self.points.push(SwipePoint::new(x, y));
        self.active = true;
    }

    /// Append a new sample to the path. Filters samples within
    /// `min_sample_distance` of the previous accepted point. Returns
    /// `true` if a point was actually pushed (passed the distance
    /// filter) so callers can avoid a second `current_path().len()`
    /// call to detect the push.
    pub fn update(&mut self, x: f64, y: f64) -> bool {
        if !self.active {
            return false;
        }
        if let Some(last) = self.points.last() {
            let dx = x - last.x;
            let dy = y - last.y;
            // Squared-distance comparison: skip the sqrt by squaring
            // the threshold. Mathematically equivalent for non-negative
            // values; ~3 ns saved per call on a 120Hz drag-update path
            // and consistent with the squared-distance pattern used
            // elsewhere (TAP_EVICTION, pivots_to_word, character-draw
            // stroke filter).
            let dist_sq = dx * dx + dy * dy;
            let min_sq = self.min_sample_distance * self.min_sample_distance;
            if dist_sq < min_sq {
                return false;
            }
            // Apply exponential smoothing: blend new point with previous.
            let blended_x = last.x * self.smoothing + x * (1.0 - self.smoothing);
            let blended_y = last.y * self.smoothing + y * (1.0 - self.smoothing);
            self.points.push(SwipePoint::new(blended_x, blended_y));
            true
        } else {
            false
        }
    }

    pub fn finish(&mut self) -> Vec<SwipePoint> {
        self.active = false;
        std::mem::take(&mut self.points)
    }

    pub fn cancel(&mut self) {
        self.active = false;
        self.points.clear();
    }

    pub fn is_active(&self) -> bool {
        self.active
    }

    pub fn current_path(&self) -> &[SwipePoint] {
        &self.points
    }

    /// True if the path is meaningfully long enough to be a swipe (and not
    /// just a tap).
    pub fn is_significant_swipe(&self, min_distance: f64) -> bool {
        if self.points.len() < 3 {
            return false;
        }
        let total = self
            .points
            .windows(2)
            .map(|w| {
                let dx = w[1].x - w[0].x;
                let dy = w[1].y - w[0].y;
                (dx * dx + dy * dy).sqrt()
            })
            .sum::<f64>();
        total >= min_distance
    }
}

/// Walk a swipe path and produce the candidate raw word — a sequence of
/// letters representing the keys the path snapped to.
///
/// We don't naively subsample every Nth point. Instead, we find the
/// **pivot points**: places where the user paused (low velocity) or
/// changed direction (sharp angle). On real swipes, these are exactly
/// where the intended letters live — the user smooths the curve elsewhere
/// but lingers on each character.
///
/// We always include the first and last point, since users land
/// deliberately on the start and end keys.
pub fn decode_swipe_path(path: &[SwipePoint], keys: &[KeyHitbox]) -> String {
    if path.is_empty() || keys.is_empty() {
        return String::new();
    }
    let pivots = extract_pivots(path);
    pivots_to_word(&pivots, keys)
}

/// Arc-curvature variant of `decode_swipe_path`. Uses
/// `extract_pivots_arc_curvature` to find pivot points based on local
/// curvature instead of turn-angle thresholds. Better for smooth
/// swipes where the user doesn't pause at each letter — the existing
/// turn-angle extractor's velocity-minimum criterion misses those.
///
/// Both decoders produce the same output format (a string of letters,
/// one per detected pivot, with adjacent duplicates removed).
/// Choose between them in the caller; future config could let the
/// user A/B them.
pub fn decode_swipe_path_arc(path: &[SwipePoint], keys: &[KeyHitbox]) -> String {
    if path.is_empty() || keys.is_empty() {
        return String::new();
    }
    let pivots = extract_pivots_arc_curvature(path);
    pivots_to_word(&pivots, keys)
}

/// Shared helper: walk a list of pivot points, snap each to the
/// nearest swipe-target key, dedup adjacent same-labels, and return
/// the resulting raw word. Both pivot extractors funnel through this.
fn pivots_to_word(pivots: &[SwipePoint], keys: &[KeyHitbox]) -> String {
    let mut word = String::new();
    let mut last_key_label: Option<String> = None;

    for point in pivots {
        let nearest = keys
            .iter()
            .filter(|k| k.key.swipe_target)
            .min_by(|a, b| {
                // Squared distance — same ordering as Euclidean,
                // one sqrt cheaper per comparison. The branch
                // body sees ~30 keys per pivot × ~5 pivots per
                // swipe = ~150 saved sqrts. Trivial individually,
                // but it's on the swipe-typing hot path.
                a.distance_sq_to(point.x, point.y)
                    .partial_cmp(&b.distance_sq_to(point.x, point.y))
                    .unwrap_or(std::cmp::Ordering::Equal)
            });

        if let Some(hit) = nearest {
            let label = hit.key.label.clone();
            if last_key_label.as_ref() != Some(&label) {
                word.push_str(&label);
                last_key_label = Some(label);
            }
        }
    }

    word
}

/// Find the "pivot" points along a swipe path — places where the user
/// paused or sharply changed direction. These are where the intended
/// letters live. The first and last point are always pivots.
///
/// The naive version of this function (look at v1, v2 between three
/// consecutive samples) gets fooled by sample-level noise: a vector
/// between two adjacent pixels has noisy direction even if the overall
/// path is smooth. So this implementation:
///
/// 1. Builds **smoothed direction vectors** by looking ahead/behind by a
///    window of ~20px of arc-length, not just one sample.
/// 2. Requires turns to be genuinely sharp (~70°+, not 40°) to register,
///    and the smoothed vectors to be substantial (>10px each) so we don't
///    react to micro-wobble.
/// 3. Uses an adaptive `min_pivot_separation` proportional to the path's
///    bounding-box diagonal — wider on a tablet, tighter on a phone.
/// 4. Detects velocity minima only on segments wider than 10px so we
///    don't pivot on every "stuck for one frame" sample.
///
/// The result is far fewer false pivots; one or two per intended letter
/// instead of one per wobble.
pub fn extract_pivots(path: &[SwipePoint]) -> Vec<SwipePoint> {
    if path.len() <= 2 {
        return path.to_vec();
    }

    let mut pivots: Vec<SwipePoint> = Vec::with_capacity(path.len() / 4);
    pivots.push(path[0]);

    // Cumulative arc-length per sample + per-segment raw velocities,
    // computed in a SINGLE pass. The old code computed `sqrt(dx²+dy²)`
    // twice per segment — once for arc-length, once for raw_velocity —
    // even though the same distance was wanted both times. Combining
    // halves the sqrt count in the hot pivot-extraction path
    // (~60 segments per swipe × 2 → 60 sqrts per swipe instead of
    // 120). Roughly 600ns saved per swipe on phone, negligible
    // individually but every nanosecond on a 60Hz drag-update path
    // helps avoid frame drops.
    let mut arc: Vec<f64> = Vec::with_capacity(path.len());
    arc.push(0.0);
    let mut raw_velocities: Vec<f64> = Vec::with_capacity(path.len().saturating_sub(1));
    for i in 1..path.len() {
        let dx = path[i].x - path[i - 1].x;
        let dy = path[i].y - path[i - 1].y;
        let d = (dx * dx + dy * dy).sqrt();
        arc.push(arc[i - 1] + d);
        let dt = ((path[i].timestamp_ms - path[i - 1].timestamp_ms).max(1)) as f64;
        raw_velocities.push(d / dt);
    }
    let total_arc = *arc.last().unwrap();

    // Acceleration smoothing: a 3-tap moving average over the raw
    // per-segment velocities. Touchscreen sampling is jittery — a
    // single slow frame (finger momentarily stuck, or a sampling
    // hiccup) used to register as a "pause" and spawn a spurious
    // pivot, over-segmenting the swipe into too many letters. The
    // smoothed series only dips when the finger ACTUALLY slows over
    // several samples, which is the deliberate "I'm landing on a
    // letter" motion we want to detect.
    let velocities: Vec<f64> = if raw_velocities.len() >= 3 {
        let n = raw_velocities.len();
        let mut out = Vec::with_capacity(n);
        for i in 0..n {
            let lo = i.saturating_sub(1);
            let hi = (i + 1).min(n - 1);
            // Sum the 1-3 surrounding velocities and average. Manual
            // bounds (rather than slice + iter().sum()) shaves a
            // function-call layer per iteration; per pivot extraction
            // this is N iterations (~60), trivial individually but
            // every drag_end runs this so it sits on a 60Hz cadence.
            let mut sum = 0.0;
            let mut count = 0.0;
            let mut j = lo;
            while j <= hi {
                sum += raw_velocities[j];
                count += 1.0;
                j += 1;
            }
            out.push(sum / count);
        }
        out
    } else {
        // Length < 3 — no smoothing applicable, consume raw directly
        // (avoid the redundant clone the old code did).
        raw_velocities
    };

    let max_v = velocities.iter().cloned().fold(0.0_f64, f64::max);
    // A "pause" requires both incoming and outgoing velocity to be in the
    // bottom 25% of observed speeds — and we also require the segments to
    // be at least a few pixels long so a momentarily-stuck sample doesn't
    // count.
    let v_pause_threshold = (max_v * 0.25).max(0.04);

    // Adaptive separation: use ~12% of the path's total arc length, but
    // clamped to a sane range. Floor raised to 50px (from 40) to further
    // discourage over-segmentation — two pivots closer than ~half a key
    // width are almost always one intended letter detected twice.
    let min_pivot_separation = (total_arc * 0.12).clamp(50.0, 120.0);

    // The look-ahead/behind window for smoothed direction vectors, in
    // pixels of arc length. Big enough to dwarf single-sample noise but
    // small enough that two real letters don't share one window.
    let dir_window_px = (min_pivot_separation * 0.5).clamp(20.0, 60.0);

    let mut last_pivot_arc = 0.0;

    for i in 2..path.len() - 2 {
        // 1. Smoothed incoming direction: from the sample at arc[i] - window
        //    to the current sample.
        let target_back_arc = arc[i] - dir_window_px;
        let back_idx = (0..i)
            .rev()
            .find(|&j| arc[j] <= target_back_arc)
            .unwrap_or(0);

        // 2. Smoothed outgoing direction: from current sample to one
        //    `dir_window_px` ahead.
        let target_fwd_arc = arc[i] + dir_window_px;
        let fwd_idx = (i + 1..path.len())
            .find(|&j| arc[j] >= target_fwd_arc)
            .unwrap_or(path.len() - 1);

        let p_back = path[back_idx];
        let p = path[i];
        let p_fwd = path[fwd_idx];

        let v1 = (p.x - p_back.x, p.y - p_back.y);
        let v2 = (p_fwd.x - p.x, p_fwd.y - p.y);
        let m1 = (v1.0 * v1.0 + v1.1 * v1.1).sqrt();
        let m2 = (v2.0 * v2.0 + v2.1 * v2.1).sqrt();

        // Both smoothed vectors must be at least 10px to have meaningful
        // direction. If either is shorter than that, this point is buried
        // in noise — skip it.
        let smoothed_dirs_ok = m1 >= 10.0 && m2 >= 10.0;

        // Real turn: require a SHARPER corner (80° vs the old 70°)
        // so a glancing curve during fast transit between two letters
        // doesn't get mistaken for a deliberate direction change. A
        // genuine "I turned here to hit this letter" corner is sharp;
        // a lazy arc through is not. cos(80°) ≈ 0.174.
        let is_corner = if smoothed_dirs_ok {
            let cos = (v1.0 * v2.0 + v1.1 * v2.1) / (m1 * m2);
            cos < 0.174
        } else {
            false
        };

        let v_in = velocities[i - 1];
        let v_out = velocities[i];
        let seg_in_dist = {
            let dx = path[i].x - path[i - 1].x;
            let dy = path[i].y - path[i - 1].y;
            (dx * dx + dy * dy).sqrt()
        };
        let seg_out_dist = {
            let dx = path[i + 1].x - path[i].x;
            let dy = path[i + 1].y - path[i].y;
            (dx * dx + dy * dy).sqrt()
        };

        // A STOP and a CORNER are two distinct, independent ways to
        // mark a letter — and a stop does NOT require a turn. You can
        // stop your thumb mid-segment in a straight line to add a
        // letter (very common on Dvorak, where same-row letter runs
        // mean lots of straight-line stops). So we detect a stop on
        // ABSOLUTE terms: a sustained near-zero dwell, independent of
        // whether the finger was fast elsewhere.
        //
        // STOP (absolute dwell): both smoothed in/out velocities are
        // genuinely near-zero. This catches the deliberate "pause to
        // place a letter" even in a uniformly slow, careful swipe
        // where there's no fast peak to dip away from. The threshold
        // is set by the "swipe speed mode" setting (slow/normal/fast).
        let abs_stop = stop_velocity_threshold();
        let is_stop = v_in < abs_stop && v_out < abs_stop;

        // RELATIVE DIP (slow-down during otherwise-fast travel): the
        // finger slows here relative to its surrounding peak speed,
        // without necessarily stopping. Catches the "decelerate onto
        // a letter then speed off" motion in faster swipes. We still
        // gate this on the pause threshold so it doesn't fire on
        // every minor speed wobble.
        let peak_window = 3usize;
        let before_peak = velocities[i.saturating_sub(peak_window)..i]
            .iter()
            .cloned()
            .fold(0.0_f64, f64::max);
        let after_hi = (i + peak_window).min(velocities.len());
        let after_peak = velocities[i..after_hi]
            .iter()
            .cloned()
            .fold(0.0_f64, f64::max);
        let surrounding_peak = before_peak.max(after_peak);
        let is_relative_dip = v_in < v_pause_threshold
            && v_out < v_pause_threshold
            && (v_in + v_out) * 0.5 <= surrounding_peak * 0.55;

        // A letter-marking slowdown is EITHER an absolute stop OR a
        // relative dip. The "did the finger actually travel here"
        // sanity check differs between them:
        //
        //  - A STOP is a dwell, so its ADJACENT segments are near-zero
        //    by definition — gating it on adjacent segment length
        //    would reject the very stops we want. Instead we confirm
        //    real travel in an arc-length window AROUND the dwell: the
        //    finger moved into this spot and will move out of it. This
        //    distinguishes a deliberate pause-mid-stroke from the
        //    finger simply being lifted/idle at the path ends.
        //  - A RELATIVE DIP is a moving slowdown, so its adjacent
        //    segments genuinely move; the original >2px gate applies.
        let stop_has_travel = {
            // Walk outward from the dwell until we find a sample that
            // is genuinely displaced (≥ 6px) from the dwell point, on
            // each side. A deliberate pause can span several samples,
            // so a fixed index offset could land INSIDE the dwell and
            // read ~zero; walking past it finds the real approach and
            // departure. Bounded so we don't scan the whole path.
            const TRAVEL_MIN_PX: f64 = 6.0;
            const MAX_WALK: usize = 12;
            let displaced = |from: usize, to: usize| {
                let dx = path[to].x - path[from].x;
                let dy = path[to].y - path[from].y;
                (dx * dx + dy * dy).sqrt() >= TRAVEL_MIN_PX
            };
            let mut found_before = false;
            let mut j = i;
            for _ in 0..MAX_WALK {
                if j == 0 {
                    break;
                }
                j -= 1;
                if displaced(j, i) {
                    found_before = true;
                    break;
                }
            }
            let mut found_after = false;
            let mut k = i;
            for _ in 0..MAX_WALK {
                if k + 1 >= path.len() {
                    break;
                }
                k += 1;
                if displaced(i, k) {
                    found_after = true;
                    break;
                }
            }
            // Real movement on BOTH sides → an interior letter stop,
            // not the finger settling at the start or lifting at the
            // end.
            found_before && found_after
        };
        let is_velocity_min = (is_stop && stop_has_travel)
            || (is_relative_dip && seg_in_dist > 2.0 && seg_out_dist > 2.0);

        if is_corner || is_velocity_min {
            // Dedupe by arc-length distance, not Euclidean — a swipe that
            // doubles back close to a previous pivot is still a separate
            // letter.
            if arc[i] - last_pivot_arc >= min_pivot_separation {
                pivots.push(p);
                last_pivot_arc = arc[i];
            }
        }
    }

    // Always include the endpoint, dedup'd against the previous pivot by
    // arc length.
    let last_idx = path.len() - 1;
    if total_arc - last_pivot_arc >= min_pivot_separation {
        pivots.push(path[last_idx]);
    } else if pivots.len() > 1 {
        // Replace the final pivot with the endpoint — endpoints are more
        // reliable than near-final wobble.
        *pivots.last_mut().unwrap() = path[last_idx];
    } else {
        pivots.push(path[last_idx]);
    }

    pivots
}

/// Pivot extraction using **arc-curvature analysis**, as an alternative
/// to `extract_pivots` (which uses turn-angle + velocity-pause heuristics).
///
/// **Motivation:** smooth swipes where the user doesn't pause at each
/// letter — common on phone touchscreens — produce few velocity minima
/// and gentle turn angles. The turn-angle threshold rejects these as
/// "noise" and the resulting word ends up under-segmented.
///
/// **Algorithm:**
///
/// 1. Resample the path at uniform arc-length intervals so curvature
///    isn't biased by where the user was fast/slow.
/// 2. For each interior sample, fit a circle through it and its
///    neighbors at fixed arc-distance away (Menger curvature: the
///    reciprocal of the circumradius of three points).
/// 3. Smooth the curvature signal (moving average over a small window).
/// 4. Find local maxima of the smoothed curvature whose magnitude
///    exceeds a threshold (~1/cell_size: a turn tight enough to fit
///    inside one key).
/// 5. Combine with start, end, and any sustained velocity minima.
/// 6. Dedup pivots that are closer than min_pivot_separation in arc-length.
///
/// **Why arcs and not lines:** finger paths between intended letters
/// curve smoothly. Trying to fit a polyline picks up wobble that an
/// arc-fit absorbs. Curvature is a local property of an arc — its
/// midpoint approximation is robust to the exact sample density.
///
/// Returns pivots in the same coordinate frame as input.
pub fn extract_pivots_arc_curvature(path: &[SwipePoint]) -> Vec<SwipePoint> {
    if path.len() <= 2 {
        return path.to_vec();
    }

    // ---- 1. Resample at uniform arc-length intervals ----
    let mut arc: Vec<f64> = Vec::with_capacity(path.len());
    arc.push(0.0);
    for i in 1..path.len() {
        let dx = path[i].x - path[i - 1].x;
        let dy = path[i].y - path[i - 1].y;
        let d = (dx * dx + dy * dy).sqrt();
        arc.push(arc[i - 1] + d);
    }
    let total_arc = *arc.last().unwrap();
    if total_arc < 1.0 {
        return path.to_vec();
    }

    // Resample to ~20 evenly-spaced samples (or one per 10px, whichever
    // is more). Resampling lets curvature analysis ignore the user's
    // velocity profile — a slow drag produces dense samples, a fast
    // one sparse; curvature should be the same in both cases.
    let resample_step_px: f64 = 10.0;
    let n_resampled = ((total_arc / resample_step_px).ceil() as usize).clamp(8, 256);
    let step = total_arc / (n_resampled - 1) as f64;

    let mut resampled: Vec<SwipePoint> = Vec::with_capacity(n_resampled);
    resampled.push(path[0]);
    let mut idx = 0;
    for k in 1..n_resampled - 1 {
        let target = k as f64 * step;
        while idx + 1 < path.len() && arc[idx + 1] < target {
            idx += 1;
        }
        // Linear interp between idx and idx+1.
        if idx + 1 >= path.len() {
            resampled.push(path[path.len() - 1]);
        } else {
            let span = arc[idx + 1] - arc[idx];
            let t = if span > 0.0 {
                (target - arc[idx]) / span
            } else {
                0.0
            };
            let x = path[idx].x + t * (path[idx + 1].x - path[idx].x);
            let y = path[idx].y + t * (path[idx + 1].y - path[idx].y);
            let ts = path[idx].timestamp_ms
                + ((path[idx + 1].timestamp_ms - path[idx].timestamp_ms) as f64 * t) as i64;
            resampled.push(SwipePoint::with_timestamp(x, y, ts));
        }
    }
    resampled.push(path[path.len() - 1]);

    // ---- 2. Compute Menger curvature at each interior sample ----
    // Menger curvature of three points (a, b, c) is 4 * area(triangle) /
    // (|ab| * |bc| * |ca|), where area uses the cross product. Result
    // is 1/circumradius — high for tight turns, near-zero for straight
    // segments. Sign is irrelevant here (left vs right turn both count).
    let n = resampled.len();
    let mut curvature = vec![0.0_f64; n];
    for i in 1..n - 1 {
        let a = &resampled[i - 1];
        let b = &resampled[i];
        let c = &resampled[i + 1];
        let ab_x = b.x - a.x;
        let ab_y = b.y - a.y;
        let bc_x = c.x - b.x;
        let bc_y = c.y - b.y;
        let ca_x = a.x - c.x;
        let ca_y = a.y - c.y;
        let ab_len = (ab_x * ab_x + ab_y * ab_y).sqrt();
        let bc_len = (bc_x * bc_x + bc_y * bc_y).sqrt();
        let ca_len = (ca_x * ca_x + ca_y * ca_y).sqrt();
        let denom = ab_len * bc_len * ca_len;
        if denom < 1e-6 {
            continue;
        }
        // Twice signed area of triangle abc.
        let cross = ab_x * bc_y - ab_y * bc_x;
        curvature[i] = 2.0 * cross.abs() / denom;
    }

    // ---- 3. Smooth curvature with a 3-sample moving average ----
    let mut smoothed = curvature.clone();
    for i in 1..n - 1 {
        smoothed[i] = (curvature[i - 1] + curvature[i] + curvature[i + 1]) / 3.0;
    }

    // ---- 4. Find local maxima above threshold ----
    // Threshold: 1/cell_size where cell_size is approximated as ~60px
    // (typical key width). A turn tighter than one cell is a real pivot.
    let cell_size_px: f64 = 60.0;
    let curvature_threshold = 1.0 / cell_size_px; // 1/60 ≈ 0.0167
    let min_pivot_separation = (total_arc * 0.12).clamp(40.0, 120.0);

    let mut pivots: Vec<SwipePoint> = Vec::new();
    pivots.push(resampled[0]);
    let mut last_pivot_arc = 0.0;
    let resample_arc_per_step = step;

    for i in 1..n - 1 {
        if smoothed[i] < curvature_threshold {
            continue;
        }
        // Local maximum check.
        if smoothed[i] < smoothed[i - 1] || smoothed[i] < smoothed[i + 1] {
            continue;
        }
        // Min-separation check.
        let this_arc = i as f64 * resample_arc_per_step;
        if this_arc - last_pivot_arc < min_pivot_separation {
            continue;
        }
        pivots.push(resampled[i]);
        last_pivot_arc = this_arc;
    }

    // Always include the endpoint, replacing nearby final pivot if any.
    let last_idx = n - 1;
    if total_arc - last_pivot_arc >= min_pivot_separation {
        pivots.push(resampled[last_idx]);
    } else if pivots.len() > 1 {
        *pivots.last_mut().unwrap() = resampled[last_idx];
    } else {
        pivots.push(resampled[last_idx]);
    }

    pivots
}

/// Compute the average velocity of a swipe path in pixels-per-millisecond.
/// Returns 0.0 for paths shorter than 2 samples or paths with degenerate
/// timestamps.
///
/// Used by speed-aware swipe recognition to balance dictionary-cleanup vs
/// raw-input trust:
/// - Slow swipes (deliberate input) — trust raw, ease up on the dictionary
/// - Fast swipes (sloppy input) — lean harder on the dictionary
pub fn path_avg_velocity(path: &[SwipePoint]) -> f64 {
    if path.len() < 2 {
        return 0.0;
    }
    let total_dist: f64 = path
        .windows(2)
        .map(|w| {
            let dx = w[1].x - w[0].x;
            let dy = w[1].y - w[0].y;
            (dx * dx + dy * dy).sqrt()
        })
        .sum();
    let dt_ms = (path.last().unwrap().timestamp_ms - path[0].timestamp_ms).max(1) as f64;
    total_dist / dt_ms
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::keyboard::Key;

    fn hitbox(label: &str, x: f64, y: f64) -> KeyHitbox {
        KeyHitbox {
            key: Key::character(label),
            x,
            y,
            width: 50.0,
            height: 50.0,
        }
    }

    /// Synthesize a swipe path that pauses at each "letter" position. The
    /// pause = three samples in a tight cluster with rising timestamps,
    /// which both lowers velocity and creates a near-zero displacement
    /// region — exactly what `extract_pivots` is designed to detect.
    fn synthesize_swipe_through(letters: &[(f64, f64)]) -> Vec<SwipePoint> {
        let mut path = Vec::new();
        let mut t: i64 = 0;
        for (i, &(lx, ly)) in letters.iter().enumerate() {
            // Approach: 3 fast samples leading to the letter.
            if i > 0 {
                let (px, py) = letters[i - 1];
                for k in 1..=3 {
                    let f = k as f64 / 4.0;
                    let x = px + (lx - px) * f;
                    let y = py + (ly - py) * f;
                    t += 20;
                    path.push(SwipePoint::with_timestamp(x, y, t));
                }
            }
            // Pause: 3 samples within a few px of the letter, slow time.
            for _ in 0..3 {
                t += 60;
                path.push(SwipePoint::with_timestamp(lx, ly, t));
            }
        }
        path
    }

    #[test]
    fn tracker_starts_empty() {
        let t = SwipeTracker::new(0.3);
        assert!(!t.is_active());
        assert!(t.current_path().is_empty());
    }

    #[test]
    fn tracker_records_significant_movement() {
        let mut t = SwipeTracker::new(0.0); // no smoothing for predictable test
        t.start(0.0, 0.0);
        for i in 1..20 {
            t.update(i as f64 * 10.0, 0.0);
        }
        let path = t.finish();
        assert!(path.len() > 5, "should have collected several points");
    }

    #[test]
    fn tracker_skips_tiny_movements() {
        let mut t = SwipeTracker::new(0.0);
        t.start(0.0, 0.0);
        for _ in 0..10 {
            t.update(0.5, 0.5); // below min_sample_distance
        }
        let path = t.finish();
        assert_eq!(path.len(), 1, "tiny movements should be ignored");
    }

    #[test]
    fn extract_pivots_finds_velocity_minima() {
        // Path that pauses at three positions:
        let path = synthesize_swipe_through(&[(10.0, 10.0), (110.0, 10.0), (210.0, 10.0)]);
        let pivots = extract_pivots(&path);
        // We should see ~3 pivots — one per letter.
        assert!(
            pivots.len() >= 2 && pivots.len() <= 6,
            "got {} pivots: {:?}",
            pivots.len(),
            pivots
        );
    }

    #[test]
    fn stop_midline_on_slow_swipe_marks_a_letter() {
        // The Dvorak case: a deliberate STOP partway through a
        // straight horizontal swipe, where the whole motion is slow
        // and steady (no fast peaks). The absolute-stop detector must
        // catch the dwell even though there's no velocity peak to dip
        // away from. No direction change — purely a pause.
        let mut path = Vec::new();
        let mut t: i64 = 0;
        // Slow, steady approach to x=120 (constant ~0.3 px/ms).
        for step in 0..=12 {
            let x = 10.0 + step as f64 * 9.0; // 9px per 30ms ≈ 0.3 px/ms
            t += 30;
            path.push(SwipePoint::with_timestamp(x, 50.0, t));
        }
        // DWELL at x≈118 for several samples (near-zero velocity).
        let dwell_x = path.last().unwrap().x;
        for _ in 0..4 {
            t += 70;
            path.push(SwipePoint::with_timestamp(dwell_x, 50.0, t));
        }
        // Continue slow and straight to the end (same direction).
        for step in 1..=12 {
            let x = dwell_x + step as f64 * 9.0;
            t += 30;
            path.push(SwipePoint::with_timestamp(x, 50.0, t));
        }
        let pivots = extract_pivots(&path);
        // Endpoints are always pivots (2). A detected mid-line stop
        // adds at least one interior pivot → 3+. If the stop were
        // missed we'd see only the 2 endpoints.
        assert!(
            pivots.len() >= 3,
            "mid-line stop on a slow swipe should add an interior pivot; got {}: {:?}",
            pivots.len(),
            pivots
        );
    }

    #[test]
    fn extract_pivots_handles_short_paths() {
        let p = vec![
            SwipePoint::with_timestamp(0.0, 0.0, 0),
            SwipePoint::with_timestamp(10.0, 0.0, 10),
        ];
        let pivots = extract_pivots(&p);
        assert_eq!(pivots.len(), 2);
    }

    #[test]
    fn decode_path_produces_word() {
        let keys = vec![
            hitbox("h", 0.0, 0.0),
            hitbox("e", 60.0, 0.0),
            hitbox("l", 120.0, 0.0),
            hitbox("o", 180.0, 0.0),
        ];
        // Pause at h, e, l, o positions (centers).
        let path = synthesize_swipe_through(&[
            (25.0, 25.0),
            (85.0, 25.0),
            (145.0, 25.0),
            (205.0, 25.0),
        ]);
        let word = decode_swipe_path(&path, &keys);
        // Should at minimum include start and end keys.
        assert!(word.contains('h'), "word: {}", word);
        assert!(word.contains('o'), "word: {}", word);
    }

    #[test]
    fn decode_collapses_consecutive_duplicates() {
        let keys = vec![hitbox("a", 0.0, 0.0)];
        let path: Vec<SwipePoint> = (0..10)
            .map(|i| SwipePoint::with_timestamp(25.0, 25.0, i * 16))
            .collect();
        let word = decode_swipe_path(&path, &keys);
        // All points hit the same key — should produce just "a".
        assert_eq!(word, "a");
    }

    // -------------------------------------------------------------------
    // Bug-reproduction tests: "swipe-typing line shows up but does not
    // write on alphabet layout". The user sees the trail render during
    // the drag but no text is committed at drag_end.
    //
    // The failure mode reduces to `decode_swipe_path` returning empty:
    // recognize_with_speed early-returns []  on empty raw, and the
    // dispatch loop's `chosen.unwrap_or(raw)` fallback is also empty,
    // so SwipeWord never fires.
    //
    // These tests build a realistic Dvorak ortholinear hitbox set
    // matching what `default_dvorak_ortholinear()` would render at
    // typical phone scale (60-px-wide keys, 10 cols × 4 rows), then
    // run the production decode path on a synthesized "the" swipe.
    // -------------------------------------------------------------------

    /// Build a Dvorak ortholinear hitbox layout at typical phone scale.
    /// 10 columns × 4 rows, 60-px-wide keys, no row gaps. Letters
    /// positioned exactly like `Layout::default_dvorak_ortholinear()`
    /// row 2 (home row) and row 3 (bottom letters). Top row (row 1)
    /// and utility row 4 (?123/voice/llm/space/z/slash/return) are
    /// included for completeness so the swipe path can land on any
    /// realistic key.
    fn make_dvorak_boxes() -> Vec<KeyHitbox> {
        let key_w = 60.0;
        let key_h = 46.0;
        let row_y = |r: usize| (r as f64) * (key_h + 3.0); // 3px row spacing
        let col_x = |c: usize| (c as f64) * key_w;
        let mk = |label: &str, c: usize, r: usize| KeyHitbox {
            key: Key::character(label),
            x: col_x(c),
            y: row_y(r),
            width: key_w,
            height: key_h,
        };
        // Row 1: ' , . p y f g c r l
        // Row 2: a o e u i d h t n s
        // Row 3: q j k x b m w v   (positions 1..9; row 3 also has shift+backspace)
        let row1 = ["'", ",", ".", "p", "y", "f", "g", "c", "r", "l"];
        let row2 = ["a", "o", "e", "u", "i", "d", "h", "t", "n", "s"];
        // Row 3 letter positions match the layout: cols 1-8 are q-v
        // (col 0 is shift, col 9 is backspace).
        let row3 = ["q", "j", "k", "x", "b", "m", "w", "v"];
        let mut boxes = Vec::new();
        for (c, &l) in row1.iter().enumerate() {
            boxes.push(mk(l, c, 0));
        }
        for (c, &l) in row2.iter().enumerate() {
            boxes.push(mk(l, c, 1));
        }
        for (c, &l) in row3.iter().enumerate() {
            boxes.push(mk(l, c + 1, 2));
        }
        boxes
    }

    /// Synthesize a swipe path through letters at the given centers,
    /// without smoothing. Each "letter visit" is 3 paused samples at
    /// the letter center with rising timestamps (so velocity dips),
    /// then 3 transit samples toward the next letter. Mirrors the
    /// `synthesize_swipe_through` helper above but produces a more
    /// realistic 60Hz-ish sample density.
    fn synth_swipe_60hz(centers: &[(f64, f64)]) -> Vec<SwipePoint> {
        let mut path = Vec::new();
        let mut t: i64 = 0;
        for (i, &(lx, ly)) in centers.iter().enumerate() {
            if i > 0 {
                let (px, py) = centers[i - 1];
                for k in 1..=5 {
                    let f = k as f64 / 6.0;
                    let x = px + (lx - px) * f;
                    let y = py + (ly - py) * f;
                    t += 16; // ~60Hz
                    path.push(SwipePoint::with_timestamp(x, y, t));
                }
            }
            for _ in 0..4 {
                t += 30; // pause: longer dwell at letter
                path.push(SwipePoint::with_timestamp(lx, ly, t));
            }
        }
        path
    }

    /// Hitbox center helper for the Dvorak layout (matches make_dvorak_boxes).
    fn dvorak_center(boxes: &[KeyHitbox], label: &str) -> (f64, f64) {
        let b = boxes
            .iter()
            .find(|b| b.key.label == label)
            .unwrap_or_else(|| panic!("no key '{}' in Dvorak boxes", label));
        b.center()
    }

    #[test]
    fn dvorak_the_swipe_decodes_letters() {
        // Realistic Dvorak hitboxes + "the" path WITHOUT smoothing.
        // Tests the decode_swipe_path pipeline in isolation: if this
        // returns empty, the bug is in pivots / hit-test / filter
        // logic at the recognizer level — reproducible without any
        // device interaction.
        let boxes = make_dvorak_boxes();
        let t = dvorak_center(&boxes, "t");
        let h = dvorak_center(&boxes, "h");
        let e = dvorak_center(&boxes, "e");
        let path = synth_swipe_60hz(&[t, h, e]);
        let word = decode_swipe_path(&path, &boxes);
        // Should contain at least the start (t) and end (e) letters —
        // pivot extraction reliably catches paused-on-letter samples.
        // h in the middle may or may not be detected depending on the
        // pivot algorithm's sensitivity; the user can swipe-type "tee"
        // → "the" (recognizer dictionary-snaps). The hard fail would
        // be empty output.
        assert!(
            !word.is_empty(),
            "decode_swipe_path returned empty for a Dvorak 'the' swipe; \
             word='{}', boxes.len={}, swipe_target_count={}",
            word,
            boxes.len(),
            boxes.iter().filter(|b| b.key.swipe_target).count()
        );
        assert!(
            word.contains('t') || word.contains('e'),
            "decoded word '{}' should include at least one of 't' or 'e'",
            word
        );
    }

    #[test]
    fn dvorak_swipe_with_production_smoothing() {
        // Same path, but pushed through SwipeTracker with the
        // production smoothing=0.3 — mirrors what actually happens
        // on-device (drag_update applies smoothing per-sample).
        // If unsmoothed passes but this fails, the smoothing is
        // destroying the recognizer's ability to find pivots.
        let boxes = make_dvorak_boxes();
        let t = dvorak_center(&boxes, "t");
        let h = dvorak_center(&boxes, "h");
        let e = dvorak_center(&boxes, "e");
        let raw_path = synth_swipe_60hz(&[t, h, e]);

        let mut tracker = SwipeTracker::new(0.3);
        tracker.start(raw_path[0].x, raw_path[0].y);
        for p in &raw_path[1..] {
            tracker.update(p.x, p.y);
        }
        let smoothed = tracker.finish();

        let word = decode_swipe_path(&smoothed, &boxes);
        assert!(
            !word.is_empty(),
            "decode_swipe_path returned empty for smoothed Dvorak 'the' \
             swipe; word='{}', smoothed.len={}",
            word,
            smoothed.len()
        );
    }

    #[test]
    fn dvorak_swipe_offkey_start_still_decodes() {
        // Touch lands outside any hitbox (e.g. in the row gap, or in
        // bottom padding), drags into the keyboard, then crosses
        // letters. The drag_begin SwipeSource fallback already lets
        // this gesture run through dispatch_swipe_source; the
        // remaining question is whether pivots_to_word's nearest-key
        // mapping handles the off-key start gracefully.
        let boxes = make_dvorak_boxes();
        let h = dvorak_center(&boxes, "h");
        let e = dvorak_center(&boxes, "e");
        // Start 50px below the bottom row — well outside any hitbox.
        let start = (h.0, h.1 + 200.0);
        let path = synth_swipe_60hz(&[start, h, e]);
        let word = decode_swipe_path(&path, &boxes);
        assert!(
            !word.is_empty(),
            "decode_swipe_path returned empty for an off-key-start Dvorak \
             swipe; word='{}'",
            word
        );
    }
}
