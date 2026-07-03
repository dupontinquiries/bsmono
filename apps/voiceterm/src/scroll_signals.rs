// src/scroll_signals.rs
//
// v0.1.183 (ship 1b-β.1) — scroll signals plumbing for the Moment
// classifier and future analytics-flip consumers.
//
// User direction (v0.1.176, v0.1.181):
//   "scroll velocity, pauses, reverse-scrolls, and document-position
//    all derive context cheaply from data we already have."
//
// This module exposes scroll-derived signals as cheap atomic reads.
// QML pushes raw contentY samples via the backend's qt_method;
// this module derives velocity, applies smoothing, and answers
// queries like "is the user reading-paced right now?"
//
// 1b-β.1 ships:
//   - push_sample(content_y, now_ms): QML pushes raw samples
//   - velocity_ema(): smoothed velocity (px/sec)
//   - is_reading_paced(): boolean for Moment classifier
//   - sample_age_ms(): how stale is the latest sample
//
// Designed for the Moment::current() call site to read these
// cheaply. Multiple readers, single writer (the QML side).
//
// Thresholds based on intuition; tune from real logs.

use std::sync::atomic::{AtomicU64, AtomicI64, Ordering};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

/// Last scroll velocity in pixels-per-second, scaled by 1000 and
/// stored as integer for atomic access. Signed because scrolling
/// up gives negative velocity; we mostly care about magnitude but
/// preserving sign lets us detect reverse-scrolls later (ship 4).
static VELOCITY_EMA_MILLI: AtomicI64 = AtomicI64::new(0);

/// Latest contentY value, scaled by 1000 for atomic access.
static LAST_CONTENT_Y_MILLI: AtomicI64 = AtomicI64::new(0);

/// Unix millis of the most recent sample.
static LAST_SAMPLE_MS: AtomicU64 = AtomicU64::new(0);

/// Unix millis when the user last had non-trivial velocity. Used
/// to detect "user has been at rest" (which is different from
/// "user has been reading slowly").
static LAST_MOVEMENT_MS: AtomicU64 = AtomicU64::new(0);

/// Unix millis when reading-pace started. 0 means "not currently
/// reading-paced." When velocity drops into the reading-paced
/// range, this gets set; if velocity exits the range or user goes
/// to rest, it gets cleared.
static READING_PACE_STARTED_MS: AtomicU64 = AtomicU64::new(0);

/// EMA decay factor — how much weight to give each new sample.
/// 0.3 = new sample has 30% influence; previous EMA keeps 70%.
/// Lower = smoother but laggier; higher = more responsive but
/// noisier. 0.3 is a reasonable middle.
const EMA_ALPHA: f64 = 0.3;

/// Velocity threshold (px/sec) below which we consider scrolling
/// "reading paced." Above this is skimming or jump-navigation.
/// 200 px/sec is roughly 10 lines per second at typical line
/// heights — that's "skimming." Reading is well below that.
const READING_PACE_THRESHOLD: f64 = 200.0;

/// How long velocity must stay in the reading-paced range before
/// we report `is_reading_paced = true`. Prevents transient slowdowns
/// (between flick gestures) from being misread as reading.
const SUSTAINED_READING_MS: u64 = 8_000;

/// If velocity stays at ~0 for this long, user is "at rest" rather
/// than reading. Tab switched, walked away, etc. Clears the
/// reading-pace state.
const REST_TIMEOUT_MS: u64 = 30_000;

/// Velocity magnitude below this counts as "at rest" (not even
/// slow reading). A bit of jitter from system noise is fine.
const REST_VELOCITY_THRESHOLD: f64 = 5.0;

/// Mutex protecting the EMA update path. Atomics handle reads;
/// the write path needs to read-modify-write multiple fields
/// coherently. Lock is very short-held.
static UPDATE_LOCK: Mutex<()> = Mutex::new(());

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Push a raw scroll sample. Called from QML's
/// `browseFlick.onContentYChanged` handler (throttled to ~10/sec).
///
/// `content_y` is in pixels.
pub fn push_sample(content_y: f64) {
    // v0.1.283: defensive — QML's Flickable.contentY is always
    // finite in practice, but NaN or Inf as i64 in newer Rust
    // (post-1.45) silently clamps to 0, which would silently
    // pollute scroll signals with bad data. Bail out instead.
    if !content_y.is_finite() {
        return;
    }
    let now = now_ms();
    let _guard = UPDATE_LOCK.lock().unwrap();

    let prev_ms = LAST_SAMPLE_MS.load(Ordering::Relaxed);
    let prev_y_milli = LAST_CONTENT_Y_MILLI.load(Ordering::Relaxed);
    let prev_y = prev_y_milli as f64 / 1000.0;

    // First sample after fresh start: just record, no velocity yet.
    if prev_ms == 0 {
        LAST_CONTENT_Y_MILLI.store((content_y * 1000.0) as i64, Ordering::Relaxed);
        LAST_SAMPLE_MS.store(now, Ordering::Relaxed);
        LAST_MOVEMENT_MS.store(now, Ordering::Relaxed);
        return;
    }

    let dt_ms = now.saturating_sub(prev_ms);
    // Guard against very short or very long deltas. Both produce
    // noise: too-short = jitter near 0; too-long = stale data
    // from a previous session or pause.
    if dt_ms < 20 {
        // Sample too fresh, ignore (caller should throttle but be
        // defensive). Don't update prev — wait for a meaningful gap.
        return;
    }
    if dt_ms > 5_000 {
        // Gap too large — page transition or pause. Reset state.
        LAST_CONTENT_Y_MILLI.store((content_y * 1000.0) as i64, Ordering::Relaxed);
        LAST_SAMPLE_MS.store(now, Ordering::Relaxed);
        LAST_MOVEMENT_MS.store(now, Ordering::Relaxed);
        READING_PACE_STARTED_MS.store(0, Ordering::Relaxed);
        VELOCITY_EMA_MILLI.store(0, Ordering::Relaxed);
        return;
    }

    // Compute instantaneous velocity (signed).
    let dy = content_y - prev_y;
    let inst_velocity = dy / (dt_ms as f64 / 1000.0);

    // Update EMA. abs() for magnitude classification.
    let prev_ema = VELOCITY_EMA_MILLI.load(Ordering::Relaxed) as f64 / 1000.0;
    let new_ema = EMA_ALPHA * inst_velocity + (1.0 - EMA_ALPHA) * prev_ema;

    let abs_v = new_ema.abs();

    // Update movement timestamp if there was real movement.
    if abs_v > REST_VELOCITY_THRESHOLD {
        LAST_MOVEMENT_MS.store(now, Ordering::Relaxed);
    }

    // Update reading-pace state machine.
    let pace_started = READING_PACE_STARTED_MS.load(Ordering::Relaxed);
    if abs_v > REST_VELOCITY_THRESHOLD && abs_v < READING_PACE_THRESHOLD {
        // In reading-paced range. Start the timer if not already.
        if pace_started == 0 {
            READING_PACE_STARTED_MS.store(now, Ordering::Relaxed);
        }
        // Else: keep the existing start time; reading-pace is
        // sustained if it's been long enough.
    } else if abs_v >= READING_PACE_THRESHOLD {
        // Too fast — definitely not reading. Clear.
        READING_PACE_STARTED_MS.store(0, Ordering::Relaxed);
    } else {
        // abs_v <= REST_VELOCITY_THRESHOLD — at rest. Don't clear
        // immediately; user may have paused to think. Clear only
        // after REST_TIMEOUT_MS via the read path.
    }

    // Commit updates.
    VELOCITY_EMA_MILLI.store((new_ema * 1000.0) as i64, Ordering::Relaxed);
    LAST_CONTENT_Y_MILLI.store((content_y * 1000.0) as i64, Ordering::Relaxed);
    LAST_SAMPLE_MS.store(now, Ordering::Relaxed);
}

/// Smoothed velocity in pixels-per-second. Sign indicates
/// direction (positive = scrolling down).
pub fn velocity_ema() -> f64 {
    VELOCITY_EMA_MILLI.load(Ordering::Relaxed) as f64 / 1000.0
}

/// How long ago (ms) was the most recent sample. Useful for
/// detecting "no scroll activity in a while."
pub fn sample_age_ms() -> u64 {
    let last = LAST_SAMPLE_MS.load(Ordering::Relaxed);
    if last == 0 { return u64::MAX; }
    now_ms().saturating_sub(last)
}

/// True if scroll velocity has been in the reading-paced range
/// (slow but non-zero) for at least SUSTAINED_READING_MS.
///
/// Returns false if:
///   - No samples yet
///   - Last sample is stale (>5s old)
///   - User has been at rest too long
///   - Velocity is too fast or too slow
///   - Reading pace hasn't been sustained long enough yet
pub fn is_reading_paced() -> bool {
    let now = now_ms();
    let last_sample = LAST_SAMPLE_MS.load(Ordering::Relaxed);
    if last_sample == 0 { return false; }

    // Stale samples → no signal.
    if now.saturating_sub(last_sample) > 5_000 {
        return false;
    }

    // User has been at rest too long → not reading, doing
    // something else.
    let last_movement = LAST_MOVEMENT_MS.load(Ordering::Relaxed);
    if last_movement > 0 && now.saturating_sub(last_movement) > REST_TIMEOUT_MS {
        return false;
    }

    let pace_started = READING_PACE_STARTED_MS.load(Ordering::Relaxed);
    if pace_started == 0 { return false; }

    let pace_duration = now.saturating_sub(pace_started);
    pace_duration >= SUSTAINED_READING_MS
}

/// For diagnostic logging.
pub fn describe() -> String {
    let v = velocity_ema();
    let age = sample_age_ms();
    let paced = is_reading_paced();
    let pace_started = READING_PACE_STARTED_MS.load(Ordering::Relaxed);
    let pace_age = if pace_started == 0 { 0 } else { now_ms().saturating_sub(pace_started) };
    format!("v={:.1}px/s age={}ms paced={} pace_age={}ms", v, age, paced, pace_age)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread::sleep;
    use std::time::Duration;

    fn reset() {
        VELOCITY_EMA_MILLI.store(0, Ordering::Relaxed);
        LAST_CONTENT_Y_MILLI.store(0, Ordering::Relaxed);
        LAST_SAMPLE_MS.store(0, Ordering::Relaxed);
        LAST_MOVEMENT_MS.store(0, Ordering::Relaxed);
        READING_PACE_STARTED_MS.store(0, Ordering::Relaxed);
    }

    // Note: these tests use real time (sleep). They're slow but
    // correctly exercise the time-based state machine. Skip with
    // `cargo test --skip slow` in fast iteration.

    #[test]
    fn first_sample_no_velocity() {
        reset();
        push_sample(0.0);
        assert_eq!(velocity_ema(), 0.0);
        assert!(!is_reading_paced());
    }

    #[test]
    fn fast_scroll_not_reading() {
        reset();
        push_sample(0.0);
        sleep(Duration::from_millis(100));
        push_sample(500.0); // 5000 px/sec — fast skim
        assert!(velocity_ema().abs() > READING_PACE_THRESHOLD);
        assert!(!is_reading_paced());
    }

    #[test]
    fn slow_scroll_eventually_reading() {
        reset();
        let mut y = 0.0;
        // Simulate ~50 px/sec sustained scroll for >SUSTAINED_READING_MS.
        // 10 samples 100ms apart = 1 second of scroll. Need 8+ seconds
        // for is_reading_paced to fire. This test takes ~9s; mark slow.
        push_sample(y);
        for _ in 0..90 {
            sleep(Duration::from_millis(100));
            y += 5.0; // 50 px/sec
            push_sample(y);
        }
        let v = velocity_ema();
        assert!(v > REST_VELOCITY_THRESHOLD && v < READING_PACE_THRESHOLD,
                "velocity {} should be in reading range", v);
        assert!(is_reading_paced(), "should be reading-paced after 9s of slow scroll");
    }
}
