//! `sim::frame_correlation` — per-frame spike correlation probe (coordinator, r332-perf).
//!
//! The lifetime profiler (`profiler.rs`, `render::gpu_timer`, `render::mesher_timer`)
//! aggregates each zone **independently**, so a 639 ms `main`-pass max and a 1021 ms
//! `near`-mesh max in the same run can't be tied to the **same frame**. That makes the
//! frame-time tail (total p99 ≈ 265 ms, max ≈ 642 ms on the r331 M2 run) impossible to
//! attribute: is a 600 ms `main` spike an upload burst, a synchronous near-mesh, GPU
//! intrinsic cost, or a CPU record stall? This probe ingests **one correlated sample per
//! frame** and, when a frame busts the budget, (a) attributes the spike to its dominant
//! subsystem and (b) keeps the worst-N frames with all their counters — so the next M2 run
//! prints *what preceded* each hitch (uploaded-KB, near-mesh-ms, camera vertical velocity —
//! the suspected "stutter1" trigger) instead of only that a hitch happened.
//!
//! Pure: the bin feeds wall values in (no clock/IO/RNG here); no `unwrap`/`expect`/`panic!`;
//! deterministic; saturating arithmetic. Wiring is proposed in `HANDOFF_frame_correlation.md`.

/// One frame's correlated counters. The bin fills this each frame from the GPU/CPU/mesher
/// timers it already records, plus the camera's vertical speed.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FrameSample {
    pub frame: u64,
    pub total_ms: f32,
    pub main_ms: f32,
    pub cull_ms: f32,
    pub record_ms: f32,
    pub wait_fences_ms: f32,
    /// Largest near-LOD mesh-build time observed for this frame (ms).
    pub near_mesh_ms: f32,
    pub draw_count: u32,
    /// Mesh bytes uploaded this frame, in KiB (the deferred-upload-queue flush size).
    pub uploaded_kb: u32,
    pub chunks_meshed: u32,
    /// |camera vertical speed| in blocks/s — the suspected fast-descent stutter trigger.
    pub vert_velocity: f32,
}

impl FrameSample {
    /// A zeroed sample for `frame` (bin fills the fields it has).
    pub fn new(frame: u64) -> FrameSample {
        FrameSample {
            frame,
            total_ms: 0.0,
            main_ms: 0.0,
            cull_ms: 0.0,
            record_ms: 0.0,
            wait_fences_ms: 0.0,
            near_mesh_ms: 0.0,
            draw_count: 0,
            uploaded_kb: 0,
            chunks_meshed: 0,
            vert_velocity: 0.0,
        }
    }
}

/// Which subsystem a budget-busting frame is attributed to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SpikeCause {
    /// `main` pass dominates and little was uploaded — intrinsic GPU cost / overdraw.
    GpuMain,
    /// `main` (or total) dominated by a large upload this frame — flush burst / barrier wait.
    Upload,
    /// A synchronous near-LOD mesh build dominated the frame.
    Meshing,
    /// The cull dispatch spiked (pipeline stall feeding the main pass).
    Cull,
    /// CPU command recording dominated (per-instance record cost).
    CpuRecord,
    /// CPU blocked on fences with no single GPU pass tagged (GPU-bound, diffuse).
    GpuWait,
    /// No contributor clearly dominated.
    Unknown,
}

impl SpikeCause {
    pub const ALL: [SpikeCause; 7] = [
        SpikeCause::GpuMain,
        SpikeCause::Upload,
        SpikeCause::Meshing,
        SpikeCause::Cull,
        SpikeCause::CpuRecord,
        SpikeCause::GpuWait,
        SpikeCause::Unknown,
    ];
    pub fn label(self) -> &'static str {
        match self {
            SpikeCause::GpuMain => "GpuMain",
            SpikeCause::Upload => "Upload",
            SpikeCause::Meshing => "Meshing",
            SpikeCause::Cull => "Cull",
            SpikeCause::CpuRecord => "CpuRecord",
            SpikeCause::GpuWait => "GpuWait",
            SpikeCause::Unknown => "Unknown",
        }
    }
    fn index(self) -> usize {
        match self {
            SpikeCause::GpuMain => 0,
            SpikeCause::Upload => 1,
            SpikeCause::Meshing => 2,
            SpikeCause::Cull => 3,
            SpikeCause::CpuRecord => 4,
            SpikeCause::GpuWait => 5,
            SpikeCause::Unknown => 6,
        }
    }
}

/// KiB uploaded above which a `main`-dominated spike is read as upload-induced rather
/// than intrinsic GPU cost. Tunable; conservative default.
pub const UPLOAD_BURST_KB: u32 = 256;

/// Attribute a frame to its dominant contributor. Returns the cause regardless of budget
/// (the probe only calls this for over-budget frames). The four timed contributors plus
/// the upload signal are compared; the largest wins, with the upload split applied when
/// `main` leads.
pub fn classify(s: &FrameSample) -> SpikeCause {
    // The contributors we can rank. Meshing is off-thread but a big synchronous near-mesh
    // is the documented hitch, so it competes here.
    let contenders = [
        (s.main_ms, 0u8),
        (s.near_mesh_ms, 1u8),
        (s.cull_ms, 2u8),
        (s.record_ms, 3u8),
        (s.wait_fences_ms, 4u8),
    ];
    let mut best = 0.0f32;
    let mut who = 255u8;
    for &(v, tag) in &contenders {
        if v > best {
            best = v;
            who = tag;
        }
    }
    match who {
        0 => {
            if s.uploaded_kb >= UPLOAD_BURST_KB {
                SpikeCause::Upload
            } else {
                SpikeCause::GpuMain
            }
        }
        1 => SpikeCause::Meshing,
        2 => SpikeCause::Cull,
        3 => SpikeCause::CpuRecord,
        4 => SpikeCause::GpuWait,
        _ => SpikeCause::Unknown,
    }
}

/// Rolling per-frame correlation probe: counts spikes by cause and retains the worst-N
/// frames (by `total_ms`) with their full counters for the end-of-session summary.
#[derive(Clone, Debug)]
pub struct CorrelationProbe {
    budget_ms: f32,
    cap: usize,
    worst: Vec<FrameSample>, // kept sorted descending by total_ms, len <= cap
    cause_counts: [u32; 7],
    spike_count: u64,
    frame_count: u64,
}

impl CorrelationProbe {
    /// `budget_ms` = the per-frame budget a frame must exceed to count as a spike
    /// (e.g. 8.3 for 120 Hz, 16.6 for 60 Hz). `cap` = how many worst frames to retain.
    pub fn new(budget_ms: f32, cap: usize) -> CorrelationProbe {
        CorrelationProbe {
            budget_ms,
            cap,
            worst: Vec::new(),
            cause_counts: [0; 7],
            spike_count: 0,
            frame_count: 0,
        }
    }

    /// Observe one frame. Returns `Some(cause)` if it busted the budget (and records it),
    /// else `None`.
    pub fn observe(&mut self, s: FrameSample) -> Option<SpikeCause> {
        self.frame_count = self.frame_count.saturating_add(1);
        if !(s.total_ms > self.budget_ms) {
            return None; // NaN-safe: NaN totals are not counted as spikes
        }
        self.spike_count = self.spike_count.saturating_add(1);
        let cause = classify(&s);
        self.cause_counts[cause.index()] = self.cause_counts[cause.index()].saturating_add(1);
        self.insert_worst(s);
        Some(cause)
    }

    fn insert_worst(&mut self, s: FrameSample) {
        // Insertion-sort into the descending worst-list, bounded at `cap`.
        if self.cap == 0 {
            return;
        }
        let mut pos = self.worst.len();
        for (i, w) in self.worst.iter().enumerate() {
            if s.total_ms > w.total_ms {
                pos = i;
                break;
            }
        }
        if pos < self.cap {
            self.worst.insert(pos, s);
            if self.worst.len() > self.cap {
                self.worst.truncate(self.cap);
            }
        } else if self.worst.len() < self.cap {
            self.worst.push(s);
        }
    }

    pub fn spike_count(&self) -> u64 {
        self.spike_count
    }
    pub fn frame_count(&self) -> u64 {
        self.frame_count
    }
    pub fn cause_count(&self, c: SpikeCause) -> u32 {
        self.cause_counts[c.index()]
    }
    pub fn worst(&self) -> &[FrameSample] {
        &self.worst
    }

    /// Spike fraction in `[0,1]` (0 when no frames observed).
    pub fn spike_rate(&self) -> f32 {
        if self.frame_count == 0 {
            0.0
        } else {
            self.spike_count as f32 / self.frame_count as f32
        }
    }

    /// Greppable end-of-session summary: per-cause spike counts + the worst frames with the
    /// counters that explain them. One `[FRAMECORR]` line plus one line per retained frame.
    pub fn summary(&self) -> String {
        let mut out = String::new();
        out.push_str(&format!(
            "[FRAMECORR] frames={} spikes={} ({:.1}%) budget={:.1}ms by_cause:",
            self.frame_count,
            self.spike_count,
            self.spike_rate() * 100.0,
            self.budget_ms,
        ));
        for c in SpikeCause::ALL {
            let n = self.cause_count(c);
            if n > 0 {
                out.push_str(&format!(" {}={}", c.label(), n));
            }
        }
        out.push('\n');
        for (i, s) in self.worst.iter().enumerate() {
            out.push_str(&format!(
                "[FRAMECORR]  #{:<2} frame={} total={:.1}ms cause={} main={:.1} near_mesh={:.1} \
                 cull={:.1} record={:.1} wait={:.1} up={}kB draws={} meshed={} vvel={:.1}\n",
                i + 1,
                s.frame,
                s.total_ms,
                classify(s).label(),
                s.main_ms,
                s.near_mesh_ms,
                s.cull_ms,
                s.record_ms,
                s.wait_fences_ms,
                s.uploaded_kb,
                s.draw_count,
                s.chunks_meshed,
                s.vert_velocity,
            ));
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(frame: u64, total: f32) -> FrameSample {
        let mut s = FrameSample::new(frame);
        s.total_ms = total;
        s
    }

    #[test]
    fn under_budget_is_not_a_spike() {
        let mut p = CorrelationProbe::new(8.3, 8);
        assert_eq!(p.observe(sample(1, 5.0)), None);
        assert_eq!(p.spike_count(), 0);
        assert_eq!(p.frame_count(), 1);
    }

    #[test]
    fn over_budget_is_a_spike_and_counted() {
        let mut p = CorrelationProbe::new(8.3, 8);
        let mut s = sample(1, 600.0);
        s.main_ms = 590.0;
        assert!(p.observe(s).is_some());
        assert_eq!(p.spike_count(), 1);
    }

    #[test]
    fn nan_total_is_not_a_spike() {
        let mut p = CorrelationProbe::new(8.3, 8);
        assert_eq!(p.observe(sample(1, f32::NAN)), None);
        assert_eq!(p.spike_count(), 0);
    }

    #[test]
    fn classify_meshing_when_near_mesh_dominates() {
        let mut s = sample(1, 1020.0);
        s.near_mesh_ms = 1010.0;
        s.main_ms = 12.0;
        assert_eq!(classify(&s), SpikeCause::Meshing);
    }

    #[test]
    fn classify_upload_vs_gpumain_on_upload_size() {
        let mut s = sample(1, 640.0);
        s.main_ms = 630.0;
        s.uploaded_kb = UPLOAD_BURST_KB; // burst → upload-induced
        assert_eq!(classify(&s), SpikeCause::Upload);
        s.uploaded_kb = 4; // tiny upload, big main → intrinsic GPU
        assert_eq!(classify(&s), SpikeCause::GpuMain);
    }

    #[test]
    fn classify_cull_record_and_wait() {
        let mut s = sample(1, 50.0);
        s.cull_ms = 46.0;
        assert_eq!(classify(&s), SpikeCause::Cull);
        let mut s = sample(2, 20.0);
        s.record_ms = 13.0;
        assert_eq!(classify(&s), SpikeCause::CpuRecord);
        let mut s = sample(3, 350.0);
        s.wait_fences_ms = 347.0;
        assert_eq!(classify(&s), SpikeCause::GpuWait);
    }

    #[test]
    fn worst_keeps_top_n_by_total_descending() {
        let mut p = CorrelationProbe::new(8.3, 3);
        for (f, t) in [(1, 50.0), (2, 600.0), (3, 20.0), (4, 300.0), (5, 9.0)] {
            p.observe(sample(f, t));
        }
        let w = p.worst();
        assert_eq!(w.len(), 3, "bounded at cap");
        assert_eq!(w[0].total_ms, 600.0);
        assert_eq!(w[1].total_ms, 300.0);
        assert_eq!(w[2].total_ms, 50.0);
    }

    #[test]
    fn spike_rate_matches_counts() {
        let mut p = CorrelationProbe::new(8.3, 8);
        for f in 0..10 {
            // half spike
            p.observe(sample(f, if f % 2 == 0 { 20.0 } else { 5.0 }));
        }
        assert_eq!(p.spike_count(), 5);
        assert!((p.spike_rate() - 0.5).abs() < 1e-6);
    }

    #[test]
    fn summary_is_greppable_and_lists_cause_and_worst() {
        let mut p = CorrelationProbe::new(8.3, 4);
        let mut s = sample(42, 640.0);
        s.main_ms = 630.0;
        s.uploaded_kb = 1024;
        s.vert_velocity = 14.0;
        p.observe(s);
        let out = p.summary();
        assert!(out.contains("[FRAMECORR]"));
        assert!(out.contains("Upload=1"));
        assert!(out.contains("frame=42"));
        assert!(out.contains("vvel=14.0"));
    }

    #[test]
    fn cap_zero_keeps_no_frames_but_still_counts() {
        let mut p = CorrelationProbe::new(8.3, 0);
        p.observe(sample(1, 600.0));
        assert_eq!(p.spike_count(), 1);
        assert!(p.worst().is_empty());
    }

    #[test]
    fn observe_is_deterministic() {
        let run = || {
            let mut p = CorrelationProbe::new(8.3, 4);
            for f in 0..6 {
                let mut s = sample(f, (f as f32) * 100.0);
                s.main_ms = (f as f32) * 90.0;
                p.observe(s);
            }
            p.summary()
        };
        assert_eq!(run(), run());
    }
}
