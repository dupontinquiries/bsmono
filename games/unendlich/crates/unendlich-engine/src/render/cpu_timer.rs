//! CPU-side per-zone timer for the render thread (v0.23.0).
//!
//! # Why this exists
//!
//! The v0.21+ GPU timer measures per-pass GPU work but doesn't see
//! anything happening on the CPU. The v0.22-era playtest revealed
//! that with LOD active, GPU total drops to ~2.4ms mean while
//! `cpu: draw` stays at ~1000ms per second of wall clock. The
//! gap is CPU work that was previously overshadowed by GPU work
//! and is now exposed.
//!
//! Likely culprits inside the renderer's draw frame:
//!
//! * `wait_for_fences` — block on previous frame's GPU work.
//! * `acquire_next_image` — block on swapchain availability
//!   (vsync stall lives here).
//! * Command-buffer recording — uploads + cull dispatch + render
//!   pass setup. CPU-only work, no GPU overlap.
//! * `queue_submit` — usually fast, can stall under driver pressure.
//! * `queue_present` — present the rendered image.
//!
//! Without per-zone timing we can't tell which of these is eating
//! the wall clock. This module instruments each one with the same
//! `ZoneStats` accumulator the GPU timer uses, so we get mean,
//! stddev, min, max per zone over the entire session.
//!
//! # Reusability
//!
//! Like `gpu_timer`, this module is engine-agnostic except for the
//! specific `Zone` enum it uses. Future extraction would make
//! `Zone` a generic parameter; not done today because there's only
//! one consumer.
//!
//! # Cost
//!
//! Each `mark` call is one `Instant::now()` plus a HashMap entry
//! update. On x86_64 / aarch64 `Instant::now()` is a syscall-free
//! `clock_gettime(CLOCK_MONOTONIC)` ~20ns. With 5 marks per frame
//! at 120 FPS this is 12 μs of overhead per second — negligible.

use std::collections::BTreeMap;
use std::time::Instant;

use unendlich_shared::zone_stats::ZoneStats;

/// Zones inside the render thread's draw frame. Each zone is the
/// span of wall-clock time spent in one logical step. Add a zone
/// here AND a corresponding `mark` call in the renderer for new
/// instrumentation.
///
/// Discriminants are dense so we can index `[ZoneStats; COUNT]`
/// directly — though we use BTreeMap today for readability.
#[repr(u8)]
#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum CpuZone {
    /// `vkWaitForFences` — block until previous frame's GPU work
    /// is complete. The classic "GPU not keeping up" stall.
    WaitFences   = 0,
    /// `vkAcquireNextImageKHR` — block until a swapchain image is
    /// ready. With FIFO present mode (vsync), this is where vblank
    /// waits land.
    AcquireImage = 1,
    /// CPU work building the command buffer: meshes uploaded,
    /// cull dispatch recorded, render passes recorded. Includes
    /// shadow + main pass recording.
    RecordCmd    = 2,
    /// `vkQueueSubmit` — submit the command buffer. Usually fast.
    QueueSubmit  = 3,
    /// `vkQueuePresentKHR` — present the rendered image. With
    /// vsync, can block waiting for vblank.
    QueuePresent = 4,
}

impl CpuZone {
    pub const COUNT: usize = 5;

    /// Human-readable label for log output.
    pub fn name(self) -> &'static str {
        match self {
            CpuZone::WaitFences   => "wait_fences",
            CpuZone::AcquireImage => "acquire",
            CpuZone::RecordCmd    => "record",
            CpuZone::QueueSubmit  => "submit",
            CpuZone::QueuePresent => "present",
        }
    }

    pub fn iter() -> impl Iterator<Item = CpuZone> {
        [
            CpuZone::WaitFences,
            CpuZone::AcquireImage,
            CpuZone::RecordCmd,
            CpuZone::QueueSubmit,
            CpuZone::QueuePresent,
        ].into_iter()
    }
}

/// Per-zone CPU timer. Pattern of use:
///
/// ```ignore
/// let mut t = CpuTimer::new();
/// t.begin_frame();
/// t.mark(CpuZone::WaitFences);
/// // … do wait …
/// t.mark(CpuZone::AcquireImage);
/// // … etc …
/// t.end_frame();
/// ```
///
/// Each `mark` records the elapsed time since the previous mark
/// (or `begin_frame` for the first mark of the frame) into the
/// zone *that just ended*. So calling `mark(X)` records "time
/// spent in the zone before X" — the zone label belongs to the
/// span ending at the mark.
///
/// Wait, that's confusing. Let me restate the contract:
///
/// `mark(Z)` says "the work labeled Z just finished." Time
/// recorded into Z = elapsed since previous mark. The first mark
/// after `begin_frame` records the time elapsed since `begin_frame`
/// into the zone at that mark.
///
/// Concretely, the renderer wraps each operation like:
///
/// ```ignore
/// t.begin_frame();
/// vk.wait_for_fences(...); t.mark(WaitFences);
/// vk.acquire_next_image(...); t.mark(AcquireImage);
/// record_cmd_buffer(...); t.mark(RecordCmd);
/// vk.queue_submit(...); t.mark(QueueSubmit);
/// vk.queue_present(...); t.mark(QueuePresent);
/// t.end_frame();
/// ```
pub struct CpuTimer {
    /// Last-mark instant; reset by `begin_frame`.
    last: Option<Instant>,
    /// Lifetime stats per zone.
    lifetime: BTreeMap<&'static str, ZoneStats>,
    /// 1-second window stats per zone, in microseconds. Reset by
    /// `flush_log`.
    window_us: BTreeMap<&'static str, u64>,
    window_frames: u32,
}

impl CpuTimer {
    pub fn new() -> Self {
        Self {
            last: None,
            lifetime: BTreeMap::new(),
            window_us: BTreeMap::new(),
            window_frames: 0,
        }
    }

    /// c.13-instrumentation: discard accumulated lifetime stats.
    /// Used at the warmup→steady boundary in spec runs so warmup
    /// frames don't pollute the assertion-evaluation samples.
    /// Does NOT touch the rolling window (that's for the per-second
    /// FPS log and rolls over on its own).
    pub fn reset_lifetime_stats(&mut self) {
        self.lifetime.clear();
    }

    /// Reset the per-frame timer. Call once at the start of the
    /// frame, before any `mark`.
    pub fn begin_frame(&mut self) {
        self.last = Some(Instant::now());
    }

    /// Record the elapsed time since the previous mark (or
    /// `begin_frame`) into the given zone, then reset the
    /// last-mark instant for the next zone.
    pub fn mark(&mut self, zone: CpuZone) {
        let now = Instant::now();
        let elapsed_us = match self.last {
            Some(prev) => now.saturating_duration_since(prev).as_micros() as u64,
            None => return, // begin_frame not called; skip silently
        };
        self.last = Some(now);
        let label = zone.name();
        *self.window_us.entry(label).or_insert(0) += elapsed_us;
        self.lifetime.entry(label).or_default().push(elapsed_us);
    }

    /// Mark frame completion. Increments the per-window frame
    /// counter; doesn't itself record any zone.
    pub fn end_frame(&mut self) {
        self.window_frames += 1;
    }

    /// Format and reset the per-zone window stats. Returns a one-
    /// line string suitable for the FPS log block.
    pub fn flush_log(&mut self) -> String {
        if self.window_frames == 0 {
            return "cpu_zones: <no frames yet>".to_string();
        }

        // Per-zone mean across the window. window_us is total μs;
        // divide by window_frames to get average per frame.
        let mut s = String::from("  cpu_zones:");
        let n = self.window_frames as f64;
        // Iterate in CpuZone enum order (fixed) for deterministic
        // log layout, not BTreeMap alphabetical.
        for zone in CpuZone::iter() {
            let label = zone.name();
            let total_us = *self.window_us.get(label).unwrap_or(&0);
            let mean_ms = (total_us as f64) / n / 1000.0;
            s.push_str(&format!(" {}={:.2}ms", label, mean_ms));
        }
        s.push_str(&format!(" (over {} frames)", self.window_frames));

        self.window_us.clear();
        self.window_frames = 0;
        s
    }

    /// Format the lifetime per-zone stats. Multi-line; intended
    /// for shutdown output. v0.24.2c.2: includes percentiles and
    /// over-budget reporting where applicable.
    pub fn flush_lifetime_summary(&self) -> String {
        if self.lifetime.is_empty() {
            return "CPU timer summary: no zones recorded".to_string();
        }
        let mut out = String::from("CPU timer lifetime summary:\n");
        let label_width = CpuZone::iter()
            .map(|z| z.name().len())
            .max()
            .unwrap_or(12);
        for zone in CpuZone::iter() {
            let label = zone.name();
            if let Some(st) = self.lifetime.get(label) {
                let budget_us = budget_for_cpu_zone(label);
                out.push_str(&unendlich_shared::zone_stats::render_zone_rich(
                    label, label_width, st, budget_us,
                ));
                out.push('\n');
            }
        }

        out.push_str("  ── box plots ──────────────────────────────────────────────────\n");
        for zone in CpuZone::iter() {
            let label = zone.name();
            if let Some(st) = self.lifetime.get(label) {
                out.push_str(&unendlich_shared::zone_stats::render_zone_boxplot(
                    label, label_width, st,
                ));
                out.push('\n');
            }
        }

        // Strip trailing newline.
        out.truncate(out.trim_end().len());
        out
    }

    /// v0.24.2c.9: structured per-zone lifetime stats for the
    /// `.unendresult.json` writer. Mirrors `GpuTimer::lifetime_zone_percentiles`.
    pub fn lifetime_zone_percentiles(&self) -> std::collections::BTreeMap<&'static str, crate::render::gpu_timer::ZonePercentiles> {
        let mut out = std::collections::BTreeMap::new();
        for (label, st) in &self.lifetime {
            if st.n == 0 { continue; }
            out.insert(*label, crate::render::gpu_timer::ZonePercentiles {
                n: st.n as usize,
                min_ms: st.min_us as f64 / 1000.0,
                p50_ms: st.percentile_us(0.50).unwrap_or(0) as f64 / 1000.0,
                p90_ms: st.percentile_us(0.90).unwrap_or(0) as f64 / 1000.0,
                p99_ms: st.percentile_us(0.99).unwrap_or(0) as f64 / 1000.0,
                max_ms: st.max_us as f64 / 1000.0,
                mean_ms: st.mean_us / 1000.0,
            });
        }
        out
    }
}
/// - wait_fences: huge variance is normal (vsync wait). 8.333ms is
///   "stutter rate" framing, so any value over that is informative.
/// - acquire: budget removed because real-world variability (vsync
///   waits land here too) means a fixed threshold is noisy. Just
///   show the distribution.
/// - record/submit/present: small overhead zones; >1ms is a sign of
///   contention or unexpected work.
fn budget_for_cpu_zone(label: &str) -> Option<u64> {
    match label {
        "wait_fences" => Some(8_333),
        "record" | "submit" | "present" => Some(1_000),
        _ => None,
    }
}

impl Default for CpuTimer {
    fn default() -> Self { Self::new() }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Zone names are stable strings — the lifetime summary and
    /// windowed log layout depend on them.
    #[test]
    fn zone_names_are_stable() {
        assert_eq!(CpuZone::WaitFences.name(),   "wait_fences");
        assert_eq!(CpuZone::AcquireImage.name(), "acquire");
        assert_eq!(CpuZone::RecordCmd.name(),    "record");
        assert_eq!(CpuZone::QueueSubmit.name(),  "submit");
        assert_eq!(CpuZone::QueuePresent.name(), "present");
    }

    /// `iter` yields all zones in declared order — the log format
    /// depends on this for deterministic output.
    #[test]
    fn iter_yields_declared_order() {
        let v: Vec<_> = CpuZone::iter().collect();
        assert_eq!(v, vec![
            CpuZone::WaitFences,
            CpuZone::AcquireImage,
            CpuZone::RecordCmd,
            CpuZone::QueueSubmit,
            CpuZone::QueuePresent,
        ]);
    }

    /// `mark` without a preceding `begin_frame` is a silent no-op.
    /// Catches a likely bug where someone instruments mid-frame
    /// instead of from the top.
    #[test]
    fn mark_without_begin_is_silent() {
        let mut t = CpuTimer::new();
        t.mark(CpuZone::WaitFences); // no panic, no log entry
        assert!(t.lifetime.is_empty());
    }

    /// Basic flow: begin, mark each zone, end. Each zone gets one
    /// observation; lifetime stats reflect that.
    #[test]
    fn full_frame_records_each_zone() {
        let mut t = CpuTimer::new();
        t.begin_frame();
        // Sleep nothing; just record marks back-to-back.
        for zone in CpuZone::iter() {
            t.mark(zone);
        }
        t.end_frame();

        // 5 zones, each got one observation.
        for zone in CpuZone::iter() {
            let label = zone.name();
            let st = t.lifetime.get(label)
                .expect(&format!("missing zone {}", label));
            assert_eq!(st.n, 1, "{}: expected 1 sample", label);
        }
    }

    /// Multiple frames accumulate observations.
    #[test]
    fn multiple_frames_accumulate() {
        let mut t = CpuTimer::new();
        for _ in 0..3 {
            t.begin_frame();
            t.mark(CpuZone::WaitFences);
            t.mark(CpuZone::QueuePresent);
            t.end_frame();
        }
        // Two zones got 3 observations each; others got 0.
        assert_eq!(t.lifetime.get("wait_fences").unwrap().n, 3);
        assert_eq!(t.lifetime.get("present").unwrap().n, 3);
        assert!(t.lifetime.get("acquire").is_none());
    }

    /// `flush_log` clears the window but preserves lifetime.
    #[test]
    fn flush_log_clears_window_preserves_lifetime() {
        let mut t = CpuTimer::new();
        t.begin_frame();
        t.mark(CpuZone::WaitFences);
        t.end_frame();

        let _line = t.flush_log();
        assert_eq!(t.window_frames, 0);
        assert!(t.window_us.is_empty());
        // Lifetime survives.
        assert_eq!(t.lifetime.get("wait_fences").unwrap().n, 1);
    }

    /// `flush_log` with no frames returns a sentinel string.
    #[test]
    fn flush_log_empty_returns_sentinel() {
        let mut t = CpuTimer::new();
        let line = t.flush_log();
        assert!(line.contains("no frames"));
    }

    /// Lifetime summary mentions every zone that received an
    /// observation, in the iter() order.
    #[test]
    fn lifetime_summary_orders_by_iter() {
        let mut t = CpuTimer::new();
        t.begin_frame();
        for zone in CpuZone::iter() {
            t.mark(zone);
        }
        t.end_frame();

        let summary = t.flush_lifetime_summary();
        // The order in which zone names appear in the summary
        // should match CpuZone::iter().
        let mut last_pos: usize = 0;
        for zone in CpuZone::iter() {
            let pos = summary.find(zone.name())
                .expect(&format!("zone {} missing from summary", zone.name()));
            assert!(pos >= last_pos,
                "zone {} appeared out of iter order", zone.name());
            last_pos = pos;
        }
    }
}
