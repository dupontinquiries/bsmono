//! `sim::submit_schedule` — submission pacing from recent frame timing.
//!
//! Real M2 data: the CPU stalls on the GPU — `wait_fences` p99 44–68ms, max
//! 138ms — while the GPU tail spikes. When the CPU races ahead of a slow GPU it
//! should keep more frames in flight (and may skip a submit) so it doesn't block;
//! when the GPU idles waiting on the CPU it should reduce inflight depth to cut
//! latency, and coalesce small command buffers since submits are then cheap.
//!
//! This is **distinct from `frame_pacer`**, which tunes *quality* knobs; this
//! decides *submission timing*. Pure, deterministic, bounded, panic-free
//! (R1/R6/R11); standalone `rustc --test`-able, release-safe.

/// One recent frame's timing (milliseconds).
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct FrameStats {
    pub gpu_ms: f32,
    pub cpu_ms: f32,
    /// CPU time spent blocked waiting on GPU fences.
    pub wait_ms: f32,
}

/// A submission-pacing decision.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Plan {
    /// Submit this frame? `false` requests a submit-skip to let the GPU drain.
    pub submit: bool,
    /// Desired frames-in-flight depth, clamped to `[1, MAX_INFLIGHT_CAP]`.
    pub max_inflight: u8,
    /// How many small command buffers to merge before submit (0 = none).
    pub coalesce: u8,
}

/// Hard cap on frames in flight.
pub const MAX_INFLIGHT_CAP: u8 = 3;
/// Command buffers to coalesce when submits are cheap (CPU-bound).
const COALESCE_WHEN_CHEAP: u8 = 2;

fn clamp_inflight(v: u8) -> u8 {
    if v < 1 {
        1
    } else if v > MAX_INFLIGHT_CAP {
        MAX_INFLIGHT_CAP
    } else {
        v
    }
}

/// Decide submission pacing from `recent` frames, the current inflight depth, and
/// the per-frame `target_ms`.
///
/// - **GPU-bound** (`gpu_ms > cpu_ms` and high `wait_ms`): raise `max_inflight`
///   toward the cap; if already capped and badly behind, request a submit-skip.
/// - **CPU-bound** (`cpu_ms > gpu_ms` with the GPU idle-low): lower `max_inflight`
///   (≥ 1) and coalesce small buffers (submits are cheap).
/// - **Steady / under target**: hold the current depth, submit normally.
///
/// Empty `recent` ⇒ safe defaults: `submit = true`, inflight = current (clamped),
/// no coalescing. `max_inflight` is always in `[1, MAX_INFLIGHT_CAP]`.
pub fn plan(recent: &[FrameStats], cur_inflight: u8, target_ms: f32) -> Plan {
    let cur = clamp_inflight(cur_inflight);
    if recent.is_empty() {
        return Plan { submit: true, max_inflight: cur, coalesce: 0 };
    }

    // means over the window (`.max(0.0)` also sanitises NaN to 0.0)
    let n = recent.len() as f32;
    let mut g = 0.0f32;
    let mut c = 0.0f32;
    let mut w = 0.0f32;
    for f in recent {
        g += f.gpu_ms.max(0.0);
        c += f.cpu_ms.max(0.0);
        w += f.wait_ms.max(0.0);
    }
    g /= n;
    c /= n;
    w /= n;

    let t = target_ms.max(0.001);
    let gpu_bound = g > c && w > t * 0.5; // CPU blocking on a slower GPU
    let cpu_bound = c > g && g < t; // GPU idle-low, CPU the bottleneck

    if gpu_bound {
        let max_inflight = clamp_inflight(cur.saturating_add(1));
        // badly behind AND already buffered to the cap ⇒ skip a submit to drain
        let submit = !(w > t && cur >= MAX_INFLIGHT_CAP);
        Plan { submit, max_inflight, coalesce: 0 }
    } else if cpu_bound {
        let max_inflight = clamp_inflight(cur.saturating_sub(1));
        Plan { submit: true, max_inflight, coalesce: COALESCE_WHEN_CHEAP }
    } else {
        // steady / under budget: hold
        Plan { submit: true, max_inflight: cur, coalesce: 0 }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fs(gpu: f32, cpu: f32, wait: f32) -> FrameStats {
        FrameStats { gpu_ms: gpu, cpu_ms: cpu, wait_ms: wait }
    }

    #[test]
    fn gpu_bound_raises_inflight_or_skips() {
        let recent = [fs(30.0, 10.0, 40.0); 4];
        // not yet at cap → inflight rises, still submits
        let p = plan(&recent, 1, 8.0);
        assert!(p.max_inflight > 1 && p.max_inflight <= MAX_INFLIGHT_CAP, "inflight should rise");
        // at cap and badly behind → submit-skip permitted
        let p2 = plan(&recent, MAX_INFLIGHT_CAP, 8.0);
        assert_eq!(p2.max_inflight, MAX_INFLIGHT_CAP);
        assert!(!p2.submit, "at cap + far behind ⇒ skip a submit");
    }

    #[test]
    fn cpu_bound_drops_inflight() {
        let recent = [fs(2.0, 20.0, 1.0); 4]; // GPU idle, CPU heavy
        let p = plan(&recent, 3, 8.0);
        assert!(p.max_inflight < 3 && p.max_inflight >= 1, "inflight should drop");
        assert!(p.submit);
        assert!(p.coalesce > 0, "cheap submits ⇒ coalesce");
    }

    #[test]
    fn steady_is_stable() {
        let recent = [fs(4.0, 4.0, 1.0); 4]; // balanced, under target
        let p = plan(&recent, 2, 8.0);
        assert_eq!(p, Plan { submit: true, max_inflight: 2, coalesce: 0 });
    }

    #[test]
    fn empty_recent_safe_defaults() {
        let p = plan(&[], 2, 8.0);
        assert_eq!(p, Plan { submit: true, max_inflight: 2, coalesce: 0 });
        // current depth clamped into range
        assert_eq!(plan(&[], 0, 8.0).max_inflight, 1);
        assert_eq!(plan(&[], 99, 8.0).max_inflight, MAX_INFLIGHT_CAP);
    }

    #[test]
    fn inflight_always_in_bounds() {
        let mut s = 0x39FA_u64; // documented seed (R20)
        let mut rng = || {
            s = s.wrapping_mul(6364136223846793005).wrapping_add(1);
            (s >> 40) as f32 / ((1u64 << 24) as f32)
        };
        for _ in 0..3000 {
            let recent: Vec<FrameStats> = (0..(1 + (rng() * 8.0) as usize))
                .map(|_| fs(rng() * 100.0 - 10.0, rng() * 100.0 - 10.0, rng() * 150.0 - 10.0))
                .collect();
            let p = plan(&recent, (rng() * 10.0) as u8, rng() * 20.0);
            assert!((1..=MAX_INFLIGHT_CAP).contains(&p.max_inflight), "inflight oob: {}", p.max_inflight);
            assert!(p.coalesce <= COALESCE_WHEN_CHEAP, "coalesce oob");
        }
        // NaN stats don't break it
        let nan = [fs(f32::NAN, f32::NAN, f32::NAN)];
        let p = plan(&nan, 2, f32::NAN);
        assert!((1..=MAX_INFLIGHT_CAP).contains(&p.max_inflight));
    }

    #[test]
    fn deterministic() {
        let recent = [fs(30.0, 10.0, 40.0), fs(28.0, 9.0, 38.0)];
        assert_eq!(plan(&recent, 2, 8.0), plan(&recent, 2, 8.0));
    }
}
