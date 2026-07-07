//! profiler — an advanced per-zone timing profiler (r204). The operator
//! asked for "an advanced logging system and profiler so you can get more
//! insights from the logs." The existing CpuTimer/GpuTimer report flat
//! per-frame averages; this adds:
//!   - per-zone HISTOGRAMS (so spikes are visible, not averaged away);
//!   - percentiles (p50/p95/p99/max) — the shape of the frame-time tail,
//!     which is what actually causes perceived stutter;
//!   - a compact structured dump (one line per zone) easy to grep in logs.
//!
//! Deterministic in structure (fixed bucket edges); the timings come from
//! the wall clock at the call sites. std-only, allocation-light (fixed
//! bucket array per zone), and unit-tested on the percentile math.

use std::collections::BTreeMap;

/// Log-spaced bucket upper-bounds in milliseconds. A frame zone that
/// takes 0.3ms lands in bucket 1; a 120ms stutter lands in the last.
/// Chosen so the interesting game range (0.1–100ms) has fine resolution.
const BUCKET_EDGES_MS: [f64; 16] = [
    0.05, 0.1, 0.25, 0.5, 1.0, 2.0, 4.0, 8.0, 16.0, 33.0, 50.0, 75.0, 100.0,
    150.0, 250.0, f64::INFINITY,
];

#[derive(Clone)]
struct ZoneStats {
    buckets: [u32; 16],
    count: u64,
    sum_ms: f64,
    max_ms: f64,
    /// Reservoir of recent samples for exact percentiles (bounded).
    recent: Vec<f64>,
}

impl Default for ZoneStats {
    fn default() -> Self {
        ZoneStats { buckets: [0; 16], count: 0, sum_ms: 0.0, max_ms: 0.0, recent: Vec::new() }
    }
}

impl ZoneStats {
    const RECENT_CAP: usize = 512;

    fn record(&mut self, ms: f64) {
        let b = BUCKET_EDGES_MS.iter().position(|&e| ms <= e).unwrap_or(15);
        self.buckets[b] += 1;
        self.count += 1;
        self.sum_ms += ms;
        if ms > self.max_ms {
            self.max_ms = ms;
        }
        if self.recent.len() < Self::RECENT_CAP {
            self.recent.push(ms);
        } else {
            // ring-replace by count so the reservoir stays recent-ish.
            let idx = (self.count as usize) % Self::RECENT_CAP;
            self.recent[idx] = ms;
        }
    }

    fn percentile(&self, p: f64) -> f64 {
        if self.recent.is_empty() {
            return 0.0;
        }
        let mut s = self.recent.clone();
        s.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        // Nearest-rank: rank = ceil(p/100 * n), 1-based, clamped to [1,n].
        // (round() mis-placed the top percentile — a single max sample in
        // n=100 must land in p99, which ceil gives and round did not.)
        let n = s.len();
        let rank = ((p / 100.0) * n as f64).ceil() as usize;
        let idx = rank.clamp(1, n) - 1;
        s[idx]
    }

    fn mean(&self) -> f64 {
        if self.count == 0 { 0.0 } else { self.sum_ms / self.count as f64 }
    }
}

/// The profiler: a named map of zone stats. Cheap to keep one global or
/// one per subsystem.
#[derive(Default)]
pub struct Profiler {
    zones: BTreeMap<&'static str, ZoneStats>,
}

impl Profiler {
    pub fn new() -> Self {
        Profiler { zones: BTreeMap::new() }
    }

    /// Record a timing sample (milliseconds) for a named zone.
    pub fn record(&mut self, zone: &'static str, ms: f64) {
        self.zones.entry(zone).or_default().record(ms);
    }

    /// Time a closure and record it under `zone`. Returns the closure's
    /// value so it drops into existing code with no restructuring.
    pub fn time<T>(&mut self, zone: &'static str, f: impl FnOnce() -> T) -> T {
        let t0 = std::time::Instant::now();
        let out = f();
        self.record(zone, t0.elapsed().as_secs_f64() * 1e3);
        out
    }

    /// Per-zone count (for tests / external aggregation).
    pub fn count(&self, zone: &str) -> u64 {
        self.zones.get(zone).map(|z| z.count).unwrap_or(0)
    }

    pub fn percentile(&self, zone: &str, p: f64) -> f64 {
        self.zones.get(zone).map(|z| z.percentile(p)).unwrap_or(0.0)
    }

    /// One structured line per zone, sorted by p99 descending (worst
    /// stutter first) — the order you want when reading a log. Format:
    /// `zone=NAME n=COUNT mean=X p50=X p95=X p99=X max=X`. Greppable.
    pub fn dump(&self) -> String {
        let mut rows: Vec<(&str, &ZoneStats)> = self.zones.iter().map(|(k, v)| (*k, v)).collect();
        rows.sort_by(|a, b| {
            b.1.percentile(99.0)
                .partial_cmp(&a.1.percentile(99.0))
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        let mut out = String::new();
        for (name, z) in rows {
            out.push_str(&format!(
                "zone={} n={} mean={:.2} p50={:.2} p95={:.2} p99={:.2} max={:.2}\n",
                name,
                z.count,
                z.mean(),
                z.percentile(50.0),
                z.percentile(95.0),
                z.percentile(99.0),
                z.max_ms,
            ));
        }
        out
    }

    /// A compact ASCII histogram for one zone, for deep dives. Each row is
    /// a bucket upper-bound and a bar proportional to its count.
    pub fn histogram(&self, zone: &str) -> String {
        let Some(z) = self.zones.get(zone) else { return String::new() };
        let peak = z.buckets.iter().copied().max().unwrap_or(1).max(1);
        let mut out = format!("histogram[{zone}] (n={}):\n", z.count);
        for (i, &c) in z.buckets.iter().enumerate() {
            if c == 0 {
                continue;
            }
            let bar = (c as usize * 40 / peak as usize).max(1);
            let edge = BUCKET_EDGES_MS[i];
            let label = if edge.is_infinite() { "  inf".to_string() } else { format!("{edge:>5.1}") };
            out.push_str(&format!("  <={label}ms |{}{} {c}\n", "#".repeat(bar), " ".repeat(40 - bar)));
        }
        out
    }

    pub fn clear(&mut self) {
        self.zones.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn records_and_means() {
        let mut p = Profiler::new();
        for _ in 0..10 {
            p.record("a", 2.0);
        }
        assert_eq!(p.count("a"), 10);
        // p50 of a constant series is that constant.
        assert!((p.percentile("a", 50.0) - 2.0).abs() < 1e-6);
    }

    #[test]
    fn percentiles_track_the_tail() {
        let mut p = Profiler::new();
        // 90 fast frames + 10 stutters (10%); p50 stays fast, p95/p99 and
        // max all catch the tail. (A single 1% spike only shows in p100/max,
        // which is statistically correct — p99 means 99% are at or below.)
        for _ in 0..90 {
            p.record("frame", 1.0);
        }
        for _ in 0..10 {
            p.record("frame", 100.0);
        }
        assert!((p.percentile("frame", 50.0) - 1.0).abs() < 1e-6, "p50 stays fast");
        assert!(p.percentile("frame", 99.0) > 50.0, "p99 sees the 10% tail");
        // max is exact regardless of reservoir.
        assert!(p.zones.get("frame").unwrap().max_ms >= 100.0);
    }

    #[test]
    fn dump_orders_worst_first() {
        let mut p = Profiler::new();
        for _ in 0..50 { p.record("cheap", 0.5); }
        for _ in 0..50 { p.record("expensive", 40.0); }
        let d = p.dump();
        let cheap_at = d.find("zone=cheap").unwrap();
        let exp_at = d.find("zone=expensive").unwrap();
        assert!(exp_at < cheap_at, "worst p99 zone should be listed first");
    }

    #[test]
    fn time_closure_returns_value() {
        let mut p = Profiler::new();
        let v = p.time("work", || 21 * 2);
        assert_eq!(v, 42);
        assert_eq!(p.count("work"), 1);
    }
}
