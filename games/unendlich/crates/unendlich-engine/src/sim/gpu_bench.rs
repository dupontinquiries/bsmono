//! GPU benchmark scaffold + GPU-ready CPU reference kernels.
//!
//! The goal: run a computation on CPU *and* (once you write the shader) on GPU
//! over scaling workloads, and emit metrics in the exact `run_summaries.txt`
//! format the telemetry dashboard parses — so a CPU run and a GPU run become two
//! comparable runs in the analytics compare view.
//!
//! How "GPU equivalent" works here, given the engine can't compile GLSL in this
//! crate: each kernel ships as a **CPU reference** written in GPU-ready form
//! (Structure-of-Arrays inputs, one logical thread per output element, no
//! host-side branching). That reference is two things at once — the ground truth
//! the GPU output must match bit-for-bit (the correctness gate), and the thing
//! the benchmark times. You implement the compute shader against the documented
//! dispatch contract; cross-validate it against the reference; then benchmark
//! both with [`bench`] and compare via the metric lines.
//!
//! ## Stats & telemetry
//! [`summarize`] produces percentile stats (type-7 interpolation) and
//! [`BenchResult::metric_line`] formats the dashboard metric line, e.g.
//! `vision_batch n=4096 mean=0.123ms±0.045 p50=… over_1ms=3 (0.07%)`.

use crate::sim::fixed::{Fixed, Vec3};
use crate::sim::physics::WorldQuery;
use crate::sim::vision::can_see;

/// Percentile/summary stats over a sample set (all times in ms).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Stats {
    pub n: usize,
    pub mean: f64,
    pub stddev: f64,
    pub min: f64,
    pub max: f64,
    pub p50: f64,
    pub p90: f64,
    pub p99: f64,
}

/// Type-7 (linear-interpolation) percentile over an ascending-sorted slice.
fn percentile(sorted: &[f64], q: f64) -> f64 {
    match sorted.len() {
        0 => 0.0,
        1 => sorted[0],
        n => {
            let rank = q / 100.0 * (n - 1) as f64;
            let lo = rank.floor() as usize;
            let hi = rank.ceil() as usize;
            let frac = rank - lo as f64;
            sorted[lo] + (sorted[hi] - sorted[lo]) * frac
        }
    }
}

/// Summarize raw millisecond samples into [`Stats`].
pub fn summarize(samples: &[f64]) -> Stats {
    let n = samples.len();
    if n == 0 {
        return Stats::default();
    }
    let mean = samples.iter().sum::<f64>() / n as f64;
    let var = samples.iter().map(|x| (x - mean) * (x - mean)).sum::<f64>() / n as f64;
    let mut sorted = samples.to_vec();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    Stats {
        n,
        mean,
        stddev: var.sqrt(),
        min: sorted[0],
        max: sorted[n - 1],
        p50: percentile(&sorted, 50.0),
        p90: percentile(&sorted, 90.0),
        p99: percentile(&sorted, 99.0),
    }
}

/// A benchmark result: the stats plus the raw per-iteration timings.
#[derive(Clone, Debug)]
pub struct BenchResult {
    pub stats: Stats,
    pub samples: Vec<f64>,
}

impl BenchResult {
    /// Count and percentage of iterations exceeding `thr_ms`.
    pub fn over(&self, thr_ms: f64) -> (usize, f64) {
        let c = self.samples.iter().filter(|&&x| x > thr_ms).count();
        let pct = if self.samples.is_empty() { 0.0 } else { 100.0 * c as f64 / self.samples.len() as f64 };
        (c, pct)
    }

    /// Format a `run_summaries.txt`-style metric line (parseable by the
    /// dashboard). `name` must be `[a-z_]+`. With `over_ms`, appends the
    /// over-budget tally.
    pub fn metric_line(&self, name: &str, over_ms: Option<f64>) -> String {
        let s = &self.stats;
        let mut line = format!(
            "{name} n={} mean={:.3}ms\u{00b1}{:.3} p50={:.3} p90={:.3} p99={:.3} min={:.3} max={:.3}",
            s.n, s.mean, s.stddev, s.p50, s.p90, s.p99, s.min, s.max
        );
        if let Some(thr) = over_ms {
            let (c, pct) = self.over(thr);
            line.push_str(&format!(" over_{}ms={} ({:.2}%)", thr, c, pct));
        }
        line
    }
}

/// Time `iters` runs of `f`, returning timings + summary. Runs on real hardware;
/// in a perf-meaningless sandbox the timings are noise but the stats are exact.
pub fn bench<F: FnMut()>(iters: usize, mut f: F) -> BenchResult {
    let mut samples = Vec::with_capacity(iters);
    for _ in 0..iters {
        let t = std::time::Instant::now();
        f();
        samples.push(t.elapsed().as_secs_f64() * 1000.0);
    }
    let stats = summarize(&samples);
    BenchResult { stats, samples }
}

// ── Kernel 1: batched visibility ─────────────────────────────────────────────
//
// Mirrors `sim::vision::can_see`. One logical thread per (observer, target)
// pair; output is row-major `vis[i * targets.len() + j]`.
//
// GPU DISPATCH CONTRACT
//   bindings:
//     0: observers SSBO  — [{ eye: i64×3, facing_x: i64, facing_z: i64 }; N]
//     1: targets   SSBO  — [{ pos: i64×3 }; M]
//     2: world           — occupancy as a 3D texture / SSBO sampled by `physics_at`
//     3: out             — [u8; N*M] (or a packed bitset)
//   uniforms: range (i64 fixed), fov_cos2 (i64 fixed)
//   dispatch: one invocation per (i in 0..N, j in 0..M); thread writes out[i*M+j].
//   each thread reimplements can_see's fixed-point math (per-axis range
//   early-out → DDA line-of-sight → FOV cos² test). Must be INTEGER fixed-point
//   so GPU output equals `visibility_cpu` bit-for-bit — that equality is the
//   correctness gate before trusting the GPU timings.

/// A visibility observer: eye position and (unit) facing in the XZ plane.
#[derive(Clone, Copy, Debug)]
pub struct Probe {
    pub eye: Vec3,
    pub facing_x: Fixed,
    pub facing_z: Fixed,
}

/// CPU reference for the batched-visibility kernel. `vis[i*M+j]` is whether
/// observer `i` can see target `j`. Delegates to the validated `can_see`, so
/// it's correct by construction and serves as the GPU's ground truth.
pub fn visibility_cpu<W: WorldQuery>(
    observers: &[Probe],
    targets: &[Vec3],
    range: Fixed,
    fov_cos2: Fixed,
    world: &W,
) -> Vec<bool> {
    let mut out = Vec::with_capacity(observers.len() * targets.len());
    for o in observers {
        for &t in targets {
            out.push(can_see(o.eye, o.facing_x, o.facing_z, t, range, fov_cos2, world));
        }
    }
    out
}

/// Benchmark the batched-visibility kernel over `iters` runs.
pub fn bench_visibility<W: WorldQuery>(
    observers: &[Probe],
    targets: &[Vec3],
    range: Fixed,
    fov_cos2: Fixed,
    world: &W,
    iters: usize,
) -> BenchResult {
    bench(iters, || {
        let v = visibility_cpu(observers, targets, range, fov_cos2, world);
        std::hint::black_box(&v);
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sim::physics::FlatWorld;
    use crate::sim::vision::{FOV_COS2_90DEG, SIGHT_RANGE};

    fn v(x: i64, y: i64, z: i64) -> Vec3 {
        Vec3 { x: Fixed::from_int(x), y: Fixed::from_int(y), z: Fixed::from_int(z) }
    }

    #[test]
    fn stats_match_reference() {
        let s: Vec<f64> = (1..=10).map(|i| i as f64).collect();
        let st = summarize(&s);
        assert_eq!(st.n, 10);
        assert!((st.mean - 5.5).abs() < 1e-9);
        assert!((st.p50 - 5.5).abs() < 1e-9);
        assert!((st.p90 - 9.1).abs() < 1e-9);
        assert!((st.p99 - 9.91).abs() < 1e-9);
        assert_eq!(st.min, 1.0);
        assert_eq!(st.max, 10.0);
    }

    #[test]
    fn metric_line_parses_shape() {
        let samples: Vec<f64> = (1..=10).map(|i| i as f64).collect();
        let br = BenchResult { stats: summarize(&samples), samples };
        let line = br.metric_line("vision_batch", Some(8.0));
        assert!(line.contains("vision_batch n=10"), "{line}");
        assert!(line.contains("p50=5.500"), "{line}");
        assert!(line.contains("over_8ms=2 (20.00%)"), "{line}");
    }

    #[test]
    fn visibility_kernel_matches_can_see() {
        let world = FlatWorld::new();
        let obs = vec![Probe { eye: v(0, 2, 0), facing_x: Fixed::ONE, facing_z: Fixed::ZERO }];
        let targets = vec![v(5, 2, 0), v(-5, 2, 0), v(100, 2, 0)];
        let out = visibility_cpu(&obs, &targets, SIGHT_RANGE, FOV_COS2_90DEG, &world);
        assert_eq!(out, vec![true, false, false], "ahead+near visible; behind and out-of-range not");
    }
}
