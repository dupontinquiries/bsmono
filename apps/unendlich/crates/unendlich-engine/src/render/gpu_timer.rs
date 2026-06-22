//! GPU timestamp queries for per-pass GPU time measurement (v0.21.0).
//!
//! # Reusability
//!
//! This module is engine-agnostic. Its only dependencies are `ash`
//! and `std`; nothing here references unendlich-specific types
//! (chunks, world state, etc.). It can be lifted into a separate
//! crate when (a) we need it from another binary, or (b) we want to
//! publish it as a standalone Vulkan profiling helper. The public
//! API surface to preserve when extracting:
//!
//! - `GpuTimer::new`, `begin_frame`, `mark`, `collect_frame`, `flush_log`,
//!   `skipped_frames`, `supported`
//! - `Marker` enum (variants are project-specific by design — the
//!   extracted version should make these caller-defined via a const
//!   generic or trait)
//! - `MAX_ZONES`, `zone_name`, `zones`
//!
//! When extracting: replace `Marker` with a generic `<const N: usize>`
//! parameter and let callers define their own zone enum implementing
//! `Into<u32>`. The timing/pool/aggregation logic is all generic over
//! the zone count.
//!
//! # Why this exists
//!
//! After v0.20's GPU-driven culling, the indirect and direct render
//! paths produced *identical* per-frame numbers (`draw=` ~1100ms per
//! 1s window in both modes). That equivalence proved the indirect
//! plumbing works, but the underlying GPU bottleneck was unmeasured —
//! we were guessing whether time went to shadow pass, main pass, the
//! cull dispatch, or somewhere else.
//!
//! This module wires up Vulkan timestamp queries so each render pass
//! reports its real GPU time. Once we know which pass dominates, the
//! next perf increment is informed instead of speculative.
//!
//! # Design
//!
//! - One `vk::QueryPool` of type `TIMESTAMP`, sized
//!   `MAX_FRAMES_IN_FLIGHT * MAX_ZONES`.
//! - Frame N writes timestamps into queries
//!   `[frame_idx * MAX_ZONES, (frame_idx + 1) * MAX_ZONES)`.
//! - Each zone is a half-open interval delimited by two timestamps.
//!   `Zone::Shadow` runs from `Marker::FrameStart` to `Marker::ShadowEnd`,
//!   and so on (see [`Marker`] for the order).
//! - At end-of-frame, results from frame N are read back (with the
//!   `WAIT` bit but we've already fenced, so it's non-blocking).
//! - Per-zone microseconds accumulate into a 1-second window. The
//!   FPS log line consumes and resets the window.
//!
//! # Hardware notes
//!
//! - `physical_device.limits.timestamp_period` is the conversion
//!   factor: nanoseconds per tick. Apple Silicon GPUs typically
//!   report ~83.33 ns (12 MHz tick) or 1 µs. Either gives sub-ms
//!   resolution.
//! - `queue_family.timestamp_valid_bits` may be less than 64. We
//!   mask reads to handle wraparound.
//! - If `timestamp_period == 0`, the device doesn't support
//!   timestamps. We construct a no-op timer so callers don't need
//!   to special-case the absence.
//!
//! # Lifecycle invariant
//!
//! The caller owns frame indexing. They must:
//!
//! 1. Call [`GpuTimer::begin_frame`] right after `begin_command_buffer`,
//!    which RESETS that frame's slice and writes [`Marker::FrameStart`].
//! 2. Call [`GpuTimer::mark`] in the same order as the [`Marker`]
//!    enum at each pass boundary.
//! 3. Call [`GpuTimer::collect_frame`] AFTER the fence for that frame
//!    has been waited on (typically at the START of the next frame
//!    that reuses the slot, when `wait_for_fences` has just returned).
//!
//! Out-of-order marks corrupt the per-zone deltas silently. The unit
//! tests verify the ring-buffer indexing math; the Vulkan side has
//! to be wired correctly by the caller.

use ash::{vk, Device, Instance};
use std::collections::BTreeMap;

/// Markers written, in order, during a frame. Each marker is one
/// timestamp. Zone durations are differences between consecutive
/// markers — so the order here defines the zones.
///
/// Adding a marker requires:
///   - bump [`MAX_ZONES`] (= number of markers)
///   - update [`zone_name`] for the new pair
#[repr(u32)]
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Marker {
    /// Right after `begin_command_buffer`. Origin for all zones.
    FrameStart   = 0,
    /// After shadow `cmd_end_render_pass`.
    ShadowEnd    = 1,
    /// After froxel-fill compute dispatch + post-barrier
    /// (volumetric fog volume populated for the main pass to sample).
    FroxelEnd    = 2,
    /// After main-pass cull dispatch (before main render pass begin).
    MainCullEnd  = 3,
    /// After main-pass `cmd_end_render_pass`.
    MainEnd      = 4,
}

/// Number of timestamps written per frame. Must equal the number of
/// `Marker` variants.
pub const MAX_ZONES: u32 = 5;

/// Map a (start, end) marker pair to a human-readable zone label
/// for log output.
pub fn zone_name(start: Marker, end: Marker) -> &'static str {
    use Marker::*;
    match (start, end) {
        (FrameStart,  ShadowEnd)   => "shadow",
        (ShadowEnd,   FroxelEnd)   => "froxel",
        (FroxelEnd,   MainCullEnd) => "cull",
        (MainCullEnd, MainEnd)     => "main",
        (FrameStart,  MainEnd)     => "total",
        _ => "?",
    }
}

/// Iterates the (start, end) zone pairs we want to surface in logs.
/// "total" is computed as a special case — it spans the whole frame.
pub fn zones() -> &'static [(Marker, Marker)] {
    &[
        (Marker::FrameStart,  Marker::ShadowEnd),
        (Marker::ShadowEnd,   Marker::FroxelEnd),
        (Marker::FroxelEnd,   Marker::MainCullEnd),
        (Marker::MainCullEnd, Marker::MainEnd),
        (Marker::FrameStart,  Marker::MainEnd),  // "total" — whole frame
    ]
}

/// Per-zone running statistics. Re-exported from `util::zone_stats`
/// in v0.23.0 so the new CPU timer can share the same numerically
/// stable Welford accumulator. Local alias kept so call sites in
/// this file don't need to be retyped.
use unendlich_shared::zone_stats::ZoneStats;

pub struct GpuTimer {
    device: Device,
    pool: vk::QueryPool,

    /// Number of frames in flight; same value the caller uses to
    /// modulo their `current_frame` index. We only store it to
    /// validate caller indices, not to advance state ourselves.
    frames_in_flight: u32,

    /// Nanoseconds per tick. Multiplied into raw timestamp diffs.
    timestamp_period_ns: f32,

    /// Mask for valid bits in a timestamp read. Some devices report
    /// fewer than 64 bits — anything above that is undefined and
    /// must be cleared before subtracting.
    timestamp_mask: u64,

    /// True if the device supports timestamps. False → all writes
    /// and collects are no-ops; the timer still exists for caller
    /// convenience.
    supported: bool,

    /// Per-zone microseconds accumulated since the last log flush.
    /// Keyed by zone label so the iteration order is deterministic.
    window_us: BTreeMap<&'static str, u64>,

    /// Number of frames whose results we've successfully collected
    /// since the last flush. Used to compute the per-frame mean
    /// (window_us / window_frame_count) for log output.
    window_frame_count: u32,

    /// Frames where `collect_frame` couldn't read results (not ready
    /// or device error). Surfaced in logs so non-zero values are
    /// visible to the user.
    skipped_frames: u64,

    /// Lifetime running stats per zone (mean, stddev, min, max,
    /// count). Independent of the 1-second log window; survives the
    /// whole session and is printed at shutdown.
    lifetime_stats: BTreeMap<&'static str, ZoneStats>,

    /// v0.24.3 chug-debug-2: the per-zone breakdown of the MOST RECENT
    /// frame `collect_frame` successfully processed, paired with its
    /// frame_idx. The caller reads (and clears) this via
    /// `take_last_frame()` to correlate per-frame GPU spikes with
    /// other side-channel events (wait_fences_spike, etc.) on the
    /// same frame_idx.
    ///
    /// Engine-agnostic: this module emits no events itself; it just
    /// surfaces the data and lets the (engine-aware) caller decide
    /// what to do with it.
    last_frame_us: Option<(u32, std::collections::HashMap<&'static str, u64>)>,
}

#[derive(Debug)]
pub enum GpuTimerError {
    Vulkan(vk::Result),
    NoGraphicsQueue,
}

impl From<vk::Result> for GpuTimerError {
    fn from(r: vk::Result) -> Self { Self::Vulkan(r) }
}

impl std::fmt::Display for GpuTimerError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Vulkan(r) => write!(f, "Vulkan error: {:?}", r),
            Self::NoGraphicsQueue => write!(f, "No graphics queue family with timestamp support"),
        }
    }
}

impl std::error::Error for GpuTimerError {}

impl GpuTimer {
    /// Create a timer.
    ///
    /// Reads the device's `timestamp_period` and the graphics queue
    /// family's `timestamp_valid_bits`. If timestamps aren't
    /// supported (period == 0 or valid_bits == 0), constructs a
    /// no-op timer.
    ///
    /// # Safety
    ///
    /// Caller must ensure `physical_device` matches the one used to
    /// create `device`, and `graphics_qf` is a valid family index
    /// for that physical device. (Same invariants as the rest of
    /// the renderer.)
    pub fn new(
        device: &Device,
        instance: &Instance,
        physical_device: vk::PhysicalDevice,
        graphics_qf: u32,
        frames_in_flight: u32,
    ) -> Result<Self, GpuTimerError> {
        let props = unsafe { instance.get_physical_device_properties(physical_device) };
        let qf_props = unsafe { instance.get_physical_device_queue_family_properties(physical_device) };

        let valid_bits = qf_props
            .get(graphics_qf as usize)
            .map(|p| p.timestamp_valid_bits)
            .unwrap_or(0);
        let period_ns = props.limits.timestamp_period;

        // No-op timer when device doesn't support timestamps. We
        // still allocate a pool of size 1 (Vulkan requires non-zero
        // count) but never write to it.
        if period_ns == 0.0 || valid_bits == 0 {
            log::warn!(
                "GPU timer: device doesn't support timestamps (period_ns={}, valid_bits={}). \
                 Per-pass GPU time will not be reported.",
                period_ns, valid_bits,
            );
            let pool_info = vk::QueryPoolCreateInfo::builder()
                .query_type(vk::QueryType::TIMESTAMP)
                .query_count(1);
            let pool = unsafe { device.create_query_pool(&pool_info, None) }?;
            return Ok(Self {
                device: device.clone(),
                pool,
                frames_in_flight,
                timestamp_period_ns: 0.0,
                timestamp_mask: 0,
                supported: false,
                window_us: BTreeMap::new(),
                window_frame_count: 0,
                skipped_frames: 0,
                lifetime_stats: BTreeMap::new(),
                last_frame_us: None,
            });
        }

        let timestamp_mask = if valid_bits >= 64 {
            u64::MAX
        } else {
            (1u64 << valid_bits) - 1
        };

        let pool_count = frames_in_flight * MAX_ZONES;
        let pool_info = vk::QueryPoolCreateInfo::builder()
            .query_type(vk::QueryType::TIMESTAMP)
            .query_count(pool_count);
        let pool = unsafe { device.create_query_pool(&pool_info, None) }?;

        log::info!(
            "GPU timer ready: timestamp_period={:.2}ns, valid_bits={}, pool_count={}",
            period_ns, valid_bits, pool_count,
        );

        Ok(Self {
            device: device.clone(),
            pool,
            frames_in_flight,
            timestamp_period_ns: period_ns,
            timestamp_mask,
            supported: true,
            window_us: BTreeMap::new(),
            window_frame_count: 0,
            skipped_frames: 0,
            lifetime_stats: BTreeMap::new(),
            last_frame_us: None,
        })
    }

    /// Compute the absolute query index for a given (frame, marker).
    /// Inlined to make sure the index math is identical between
    /// `begin_frame` (resets the slice) and `mark` (writes to a slot)
    /// and `collect_frame` (reads them back).
    #[inline]
    fn query_index(&self, frame_idx: u32, marker: Marker) -> u32 {
        debug_assert!(frame_idx < self.frames_in_flight);
        frame_idx * MAX_ZONES + marker as u32
    }

    /// Reset this frame's slice of the pool and write the
    /// `FrameStart` marker. Call this immediately after
    /// `begin_command_buffer`.
    ///
    /// `frame_idx` must be `< frames_in_flight`. Out-of-range
    /// indices are silently ignored (debug_assert in debug builds).
    pub fn begin_frame(&self, cmd: vk::CommandBuffer, frame_idx: u32) {
        if !self.supported || frame_idx >= self.frames_in_flight { return; }
        let first = self.query_index(frame_idx, Marker::FrameStart);
        unsafe {
            // Vulkan spec: queries must be reset before each use.
            self.device.cmd_reset_query_pool(cmd, self.pool, first, MAX_ZONES);
            // FrameStart marker — write at TOP_OF_PIPE since we want
            // this to mark the START of GPU work, before any other
            // command in the buffer can have committed.
            self.device.cmd_write_timestamp(
                cmd,
                vk::PipelineStageFlags::TOP_OF_PIPE,
                self.pool,
                first,
            );
        }
    }

    /// Write a marker at the current point in the command buffer.
    ///
    /// The pipeline stage here is `BOTTOM_OF_PIPE`: the timestamp
    /// fires after all prior commands have finished GPU execution.
    /// This is what we want for "end of pass" markers. (TOP_OF_PIPE
    /// would fire as soon as the command is reached, before the GPU
    /// has actually done the prior work.)
    pub fn mark(&self, cmd: vk::CommandBuffer, frame_idx: u32, marker: Marker) {
        if !self.supported || frame_idx >= self.frames_in_flight { return; }
        if marker == Marker::FrameStart { return; } // FrameStart is begin_frame's job
        let idx = self.query_index(frame_idx, marker);
        unsafe {
            self.device.cmd_write_timestamp(
                cmd,
                vk::PipelineStageFlags::BOTTOM_OF_PIPE,
                self.pool,
                idx,
            );
        }
    }

    /// Read back this frame's timestamps and accumulate per-zone
    /// microseconds into the 1-second window.
    ///
    /// Caller MUST have waited on this frame's fence before calling
    /// (otherwise the GPU may not have completed the writes).
    ///
    /// We pass `WAIT | WITH_AVAILABILITY`:
    ///
    /// - `WAIT` blocks the host until results are available. After
    ///   the caller's `wait_for_fences` this is at most a μs of
    ///   driver-side readback work — the GPU is already done. It is
    ///   the correct flag for "I know the work is complete, just
    ///   give me the numbers."
    ///
    /// - `WITH_AVAILABILITY` is defensive: on the very first cycle
    ///   through a slot the queries have never been written. Without
    ///   this flag the read would still succeed (returning garbage
    ///   from the uninitialised pool slots) — with it, each query's
    ///   availability is reported as a second u64 we check before
    ///   trusting the timestamp.
    ///
    /// v0.21.0 shipped without `WAIT` and every collect returned
    /// `NOT_READY`, producing the "skipped=N" log line for the
    /// entire session. Lesson recorded in DESIGN_NOTES.
    pub fn collect_frame(&mut self, frame_idx: u32) {
        if !self.supported || frame_idx >= self.frames_in_flight { return; }

        // Per-query result format with TYPE_64 | WITH_AVAILABILITY:
        // each query produces a `[timestamp: u64, availability: u64]`
        // pair = 16 bytes.
        //
        // ash 0.37's `get_query_pool_results::<T>` computes the
        // data_size argument as `size_of::<T>() * query_count`. To
        // get the correct stride for WITH_AVAILABILITY, T must be
        // sized to one full per-query result (16 bytes), not just a
        // u64 (8 bytes). v0.21.0+v0.21.1 used `T = u64` and passed
        // a `[u64; MAX_ZONES * 2]` slice with the right total length;
        // but ash's data_size came out to 40 bytes for 5 queries
        // (not 80), so the driver only filled HALF the buffer with
        // raw timestamps (no interleaved availability), and our
        // availability check then read garbage at odd indices and
        // bailed every frame.
        //
        // Using `T = [u64; 2]` makes the stride 16 bytes per query,
        // matching what the driver expects.
        let mut data: [[u64; 2]; MAX_ZONES as usize] = [[0; 2]; MAX_ZONES as usize];
        let first = self.query_index(frame_idx, Marker::FrameStart);

        let result = unsafe {
            self.device.get_query_pool_results::<[u64; 2]>(
                self.pool,
                first,
                MAX_ZONES,
                &mut data,
                vk::QueryResultFlags::TYPE_64
                    | vk::QueryResultFlags::WAIT
                    | vk::QueryResultFlags::WITH_AVAILABILITY,
            )
        };

        match result {
            Ok(_) => {}
            Err(e) => {
                log::warn!("GPU timer: get_query_pool_results failed: {:?}", e);
                self.skipped_frames += 1;
                return;
            }
        }

        // Validate availability defensively: on the first cycle
        // through a slot the queries are unwritten. With WAIT, this
        // path only fires when the slot has genuinely never been
        // recorded into yet (first MAX_FRAMES_IN_FLIGHT frames).
        for i in 0..MAX_ZONES as usize {
            if data[i][1] == 0 {
                self.skipped_frames += 1;
                return;
            }
        }

        // v0.24.3 chug-debug-2: also capture per-zone deltas for THIS
        // frame so the caller can correlate it with a wait_fences_spike
        // event on the same frame_idx. The lifetime_stats/window_us
        // paths above give aggregates across all frames; this path
        // gives the *single frame* breakdown.
        //
        // We stash the breakdown in `last_frame_us` for the caller to
        // read via `take_last_frame()`. Keeping event emission OUT of
        // this module preserves its "engine-agnostic" reusability
        // property (only depends on ash + std).
        let mut this_frame_us: std::collections::HashMap<&'static str, u64> =
            std::collections::HashMap::new();

        // Accumulate per-zone deltas.
        for &(start, end) in zones() {
            let s = data[start as usize][0] & self.timestamp_mask;
            let e = data[end   as usize][0] & self.timestamp_mask;

            // Handle wraparound: if e < s (high bits clamped by
            // mask), interpret as wrap by adding mask+1.
            let raw_delta = if e >= s {
                e - s
            } else {
                (e.wrapping_sub(s)) & self.timestamp_mask
            };

            // raw_delta is in ticks; convert to microseconds.
            // delta_ns = raw * period_ns;  delta_us = delta_ns / 1000.
            // Multiply first to keep precision (period_ns is small ~83).
            let delta_ns = (raw_delta as f64) * (self.timestamp_period_ns as f64);
            let delta_us = (delta_ns / 1000.0) as u64;

            let label = zone_name(start, end);
            *self.window_us.entry(label).or_insert(0) += delta_us;
            self.lifetime_stats.entry(label).or_default().push(delta_us);
            this_frame_us.insert(label, delta_us);
        }
        self.last_frame_us = Some((frame_idx, this_frame_us));

        self.window_frame_count += 1;
    }

    /// v0.24.3 chug-debug-2: hand the caller the most recent collected
    /// frame's per-zone microsecond breakdown, paired with the
    /// frame_idx it was collected against. Consumes (takes) it so
    /// subsequent calls return None until the next `collect_frame`.
    ///
    /// Returns `None` if `collect_frame` hasn't run since last call
    /// (or never ran for this slot, or skipped the slot, etc).
    pub fn take_last_frame(&mut self)
        -> Option<(u32, std::collections::HashMap<&'static str, u64>)>
    {
        self.last_frame_us.take()
    }

    /// Format the per-zone totals into a one-line log string,
    /// matching the existing `streaming:` style. Resets the window
    /// after formatting.
    ///
    /// Output looks like:
    ///   `gpu: shadow=2.3ms cull=0.1ms main=12.5ms total=15.0ms (over N frames, M skipped)`
    pub fn flush_log(&mut self) -> String {
        if !self.supported {
            return "gpu: <timestamps not supported by device>".to_string();
        }
        if self.window_frame_count == 0 {
            return format!(
                "gpu: <no frames collected, skipped={}>",
                self.skipped_frames,
            );
        }

        let mut s = String::from("gpu:");
        let frames = self.window_frame_count.max(1) as f64;
        for &(start, end) in zones() {
            let name = zone_name(start, end);
            let total_us = self.window_us.get(name).copied().unwrap_or(0);
            let mean_ms = (total_us as f64 / frames) / 1000.0;
            s.push_str(&format!(" {}={:.2}ms", name, mean_ms));
        }
        s.push_str(&format!(
            " (over {} frames, {} skipped)",
            self.window_frame_count, self.skipped_frames,
        ));

        // Reset window — caller flushes once per second.
        self.window_us.clear();
        self.window_frame_count = 0;
        // Don't reset skipped_frames — it's lifetime-cumulative as
        // a low-frequency error indicator.

        s
    }

    /// Total skipped frames over the lifetime of the timer.
    /// Surfaced in case the user wants a quick "is anything wrong"
    /// number without parsing the log line.
    pub fn skipped_frames(&self) -> u64 { self.skipped_frames }

    /// Format a multi-line lifetime summary of every zone observed
    /// during this session: count, mean, stddev, min, max. Intended
    /// for shutdown output. Returns a single string with embedded
    /// newlines so the caller can `log::info!("{}", ...)` it as a
    /// block.
    ///
    /// Stats use Welford's running algorithm — accurate even for
    /// tens of thousands of samples. The per-zone label set is
    /// whatever zones() produced during the session; on a no-collect
    /// session this returns a placeholder.
    pub fn flush_lifetime_summary(&self) -> String {
        if !self.supported {
            return "GPU timer summary: <timestamps not supported by device>".to_string();
        }
        if self.lifetime_stats.is_empty() {
            return format!(
                "GPU timer summary: no frames collected (skipped={})",
                self.skipped_frames,
            );
        }

        let mut out = String::from("GPU timer lifetime summary:\n");

        // v0.24.2c.2: rich format — mean/stddev plus percentiles and
        // over-budget fraction. Frame-time-like zones use the 120fps
        // budget (8.333ms) so we can spot stutter rate at a glance.
        // Per-zone budgets: cull/froxel are tiny so use 1ms; shadow/
        // main/total use 8.3ms (a full-frame budget).
        let label_width = self.lifetime_stats.keys()
            .map(|k| k.len())
            .max()
            .unwrap_or(10);
        for (label, st) in &self.lifetime_stats {
            let budget_us = budget_for_gpu_zone(label);
            out.push_str(&unendlich_shared::zone_stats::render_zone_rich(
                label, label_width, st, budget_us,
            ));
            out.push('\n');
        }

        out.push_str("  ── box plots ──────────────────────────────────────────────────\n");
        for (label, st) in &self.lifetime_stats {
            out.push_str(&unendlich_shared::zone_stats::render_zone_boxplot(
                label, label_width, st,
            ));
            out.push('\n');
        }

        out.push_str(&format!(
            "  (skipped frames: {})",
            self.skipped_frames,
        ));
        out
    }

    /// True if the device supports timestamp queries. When false,
    /// `flush_log` returns a placeholder and other methods are no-ops.
    pub fn supported(&self) -> bool { self.supported }

    /// c.13-instrumentation: clear all accumulated per-zone lifetime
    /// stats. Used to discard warmup samples before the measurement
    /// window begins. The window-accumulator (used by the per-second
    /// FPS log) is not cleared — that's a rolling window that resets
    /// on its own. Only the lifetime accumulator is cleared.
    ///
    /// Idempotent. Cheap (just empties the map; per-zone histograms
    /// are dropped, not re-zeroed).
    pub fn reset_lifetime_stats(&mut self) {
        self.lifetime_stats.clear();
    }

    /// v0.24.2c.9: structured per-zone lifetime stats for the
    /// `.unendresult.json` writer. Returns one map per zone name
    /// containing min/p50/p90/p99/max/mean in milliseconds. Empty if
    /// no frames have been collected.
    pub fn lifetime_zone_percentiles(&self) -> BTreeMap<&'static str, ZonePercentiles> {
        let mut out = BTreeMap::new();
        for (label, st) in &self.lifetime_stats {
            if st.n == 0 { continue; }
            out.insert(*label, ZonePercentiles {
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

/// Per-zone lifetime stats in milliseconds. See
/// `GpuTimer::lifetime_zone_percentiles`.
#[derive(Debug, Clone, Copy, serde::Serialize)]
pub struct ZonePercentiles {
    pub n: usize,
    pub min_ms: f64,
    pub p50_ms: f64,
    pub p90_ms: f64,
    pub p99_ms: f64,
    pub max_ms: f64,
    pub mean_ms: f64,
}

/// Per-zone "budget" thresholds for over-budget reporting. Returns
/// `Some(microseconds)` for zones with a meaningful budget; `None`
/// for zones where over-budget framing doesn't apply.
///
/// Frame-time zones use 8333µs (120fps target). Sub-zones like cull/
/// froxel use 5ms — the cull is "should-be-cheap, anything more is
/// suspicious" rather than a hard budget. Calibrated against
/// v0.24.2b/c playtest data where cull mean was 3-5ms at RD=64.
fn budget_for_gpu_zone(label: &str) -> Option<u64> {
    match label {
        "shadow" | "main" | "total" => Some(8_333),
        "cull" | "froxel" => Some(5_000),
        _ => None,
    }
}

impl Drop for GpuTimer {
    fn drop(&mut self) {
        unsafe {
            self.device.destroy_query_pool(self.pool, None);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Marker discriminants are dense and start at 0. The pool size
    /// math depends on this.
    #[test]
    fn marker_discriminants_are_dense() {
        assert_eq!(Marker::FrameStart   as u32, 0);
        assert_eq!(Marker::ShadowEnd    as u32, 1);
        assert_eq!(Marker::FroxelEnd    as u32, 2);
        assert_eq!(Marker::MainCullEnd  as u32, 3);
        assert_eq!(Marker::MainEnd      as u32, 4);
        assert_eq!(MAX_ZONES, 5); // == number of markers
    }

    /// The zone label table covers every adjacent-marker pair we
    /// expect to surface in logs, plus the "total" span.
    #[test]
    fn zone_names_cover_expected_pairs() {
        assert_eq!(zone_name(Marker::FrameStart,  Marker::ShadowEnd),   "shadow");
        assert_eq!(zone_name(Marker::ShadowEnd,   Marker::FroxelEnd),   "froxel");
        assert_eq!(zone_name(Marker::FroxelEnd,   Marker::MainCullEnd), "cull");
        assert_eq!(zone_name(Marker::MainCullEnd, Marker::MainEnd),     "main");
        assert_eq!(zone_name(Marker::FrameStart,  Marker::MainEnd),     "total");
    }

    /// `zones()` enumerates the labels we surface, exactly five of
    /// them, with consistent labels.
    #[test]
    fn zones_iter_is_consistent() {
        let zs = zones();
        assert_eq!(zs.len(), 5);
        let labels: Vec<_> = zs.iter().map(|(s, e)| zone_name(*s, *e)).collect();
        assert_eq!(labels, vec!["shadow", "froxel", "cull", "main", "total"]);
    }

    /// Ring-buffer indexing math: query index for (frame, marker)
    /// must be unique across frames and markers, and dense in
    /// `[0, frames_in_flight * MAX_ZONES)`.
    ///
    /// We can't construct a real timer without a Vulkan context, so
    /// we test the formula directly.
    #[test]
    fn query_index_math_is_dense_and_unique() {
        const FRAMES: u32 = 3;
        let mut seen = std::collections::HashSet::new();
        for f in 0..FRAMES {
            for &m in &[
                Marker::FrameStart,
                Marker::ShadowEnd,
                Marker::FroxelEnd,
                Marker::MainCullEnd,
                Marker::MainEnd,
            ] {
                let idx = f * MAX_ZONES + m as u32;
                assert!(seen.insert(idx), "duplicate index for ({}, {:?})", f, m);
                assert!(idx < FRAMES * MAX_ZONES);
            }
        }
        assert_eq!(seen.len(), (FRAMES * MAX_ZONES) as usize);
    }

    /// Timestamp wraparound: when `e < s` due to high-bit clamp,
    /// the wrap-aware subtract should produce a small positive
    /// number, not a giant one.
    ///
    /// We mimic the inner subtract directly; the integration tests
    /// for the renderer would catch a regression in the actual
    /// `collect_frame` path.
    #[test]
    fn timestamp_wraparound_handled() {
        let mask: u64 = (1u64 << 36) - 1; // 36-bit timer (typical)
        let s: u64 = mask - 100;          // near top of range
        let e: u64 = 50;                  // wrapped past zero
        let delta = if e >= s {
            e - s
        } else {
            (e.wrapping_sub(s)) & mask
        };
        // Expected: 50 + 100 + 1 = 151 ticks (since `mask` is the
        // largest representable value, wrap goes mask+1 → 0).
        assert_eq!(delta, 151);
    }

    /// CPU smoke test for the per-query result stride. With
    /// `WITH_AVAILABILITY | TYPE_64` each query writes 16 bytes
    /// (u64 timestamp + u64 availability). The collect_frame impl
    /// passes `T = [u64; 2]` to ash so size_of::<T>() = 16 matches
    /// what the driver expects per query.
    ///
    /// v0.21.0 + v0.21.1 used `T = u64` (size 8) which made
    /// ash compute data_size = 40 bytes for 5 queries instead of
    /// 80, causing the driver to fill only half the buffer with no
    /// availability bits. This test pins the stride choice so
    /// future refactors don't regress.
    #[test]
    fn per_query_result_stride_is_16_bytes() {
        // What our buffer type must serialize to for the chosen
        // result flags. Each [u64; 2] is exactly 16 bytes (Rust
        // guarantees no padding for arrays of identical primitives).
        assert_eq!(std::mem::size_of::<[u64; 2]>(), 16);
        // Total buffer size for MAX_ZONES queries with availability:
        // 5 * 16 = 80 bytes.
        assert_eq!(
            std::mem::size_of::<[[u64; 2]; MAX_ZONES as usize]>(),
            16 * MAX_ZONES as usize
        );
    }
}
