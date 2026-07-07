//! Online running statistics for a single observation series.
//!
//! Originally built for the GPU timer (per-pass timing across
//! thousands of frames). Extracted in v0.23.0 so the new CPU timer
//! can reuse the same Welford-stable accumulator without
//! duplication. Extended in v0.24.2c.2 to retain samples so we can
//! compute percentiles (p50/p90/p99) and render textual box plots
//! at end-of-run.
//!
//! The stats are deliberately microsecond-typed (`u64`) because
//! that's what both the GPU and CPU timers produce after their
//! tick→time conversions; a generic numeric stats type could be
//! built later if needed but YAGNI for now.
//!
//! ## Memory footprint of sample retention
//!
//! For a 1-hour run at 120 fps with 8 zones, the cap on retained
//! samples is 8 × 432,000 × 4 bytes = 13.7 MB. We accept this cost
//! because end-of-run percentile data is otherwise unavailable and
//! the mean/stddev alone can mislead when the distribution is
//! bimodal (a known pattern in our renderer: most frames at 8ms,
//! some streaming frames at 200ms).

/// Online running stats (Welford's algorithm) for one observation
/// series. Also retains the raw samples so we can compute percentiles
/// at end-of-run.
///
/// Numerically stable for variance accumulation across thousands of
/// samples — naive sum-of-squares would lose precision past ~10⁶
/// samples. We track these per-zone over the entire session
/// lifetime, separate from any 1-second log window.
#[derive(Clone, Debug, Default)]
pub struct ZoneStats {
    /// Number of samples accumulated.
    pub n: u64,
    /// Running mean (microseconds).
    pub mean_us: f64,
    /// Sum of squared deviations from mean ("M2" in Welford).
    /// Variance = m2 / (n - 1).
    pub m2: f64,
    pub min_us: u64,
    pub max_us: u64,
    /// Retained samples for percentile computation. Stored as u32
    /// (microseconds, capped at ~71 minutes per sample) to halve
    /// memory vs u64. At u32, a 1-hour run at 120fps × 8 zones is
    /// ~13.7 MB total.
    samples: Vec<u32>,
}

impl ZoneStats {
    /// Add an observation in microseconds.
    pub fn push(&mut self, x_us: u64) {
        self.n += 1;
        let xf = x_us as f64;
        let delta = xf - self.mean_us;
        self.mean_us += delta / self.n as f64;
        let delta2 = xf - self.mean_us;
        self.m2 += delta * delta2;
        if self.n == 1 {
            self.min_us = x_us;
            self.max_us = x_us;
        } else {
            if x_us < self.min_us { self.min_us = x_us; }
            if x_us > self.max_us { self.max_us = x_us; }
        }
        // Saturating-cast to u32 — a sample over u32::MAX µs
        // (~71 minutes) is implausibly long for a single frame and
        // would have already produced extreme outliers in the mean.
        let x_clamped = x_us.min(u32::MAX as u64) as u32;
        self.samples.push(x_clamped);
    }

    /// Sample standard deviation (Bessel's correction). Returns 0
    /// for n < 2.
    pub fn stddev_us(&self) -> f64 {
        if self.n < 2 { 0.0 } else { (self.m2 / (self.n - 1) as f64).sqrt() }
    }

    /// Compute the `q` percentile (q in [0.0, 1.0]) in microseconds.
    /// Returns `None` for an empty sample set. Uses **linear
    /// interpolation between the two nearest ranks** — the method
    /// most data tools (numpy, R) use by default and the one that
    /// matches user intuition ("p99 = 99% of samples are at or
    /// below this value").
    ///
    /// Algorithm: with `n` samples sorted ascending, the q-quantile
    /// is at fractional rank `r = q * (n - 1)`. The result is
    /// `sorted[floor(r)] + frac * (sorted[ceil(r)] - sorted[floor(r)])`.
    ///
    /// Edge cases:
    /// - q == 0.0 → returns the minimum
    /// - q == 1.0 → returns the maximum
    /// - n == 1 → always returns that sample
    pub fn percentile_us(&self, q: f64) -> Option<u64> {
        if self.samples.is_empty() { return None; }
        let q = q.clamp(0.0, 1.0);
        let mut sorted = self.samples.clone();
        sorted.sort_unstable();
        Some(interp_percentile(&sorted, q))
    }

    /// Convenience for the common percentiles. Returns
    /// (p1, p25, p50, p75, p90, p99) in microseconds, or `None` for
    /// empty stats. Single sort, then index lookups — much cheaper
    /// than calling `percentile_us` six times.
    pub fn percentile_panel_us(&self) -> Option<(u64, u64, u64, u64, u64, u64)> {
        if self.samples.is_empty() { return None; }
        let mut sorted = self.samples.clone();
        sorted.sort_unstable();
        Some((
            interp_percentile(&sorted, 0.01),
            interp_percentile(&sorted, 0.25),
            interp_percentile(&sorted, 0.50),
            interp_percentile(&sorted, 0.75),
            interp_percentile(&sorted, 0.90),
            interp_percentile(&sorted, 0.99),
        ))
    }

    /// Count samples above a microsecond threshold. Useful for
    /// "fraction of frames over budget" — e.g. how many frames
    /// exceeded 8333µs (120 fps target).
    pub fn count_over_us(&self, threshold_us: u64) -> u64 {
        let t = threshold_us.min(u32::MAX as u64) as u32;
        self.samples.iter().filter(|&&s| s > t).count() as u64
    }
}

/// Internal helper: linear-interpolation percentile on a sorted
/// `&[u32]`. Caller must have sorted ascending. Returns a u64 in
/// microseconds.
fn interp_percentile(sorted: &[u32], q: f64) -> u64 {
    let n = sorted.len();
    if n == 0 { return 0; }
    if n == 1 { return sorted[0] as u64; }
    let r = q * (n - 1) as f64;
    let lo = r.floor() as usize;
    let hi = r.ceil() as usize;
    let frac = r - lo as f64;
    let v_lo = sorted[lo] as f64;
    let v_hi = sorted[hi] as f64;
    (v_lo + frac * (v_hi - v_lo)).round() as u64
}

/// Render a one-line rich summary for a single zone. Emits mean,
/// stddev, percentiles (p50/p90/p99), min, max, and (if a target
/// budget µs is supplied) the count and fraction of samples over
/// that budget. Suitable for end-of-run summaries and the
/// per-run blocks written to `run_summaries.txt`.
///
/// Example output:
/// ```text
///   shadow    n= 1138  mean= 64.85ms±56.44  p50=  3.13  p90= 38.40  p99= 85.40  min= 0.00  max=265.05  over_8.3ms=421 (37.0%)
/// ```
///
/// `label_width` lets the caller right-pad the zone label so a
/// block of these lines stays column-aligned across zones with
/// different name lengths.
///
/// `budget_us` is `Some(microseconds)` to print over-budget stats,
/// `None` to omit them — appropriate for zones (like per-LOD
/// mesher costs) where there's no single "frame budget."
pub fn render_zone_rich(
    label: &str,
    label_width: usize,
    stats: &ZoneStats,
    budget_us: Option<u64>,
) -> String {
    let n = stats.n;
    if n == 0 {
        return format!("  {:<width$}  n=     0  (no samples)",
            label, width = label_width);
    }
    let mean_ms = stats.mean_us / 1000.0;
    let stddev_ms = stats.stddev_us() / 1000.0;
    let min_ms = stats.min_us as f64 / 1000.0;
    let max_ms = stats.max_us as f64 / 1000.0;

    let (p50_ms, p90_ms, p99_ms) = match stats.percentile_panel_us() {
        Some((_, _, p50, _, p90, p99)) => (
            p50 as f64 / 1000.0,
            p90 as f64 / 1000.0,
            p99 as f64 / 1000.0,
        ),
        None => (0.0, 0.0, 0.0),
    };

    let over = match budget_us {
        Some(b) => {
            let over_n = stats.count_over_us(b);
            let pct = if n > 0 { 100.0 * over_n as f64 / n as f64 } else { 0.0 };
            format!("  over_{:.1}ms={} ({:.1}%)",
                b as f64 / 1000.0, over_n, pct)
        }
        None => String::new(),
    };

    format!(
        "  {:<width$}  n={:>6}  mean={:7.2}ms±{:6.2}  p50={:6.2}  p90={:6.2}  p99={:6.2}  min={:6.2}  max={:7.2}{}",
        label,
        n,
        mean_ms, stddev_ms,
        p50_ms, p90_ms, p99_ms,
        min_ms, max_ms,
        over,
        width = label_width,
    )
}

/// Render an ASCII box-and-whisker plot for one zone. The plot is
/// a fixed-width visualisation where:
/// - The full bar spans `[min_us, max_us]`.
/// - Whiskers ("|---") span [min, p25] and [p75, max].
/// - The box ("[==X==]") covers [p25, p75] with `X` at p50.
///
/// Useful for visually spotting bimodal distributions in a quick
/// scan: a long thin tail to one side means streaming spikes; a
/// fat box means consistently variable cost; a single point
/// collapse means stable steady-state.
///
/// Width is fixed at 60 chars for the plot itself; the label is
/// right-padded to `label_width`.
pub fn render_zone_boxplot(
    label: &str,
    label_width: usize,
    stats: &ZoneStats,
) -> String {
    let n = stats.n;
    if n == 0 {
        return format!("  {:<width$}  (no samples for box plot)",
            label, width = label_width);
    }
    let (_, p25, p50, p75, _, _) = match stats.percentile_panel_us() {
        Some(t) => t,
        None => return format!("  {:<width$}  (no samples)",
            label, width = label_width),
    };

    let min_us = stats.min_us;
    let max_us = stats.max_us;
    let range = max_us.saturating_sub(min_us);

    const WIDTH: usize = 60;
    let pos = |v: u64| -> usize {
        if range == 0 { 0 }
        else {
            let f = (v.saturating_sub(min_us)) as f64 / range as f64;
            ((f * (WIDTH - 1) as f64).round() as usize).min(WIDTH - 1)
        }
    };

    let p25_pos = pos(p25);
    let p50_pos = pos(p50);
    let p75_pos = pos(p75);
    let max_pos = WIDTH - 1;

    // Build the bar layer-by-layer so each later layer can overwrite
    // earlier layers cleanly. Order: filler → whiskers → ends →
    // box body → box edges → median.
    let mut bar = vec![b' '; WIDTH];

    if range == 0 {
        // Totally collapsed distribution (min == max). Show a single
        // marker at position 0 — drawing a "range" would be a lie.
        // Use 'X' (the median) since it's the most informative.
        bar[0] = b'X';
        for i in 1..WIDTH { bar[i] = b'.'; }
        let bar_str = std::str::from_utf8(&bar).unwrap();
        return format!(
            "  {:<width$}  {:>7.2}ms {} {:>7.2}ms  (all samples equal — no spread)",
            label,
            min_us as f64 / 1000.0,
            bar_str,
            max_us as f64 / 1000.0,
            width = label_width,
        );
    }

    // Whiskers as dashes across the full span.
    for i in 0..WIDTH { bar[i] = b'-'; }
    // Whisker endpoints.
    bar[0] = b'|';
    bar[max_pos] = b'|';

    // Box body and edges.
    if p75_pos > p25_pos {
        // Normal case: box has nonzero width.
        for i in p25_pos + 1..p75_pos { bar[i] = b'='; }
        bar[p25_pos] = b'[';
        bar[p75_pos] = b']';
    } else {
        // Collapsed box (p25 == p75 == same position). Use a
        // single '#' to indicate the whole IQR is at one point —
        // visually distinct from the dash whiskers.
        bar[p25_pos] = b'#';
    }

    // Median marker. Always placed, even if it coincides with a
    // box edge or collapses to the same position as p25/p75.
    bar[p50_pos] = b'X';

    // Ensure endpoints stay readable if percentiles landed on them.
    if bar[0] != b'X' { bar[0] = b'|'; }
    if bar[max_pos] != b'X' { bar[max_pos] = b'|'; }

    let bar_str = std::str::from_utf8(&bar).unwrap();

    format!(
        "  {:<width$}  {:>7.2}ms {} {:>7.2}ms  (p25={:.2} p50={:.2} p75={:.2})",
        label,
        min_us as f64 / 1000.0,
        bar_str,
        max_us as f64 / 1000.0,
        p25 as f64 / 1000.0,
        p50 as f64 / 1000.0,
        p75 as f64 / 1000.0,
        width = label_width,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Welford matches naive offline computation on a small sample.
    /// The previous tests for this code lived in gpu_timer.rs and
    /// pinned the same property; we keep the test here so the
    /// extracted module is self-contained.
    #[test]
    fn welford_matches_naive() {
        let samples: [u64; 5] = [10_000, 12_000, 8_000, 14_000, 11_000];
        let mut zs = ZoneStats::default();
        for &x in &samples { zs.push(x); }

        let n = samples.len() as f64;
        let mean = samples.iter().map(|&x| x as f64).sum::<f64>() / n;
        let var = samples.iter()
            .map(|&x| { let d = x as f64 - mean; d * d })
            .sum::<f64>() / (n - 1.0);
        let stddev = var.sqrt();

        assert_eq!(zs.n, 5);
        assert!((zs.mean_us - mean).abs() < 1e-6);
        assert!((zs.stddev_us() - stddev).abs() < 1e-6);
        assert_eq!(zs.min_us, 8_000);
        assert_eq!(zs.max_us, 14_000);
    }

    #[test]
    fn empty_returns_zero_stddev() {
        let zs = ZoneStats::default();
        assert_eq!(zs.n, 0);
        assert_eq!(zs.stddev_us(), 0.0);
    }

    #[test]
    fn single_sample_stddev_is_zero() {
        let mut zs = ZoneStats::default();
        zs.push(1234);
        assert_eq!(zs.n, 1);
        assert_eq!(zs.mean_us, 1234.0);
        assert_eq!(zs.stddev_us(), 0.0);
        assert_eq!(zs.min_us, 1234);
        assert_eq!(zs.max_us, 1234);
    }

    /// Min and max track correctly across mixed-magnitude samples.
    #[test]
    fn min_max_track_extremes() {
        let mut zs = ZoneStats::default();
        for &x in &[5u64, 100, 1, 200, 50] { zs.push(x); }
        assert_eq!(zs.min_us, 1);
        assert_eq!(zs.max_us, 200);
    }

    // ===== Percentile API (v0.24.2c.2) =====

    /// p0 == min, p100 == max — boundary cases.
    #[test]
    fn percentile_boundaries() {
        let mut zs = ZoneStats::default();
        for &x in &[10u64, 20, 30, 40, 50] { zs.push(x); }
        assert_eq!(zs.percentile_us(0.0), Some(10));
        assert_eq!(zs.percentile_us(1.0), Some(50));
    }

    /// p50 of an odd-count sorted set is the middle element.
    #[test]
    fn percentile_p50_on_odd_count() {
        let mut zs = ZoneStats::default();
        for &x in &[10u64, 20, 30, 40, 50] { zs.push(x); }
        // Nearest-rank: ceil(0.5 * 5) - 1 = 2 → sorted[2] = 30.
        assert_eq!(zs.percentile_us(0.5), Some(30));
    }

    /// Percentile API returns None for an empty stat set.
    #[test]
    fn percentile_empty_is_none() {
        let zs = ZoneStats::default();
        assert!(zs.percentile_us(0.5).is_none());
        assert!(zs.percentile_panel_us().is_none());
    }

    /// percentile_panel returns all six values consistent with
    /// individual `percentile_us` calls.
    #[test]
    fn percentile_panel_matches_individual_calls() {
        let mut zs = ZoneStats::default();
        for i in 1u64..=100 { zs.push(i); }
        let (p1, p25, p50, p75, p90, p99) = zs.percentile_panel_us().unwrap();
        assert_eq!(Some(p1),  zs.percentile_us(0.01));
        assert_eq!(Some(p25), zs.percentile_us(0.25));
        assert_eq!(Some(p50), zs.percentile_us(0.50));
        assert_eq!(Some(p75), zs.percentile_us(0.75));
        assert_eq!(Some(p90), zs.percentile_us(0.90));
        assert_eq!(Some(p99), zs.percentile_us(0.99));
    }

    /// `count_over_us` correctly counts samples strictly above the
    /// threshold (matches "frames over budget" framing).
    #[test]
    fn count_over_threshold_is_strict() {
        let mut zs = ZoneStats::default();
        for &x in &[100u64, 200, 300, 400, 500] { zs.push(x); }
        // Strictly greater than 200: 300, 400, 500 → 3.
        assert_eq!(zs.count_over_us(200), 3);
        // No samples over 500.
        assert_eq!(zs.count_over_us(500), 0);
        assert_eq!(zs.count_over_us(0), 5);
    }

    /// A pile of identical samples produces identical percentiles —
    /// box plot would be a single point.
    #[test]
    fn identical_samples_collapse_percentiles() {
        let mut zs = ZoneStats::default();
        for _ in 0..100 { zs.push(8333); }
        let (p1, p25, p50, p75, p90, p99) = zs.percentile_panel_us().unwrap();
        assert_eq!(p1, 8333);
        assert_eq!(p25, 8333);
        assert_eq!(p50, 8333);
        assert_eq!(p75, 8333);
        assert_eq!(p90, 8333);
        assert_eq!(p99, 8333);
    }

    /// Bimodal distribution (a known pattern in the renderer: most
    /// frames cheap, some streaming frames expensive) reveals itself
    /// in p99 ≫ p50 — exactly the signal mean/stddev hide.
    ///
    /// With 5% of frames expensive, the upper 5% of the sorted
    /// samples is all-expensive — so p95 and p99 both land in the
    /// expensive mode while p50 stays in the cheap mode.
    #[test]
    fn bimodal_distribution_shows_in_p99() {
        let mut zs = ZoneStats::default();
        // 950 cheap frames at 8ms, 50 expensive at 200ms (5%
        // streaming-spike rate).
        for _ in 0..950 { zs.push(8_000); }
        for _ in 0..50  { zs.push(200_000); }
        let (_, _, p50, _, p90, p99) = zs.percentile_panel_us().unwrap();
        assert_eq!(p50, 8_000, "median should be the cheap mode");
        assert_eq!(p90, 8_000, "90% of frames are cheap");
        assert_eq!(p99, 200_000, "p99 catches the streaming spikes");
    }

    // ===== Rich rendering (box plot, one-line summary) =====

    /// One-line summary contains the basic fields users want to scan:
    /// n, mean, p50/p90/p99, min, max, and optional over-budget %.
    #[test]
    fn render_zone_rich_contains_expected_fields() {
        let mut zs = ZoneStats::default();
        for i in 0..100 { zs.push(1000 + i * 10); }  // 1.0ms..2.0ms
        let line = render_zone_rich("test_zone", 12, &zs, Some(1500));
        assert!(line.contains("test_zone"));
        assert!(line.contains("n=   100"));
        assert!(line.contains("mean="));
        assert!(line.contains("p50="));
        assert!(line.contains("p90="));
        assert!(line.contains("p99="));
        assert!(line.contains("min="));
        assert!(line.contains("max="));
        assert!(line.contains("over_1.5ms="));
    }

    /// With no budget, the over-budget suffix is omitted.
    #[test]
    fn render_zone_rich_no_budget_omits_over_field() {
        let mut zs = ZoneStats::default();
        for i in 0..10 { zs.push(1000 + i * 10); }
        let line = render_zone_rich("test", 6, &zs, None);
        assert!(!line.contains("over_"));
    }

    /// Empty stats render a clear "no samples" line rather than nan/inf.
    #[test]
    fn render_zone_rich_empty_says_no_samples() {
        let zs = ZoneStats::default();
        let line = render_zone_rich("empty", 6, &zs, Some(1000));
        assert!(line.contains("no samples"));
        assert!(!line.contains("nan"));
        assert!(!line.contains("inf"));
    }

    /// Box plot includes label, min/max in ms, and bar with whiskers.
    #[test]
    fn render_zone_boxplot_basics() {
        let mut zs = ZoneStats::default();
        for i in 0..100 { zs.push(1000 + i * 10); }
        let plot = render_zone_boxplot("test", 6, &zs);
        assert!(plot.contains("test"));
        assert!(plot.contains("1.00ms")); // min
        assert!(plot.contains("|"));      // whisker ends
        assert!(plot.contains("["));      // box start
        assert!(plot.contains("]"));      // box end
        assert!(plot.contains("X"));      // median
        assert!(plot.contains("p25=") && plot.contains("p50=") && plot.contains("p75="));
    }

    /// Box plot for identical samples doesn't divide by zero.
    #[test]
    fn render_zone_boxplot_identical_samples_handled() {
        let mut zs = ZoneStats::default();
        for _ in 0..50 { zs.push(8333); }
        let plot = render_zone_boxplot("flat", 6, &zs);
        // Doesn't panic, doesn't contain NaN.
        assert!(!plot.contains("nan"));
        assert!(!plot.contains("inf"));
    }

    /// Box plot on a bimodal distribution puts the median in the
    /// cheap mode (left side of bar).
    #[test]
    fn render_zone_boxplot_bimodal_shows_left_skew() {
        let mut zs = ZoneStats::default();
        for _ in 0..900 { zs.push(8_000); }
        for _ in 0..100 { zs.push(200_000); }
        let plot = render_zone_boxplot("bimodal", 8, &zs);
        // The 'X' (median) should be early in the bar — at position
        // corresponding to ~(8000-8000)/(200000-8000) = 0 of the
        // 0..59 range. We don't pin the exact column, just verify
        // 'X' appears in the early third.
        let bar_start = plot.find("ms ").map(|p| p + 3).unwrap();
        let bar_end = plot.rfind("ms").unwrap();
        let bar = &plot[bar_start..bar_end];
        let x_idx = bar.find('X').expect("box plot missing median X");
        let bar_len = bar.len();
        assert!(x_idx < bar_len / 3,
            "median X should be in first third of bimodal box plot \
             (left-skewed); got at index {} of {}: '{}'",
            x_idx, bar_len, bar);
    }
}
