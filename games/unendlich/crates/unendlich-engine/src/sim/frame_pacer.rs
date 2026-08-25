//! `sim::frame_pacer` — adaptive frame pacer / graceful degradation (round R37).
//!
//! On M2 (render distance 3) the median frame is fast (GPU p50 ≈ 3.3 ms) but the
//! tail is bad — ~10% of frames blow the 8.3 ms budget (p99 ≈ 77.8 ms, max
//! ≈ 153 ms). That's a frame-time *consistency* problem, not throughput. This is
//! the closed loop that drives the existing (previously un-driven) quality knobs
//! — `SHADOW_CHUNK_STRIDE`, `LOD_FIDELITY`, `ADAPTIVE_UPLOADS`,
//! `froxel steps` — trading a little quality on hot frames to kill stutter, so
//! the over-budget frames become a momentary dip instead of a visible hitch.
//!
//! [`adjust`] looks at a high percentile (≈p90, nearest-rank) of the recent
//! frame times: if it exceeds `target_ms` it steps quality DOWN one notch; if it
//! is comfortably under (below a hysteresis band) it steps one notch back UP;
//! inside the band it leaves the knobs untouched so it doesn't oscillate. Every
//! result is clamped to `lim`.
//!
//! Pure, deterministic, no wall-clock, no RNG, no I/O, no `.unwrap()`/`.expect()`.
//! Release-safe: all helpers are unconditional (no `#[cfg]`-gated items), so it
//! compiles clean under `cargo check --release`.

/// Ring of recent total frame times in milliseconds; `len` valid entries.
///
/// 32 samples (was 16) gives the p90 estimator a wider window — at 60 FPS
/// that's ~530ms of history instead of ~267ms. Reduces noise-driven
/// oscillation without meaningfully delaying reaction (the multi-step
/// degradation below handles urgency).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FrameHistory {
    pub recent_ms: [f32; 32],
    pub len: usize,
}

/// The quality knobs the pacer drives. Higher quality (and cost) =
/// lower `shadow_stride`, higher `lod_fidelity` / `max_uploads` / `froxel_steps`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Knobs {
    pub shadow_stride: u8,
    pub lod_fidelity: f32,
    pub max_uploads: u8,
    pub froxel_steps: u8,
}

/// Inclusive `(low, high)` bounds for each knob (raw value bounds, not quality).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Limits {
    pub stride: (u8, u8),
    pub fidelity: (f32, f32),
    pub uploads: (u8, u8),
    pub froxel: (u8, u8),
}

/// Recover only when the percentile is below `(1 - HYST_FRAC) * target` — the
/// hysteresis band that stops oscillation around the target.
const HYST_FRAC: f32 = 0.2;
/// One notch of LOD fidelity.
const LOD_STEP: f32 = 0.1;

fn sanitize_ms(v: f32) -> f32 {
    if v.is_finite() && v > 0.0 {
        v
    } else {
        0.0
    }
}

/// `≈p90` (nearest-rank) of the first `n` sanitized samples. `n ≥ 1`.
fn high_percentile(recent: &[f32; 32], n: usize) -> f32 {
    let mut buf = [0.0f32; 32];
    let mut i = 0;
    while i < n {
        buf[i] = sanitize_ms(recent[i]);
        i += 1;
    }
    let s = &mut buf[..n];
    s.sort_unstable_by(|a, b| a.total_cmp(b)); // total order ⇒ no unwrap, NaN-safe
    let rank = (0.9 * n as f32).ceil() as usize; // 1-based nearest-rank
    let idx = rank.saturating_sub(1).min(n - 1);
    s[idx]
}

fn step_down(cur: Knobs, lim: Limits) -> Knobs {
    Knobs {
        // lower quality: coarser shadows, less fidelity / uploads / froxel steps
        shadow_stride: cur.shadow_stride.saturating_add(1).min(lim.stride.1),
        lod_fidelity: (cur.lod_fidelity - LOD_STEP).max(lim.fidelity.0),
        max_uploads: cur.max_uploads.saturating_sub(1).max(lim.uploads.0),
        froxel_steps: cur.froxel_steps.saturating_sub(1).max(lim.froxel.0),
    }
}

fn step_up(cur: Knobs, lim: Limits) -> Knobs {
    Knobs {
        // higher quality, one notch toward the high bound
        shadow_stride: cur.shadow_stride.saturating_sub(1).max(lim.stride.0),
        lod_fidelity: (cur.lod_fidelity + LOD_STEP).min(lim.fidelity.1),
        max_uploads: cur.max_uploads.saturating_add(1).min(lim.uploads.1),
        froxel_steps: cur.froxel_steps.saturating_add(1).min(lim.froxel.1),
    }
}

/// How many down-steps the overshoot ratio warrants. Proportional: 1.5x over
/// budget = 2 steps, 2x = 3, etc. Capped at 4 to avoid slamming to minimum
/// quality in a single call from one transient spike batch.
fn urgency_steps(perc: f32, target: f32) -> u32 {
    if target <= 0.0 || perc <= target {
        return 0;
    }
    // ratio 1.0..1.25 → 1 step, 1.25..1.75 → 2, 1.75..2.25 → 3, 2.25+ → 4
    let ratio = perc / target;
    let steps = ((ratio - 1.0) * 4.0 + 1.0) as u32;
    steps.min(4)
}

/// Step the quality knobs toward lower quality if recent frames run hot (with
/// urgency proportional to overshoot), one notch toward higher quality if
/// comfortably under (past the hysteresis band), or leave them unchanged inside
/// the band. Clamped to `lim`.
///
/// Multi-step degradation (new in this revision): when p90 is well above
/// target, applying a single notch per frame lets 5-10 frames of stutter pass
/// before quality settles. Stepping 2-4 notches at once for severe overshoot
/// kills the visible hitch within 1-2 frames, at the cost of a brief quality
/// dip that the additive recovery restores smoothly.
pub fn adjust(hist: &FrameHistory, target_ms: f32, cur: Knobs, lim: Limits) -> Knobs {
    let n = hist.len.min(32);
    if n == 0 || !(target_ms.is_finite() && target_ms > 0.0) {
        return cur; // no data / nonsensical target ⇒ hold
    }
    let perc = high_percentile(&hist.recent_ms, n);
    let recover = target_ms * (1.0 - HYST_FRAC);

    if perc > target_ms {
        let steps = urgency_steps(perc, target_ms);
        let mut k = cur;
        for _ in 0..steps {
            k = step_down(k, lim);
        }
        k
    } else if perc < recover {
        step_up(cur, lim)
    } else {
        cur // inside the hysteresis band — hold
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hist(vals: &[f32]) -> FrameHistory {
        let mut recent = [0.0f32; 32];
        let n = vals.len().min(32);
        recent[..n].copy_from_slice(&vals[..n]);
        FrameHistory { recent_ms: recent, len: n }
    }
    fn limits() -> Limits {
        Limits { stride: (1, 4), fidelity: (0.4, 1.0), uploads: (1, 8), froxel: (4, 16) }
    }
    fn mid() -> Knobs {
        Knobs { shadow_stride: 2, lod_fidelity: 0.7, max_uploads: 4, froxel_steps: 10 }
    }

    #[test]
    fn sustained_over_steps_each_knob_toward_low_quality() {
        let h = hist(&[20.0; 16]); // all way over the 8.3ms budget
        // 20/8.3 ≈ 2.4x over → urgency_steps = 4. From mid() each knob
        // takes 4 steps toward the low-quality bound.
        let k = adjust(&h, 8.3, mid(), limits());
        // shadow_stride: 2 + 4 = 6, clamped to max 4
        assert_eq!(k.shadow_stride, 4, "stride clamped at limit");
        // lod_fidelity: 0.7 - 4*0.1 = 0.3, clamped to min 0.4
        assert!((k.lod_fidelity - 0.4).abs() < 1e-6, "fidelity clamped at min");
        // max_uploads: 4 - 4 = 0, clamped to min 1
        assert_eq!(k.max_uploads, 1, "uploads clamped at min");
        // froxel_steps: 10 - 4 = 6, above min 4
        assert_eq!(k.froxel_steps, 6, "froxel -4");
    }

    #[test]
    fn monotone_toward_low_quality_and_converges() {
        let h = hist(&[50.0; 8]);
        let lim = limits();
        let mut k = mid();
        let mut last = k;
        for _ in 0..10 {
            k = adjust(&h, 8.3, k, lim);
            // each call moves toward low quality (monotone)
            assert!(k.shadow_stride >= last.shadow_stride);
            assert!(k.lod_fidelity <= last.lod_fidelity + 1e-6);
            assert!(k.max_uploads <= last.max_uploads);
            assert!(k.froxel_steps <= last.froxel_steps);
            last = k;
        }
        // eventually clamped at the low-quality bound
        assert_eq!(k.shadow_stride, lim.stride.1);
        assert!((k.lod_fidelity - lim.fidelity.0).abs() < 1e-6);
        assert_eq!(k.max_uploads, lim.uploads.0);
        assert_eq!(k.froxel_steps, lim.froxel.0);
    }

    #[test]
    fn sustained_under_steps_back_toward_high_quality() {
        let h = hist(&[2.0; 16]); // comfortably under 8.3 (< 0.8*target)
        let lim = limits();
        // start at the low-quality bound
        let mut k = Knobs { shadow_stride: 4, lod_fidelity: 0.4, max_uploads: 1, froxel_steps: 4 };
        let first = adjust(&h, 8.3, k, lim);
        assert_eq!(first.shadow_stride, 3);
        assert!((first.lod_fidelity - 0.5).abs() < 1e-6);
        assert_eq!(first.max_uploads, 2);
        assert_eq!(first.froxel_steps, 5);
        for _ in 0..20 {
            k = adjust(&h, 8.3, k, lim);
        }
        assert_eq!(k.shadow_stride, lim.stride.0);
        assert!((k.lod_fidelity - lim.fidelity.1).abs() < 1e-6);
        assert_eq!(k.max_uploads, lim.uploads.1);
        assert_eq!(k.froxel_steps, lim.froxel.1);
    }

    #[test]
    fn inside_band_leaves_knobs_unchanged_no_oscillation() {
        let lim = limits();
        let k = mid();
        // target 10 ⇒ band [8.0, 10.0]. All samples in-band ⇒ p90 in-band.
        let steady = hist(&[9.0; 16]);
        assert_eq!(adjust(&steady, 10.0, k, lim), k);
        // alternating but still in-band ⇒ still unchanged (no flip-flop)
        let alt = hist(&[8.2, 9.8, 8.2, 9.8, 8.2, 9.8, 8.2, 9.8, 8.2, 9.8, 8.2, 9.8, 8.2, 9.8, 8.2, 9.8]);
        let once = adjust(&alt, 10.0, k, lim);
        assert_eq!(once, k, "in-band alternation must not move the knobs");
        assert_eq!(adjust(&alt, 10.0, once, lim), once, "and stays put across calls");
    }

    #[test]
    fn clamps_at_both_limits() {
        let lim = limits();
        // already at low-quality bound + hot ⇒ unchanged
        let low = Knobs { shadow_stride: 4, lod_fidelity: 0.4, max_uploads: 1, froxel_steps: 4 };
        assert_eq!(adjust(&hist(&[99.0; 16]), 8.3, low, lim), low);
        // already at high-quality bound + cold ⇒ unchanged
        let high = Knobs { shadow_stride: 1, lod_fidelity: 1.0, max_uploads: 8, froxel_steps: 16 };
        assert_eq!(adjust(&hist(&[1.0; 16]), 8.3, high, lim), high);
    }

    #[test]
    fn empty_history_unchanged() {
        let k = mid();
        assert_eq!(adjust(&hist(&[]), 8.3, k, limits()), k);
    }

    #[test]
    fn nonsensical_target_holds() {
        let k = mid();
        assert_eq!(adjust(&hist(&[20.0; 4]), f32::NAN, k, limits()), k);
        assert_eq!(adjust(&hist(&[20.0; 4]), 0.0, k, limits()), k);
    }

    #[test]
    fn tail_drives_decision_not_median() {
        // mostly fast, but the p90 tail blows the budget ⇒ step DOWN.
        let mut v = [2.0f32; 16];
        v[14] = 40.0;
        v[15] = 60.0; // worst frames
        let k = adjust(&hist(&v), 8.3, mid(), limits());
        // p90 of 16 samples with 2 outliers at 40/60 is one of those
        // outliers; severity drives multi-step. Just verify degradation.
        assert!(k.shadow_stride > 2, "tail over budget ⇒ degrade");
    }

    #[test]
    fn deterministic_and_finite() {
        let h = hist(&[3.0, 9.0, 4.0, 12.0, 2.0, 8.0]);
        let a = adjust(&h, 8.3, mid(), limits());
        let b = adjust(&h, 8.3, mid(), limits());
        assert_eq!(a, b);
        assert!(a.lod_fidelity.is_finite());
    }
}
