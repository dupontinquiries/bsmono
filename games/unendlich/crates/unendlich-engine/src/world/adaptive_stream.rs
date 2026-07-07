//! Adaptive streaming — the CPU-side brains that let render distance grow
//! without the frame freezing. None of this touches the GPU; it *decides* how
//! much work the GPU path is allowed to do each frame so that a 7ms far-chunk
//! mesh can never land inside a 2–4ms frame (240–500Hz) and stutter the camera.
//!
//! Five pieces, all pure and testable:
//!
//! 1. [`FrameTimer`] — a smoothed (EWMA) frame-time estimate, so one spike
//!    doesn't crater the budget and one good frame doesn't overcommit.
//! 2. [`FrameBudget`] — splits the frame: an absolute **reserve** for
//!    input + camera + sim that streaming may never touch (this is what keeps
//!    the camera smooth), then a streaming slice from the leftover headroom that
//!    *shrinks* when we ran long, so a slow frame doesn't compound.
//! 3. [`in_view`] — culling with a "slight overlap": a near ring is always kept
//!    (so spinning the camera never reveals an unmeshed hole) and beyond it only
//!    the forward cone survives.
//! 4. [`drain_to_budget`] — orders the pending mesh/upload jobs by priority and
//!    takes only as many as fit the streaming budget this frame; jobs too big
//!    for any single frame are split off to the background worker pool (this is
//!    where the mesher becomes distributed) so they never block the frame.
//! 5. [`ResolutionController`] — scales the resolution of expensive long-range
//!    effects up/down from frame-time pressure, with hysteresis so it doesn't
//!    flip every frame.
//!
//! These are per-frame scheduling decisions (not simulation), so they use `f32`
//! / wall-clock microseconds and need no cross-frame determinism.

use crate::world::lod::LodLevel;

// ── 1. Smoothed frame time ──────────────────────────────────────────────────--

/// Exponentially-weighted moving average of frame time (µs). Alpha = 1/4:
/// responsive but a single spike only moves it a quarter of the way.
#[derive(Debug, Clone, Copy)]
pub struct FrameTimer {
    pub smoothed_us: u32,
}

impl FrameTimer {
    pub fn new(initial_us: u32) -> FrameTimer {
        FrameTimer { smoothed_us: initial_us }
    }
    pub fn sample(&mut self, frame_us: u32) -> u32 {
        let s = self.smoothed_us as i64;
        let n = frame_us as i64;
        self.smoothed_us = (s + (n - s) / 4).max(0) as u32;
        self.smoothed_us
    }
}

// ── 2. Frame budget (protect input/camera, amortize streaming) ────────────────

/// Splits each frame's time. The `reserve` is sacred — input, camera, and sim
/// run inside it every frame regardless of streaming pressure.
#[derive(Debug, Clone, Copy)]
pub struct FrameBudget {
    /// Target frame time, µs (4167 = 240Hz, 2000 = 500Hz).
    pub target_us: u32,
    /// Reserved each frame for input + camera + sim before any streaming.
    pub reserve_us: u32,
    /// A streaming floor so loading still creeps forward even under pressure.
    pub min_stream_us: u32,
}

impl FrameBudget {
    /// Build from a target refresh rate.
    pub fn for_hz(hz: u32, reserve_us: u32, min_stream_us: u32) -> FrameBudget {
        let target_us = if hz == 0 { 4167 } else { 1_000_000 / hz };
        FrameBudget { target_us, reserve_us, min_stream_us }
    }

    /// The µs of streaming work allowed this frame given the smoothed frame
    /// time. Always ≤ `target − reserve` (so the reserve is protected), and it
    /// backs off by the overrun when we ran long — but never below the floor.
    pub fn streaming_us(&self, smoothed_frame_us: u32) -> u32 {
        let headroom = self.target_us.saturating_sub(self.reserve_us);
        if smoothed_frame_us <= self.target_us {
            headroom
        } else {
            let over = smoothed_frame_us - self.target_us;
            headroom.saturating_sub(over).max(self.min_stream_us)
        }
    }
}

// ── 3. Culling with a camera-turn margin ──────────────────────────────────────

fn len(v: [f32; 3]) -> f32 {
    (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt()
}

/// Keep this chunk in the working set? Chunks within `keep_radius` of the camera
/// are always kept (the "slight overlap" — a quick turn never reveals a hole);
/// farther chunks survive only if inside the forward cone (`cam_fwd` is unit,
/// `fov_cos` = cosine of the half-angle, widened a little for margin).
pub fn in_view(
    chunk_center: [f32; 3],
    cam_pos: [f32; 3],
    cam_fwd: [f32; 3],
    fov_cos: f32,
    keep_radius: f32,
) -> bool {
    let to = [chunk_center[0] - cam_pos[0], chunk_center[1] - cam_pos[1], chunk_center[2] - cam_pos[2]];
    let d = len(to);
    if d <= keep_radius {
        return true;
    }
    if d == 0.0 {
        return true;
    }
    let dot = (to[0] * cam_fwd[0] + to[1] * cam_fwd[1] + to[2] * cam_fwd[2]) / d;
    dot >= fov_cos
}

// ── 4. The mesh/upload job queue ──────────────────────────────────────────────

/// One pending mesh-or-upload of a chunk.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MeshJob {
    pub chunk: [i32; 3],
    pub center: [f32; 3],
    pub lod: LodLevel,
    /// Measured/estimated cost on the frame thread, µs (from the run logs —
    /// e.g. a far dispatch ~7000). The caller supplies real numbers.
    pub cost_us: u32,
    /// Higher = more recently dirtied (tie-break toward fresh edits).
    pub dirty_seq: u64,
}

/// Scheduling score: higher runs sooner. Nearer, finer-LOD, and more-recently
/// dirtied jobs win. Distance dominates; LOD and recency break ties.
pub fn priority(job: &MeshJob, cam_pos: [f32; 3]) -> i64 {
    let to = [job.center[0] - cam_pos[0], job.center[1] - cam_pos[1], job.center[2] - cam_pos[2]];
    let dist = len(to);
    // Closer is better: large base minus distance (in blocks).
    let mut score = 1_000_000 - (dist as i64);
    // Finer LOD (Near=0) is more urgent than coarse (VeryFar=3).
    score += (LodLevel::COUNT as i64 - job.lod.index() as i64) * 4_000;
    // Gentle recency nudge.
    score += (job.dirty_seq % 1000) as i64;
    score
}

/// Result of scheduling a frame's worth of jobs.
#[derive(Debug, Clone, PartialEq)]
pub struct Schedule {
    /// Run on the frame thread this frame; `Σ cost_us ≤ budget` is guaranteed.
    pub run: Vec<MeshJob>,
    /// Didn't fit this frame; retry next frame (no work lost, no stall).
    pub deferred: Vec<MeshJob>,
    /// Too costly for any single frame at this budget — hand to the background
    /// worker pool so they never block a frame (the distributed mesher).
    pub offload: Vec<MeshJob>,
}

/// Order `jobs` by priority and take only what fits `budget_us` on the frame
/// thread; defer the rest; offload any single job that can't fit a frame.
/// Culls jobs outside the view first (see [`in_view`]).
#[allow(clippy::too_many_arguments)]
pub fn drain_to_budget(
    mut jobs: Vec<MeshJob>,
    cam_pos: [f32; 3],
    cam_fwd: [f32; 3],
    fov_cos: f32,
    keep_radius: f32,
    budget_us: u32,
) -> Schedule {
    jobs.retain(|j| in_view(j.center, cam_pos, cam_fwd, fov_cos, keep_radius));
    jobs.sort_by(|a, b| priority(b, cam_pos).cmp(&priority(a, cam_pos)));

    let mut run = Vec::new();
    let mut deferred = Vec::new();
    let mut offload = Vec::new();
    let mut spent: u32 = 0;
    for j in jobs {
        if j.cost_us > budget_us {
            offload.push(j); // can't fit a frame → background worker
        } else if spent + j.cost_us <= budget_us {
            spent += j.cost_us;
            run.push(j);
        } else {
            deferred.push(j); // fits some frame, just not what's left of this one
        }
    }
    Schedule { run, deferred, offload }
}

// ── 5. Adaptive resolution for long-range effects ─────────────────────────────

/// Scales the resolution of expensive wide-area effects (volumetrics, GI, water
/// reflections) by frame-time pressure. Hysteresis (`patience` frames of
/// sustained pressure) prevents per-frame flicker between scales.
#[derive(Debug, Clone, Copy)]
pub struct ResolutionController {
    pub scale: f32,
    pub min_scale: f32,
    pub step: f32,
    pub patience: u8,
    over: u8,
    under: u8,
}

impl ResolutionController {
    pub fn new(min_scale: f32, step: f32, patience: u8) -> ResolutionController {
        ResolutionController { scale: 1.0, min_scale, step, patience, over: 0, under: 0 }
    }

    /// Feed the smoothed frame time + target; returns the new scale in
    /// `[min_scale, 1.0]`. Only moves after `patience` consecutive frames on the
    /// same side of the target, so a lone spike changes nothing.
    pub fn update(&mut self, smoothed_us: u32, target_us: u32) -> f32 {
        if smoothed_us > target_us {
            self.over = self.over.saturating_add(1);
            self.under = 0;
        } else {
            self.under = self.under.saturating_add(1);
            self.over = 0;
        }
        if self.over >= self.patience {
            self.scale = (self.scale - self.step).max(self.min_scale);
            self.over = 0;
        } else if self.under >= self.patience && self.scale < 1.0 {
            self.scale = (self.scale + self.step).min(1.0);
            self.under = 0;
        }
        self.scale
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn job(center: [f32; 3], lod: LodLevel, cost_us: u32) -> MeshJob {
        MeshJob { chunk: [center[0] as i32, center[1] as i32, center[2] as i32], center, lod, cost_us, dirty_seq: 0 }
    }

    #[test]
    fn frame_timer_smooths_toward_samples() {
        let mut t = FrameTimer::new(2000);
        let a = t.sample(6000); // a big spike
        assert!(a > 2000 && a < 6000, "moves partway, not all the way: {a}");
        for _ in 0..40 {
            t.sample(6000);
        }
        assert!(t.smoothed_us > 5500, "converges to a sustained level: {}", t.smoothed_us);
    }

    #[test]
    fn budget_protects_the_reserve_and_backs_off_when_over() {
        let b = FrameBudget::for_hz(240, 1500, 200); // ~4167µs target
        let calm = b.streaming_us(3000); // under target → full headroom
        assert_eq!(calm, b.target_us - 1500);
        // Running long: streaming shrinks by the overrun, never below the floor,
        // and never eats the reserve.
        let pressed = b.streaming_us(6000);
        assert!(pressed < calm, "backs off under pressure: {pressed} < {calm}");
        assert!(pressed >= 200, "but keeps a floor");
        let crushed = b.streaming_us(50_000);
        assert_eq!(crushed, 200, "deep overrun → only the floor; reserve untouched");
    }

    #[test]
    fn five_hundred_hz_budget_is_tight() {
        let b = FrameBudget::for_hz(500, 1200, 100); // 2000µs target
        assert_eq!(b.target_us, 2000);
        assert_eq!(b.streaming_us(1500), 800, "only 800µs of streaming headroom at 500Hz");
    }

    #[test]
    fn near_ring_is_kept_regardless_of_facing() {
        let cam = [0.0, 0.0, 0.0];
        let fwd = [1.0, 0.0, 0.0]; // looking +x
        // A chunk directly behind, but within the keep radius → still kept.
        assert!(in_view([-5.0, 0.0, 0.0], cam, fwd, 0.5, 10.0), "near behind-camera chunk kept");
        // Far behind and outside the cone → culled.
        assert!(!in_view([-100.0, 0.0, 0.0], cam, fwd, 0.5, 10.0), "far behind chunk culled");
        // Far ahead, inside the cone → kept.
        assert!(in_view([100.0, 0.0, 0.0], cam, fwd, 0.5, 10.0), "far ahead chunk kept");
    }

    #[test]
    fn drain_never_exceeds_budget_and_orders_by_priority() {
        let cam = [0.0, 0.0, 0.0];
        let fwd = [1.0, 0.0, 0.0];
        let jobs = vec![
            job([80.0, 0.0, 0.0], LodLevel::Far, 1000), // far ahead
            job([10.0, 0.0, 0.0], LodLevel::Near, 1000), // near ahead — should go first
            job([40.0, 0.0, 0.0], LodLevel::Mid, 1000),
        ];
        let s = drain_to_budget(jobs, cam, fwd, 0.3, 16.0, 2000); // room for 2 of 3
        let cost: u32 = s.run.iter().map(|j| j.cost_us).sum();
        assert!(cost <= 2000, "never blows the frame budget: {cost}");
        assert_eq!(s.run.len(), 2);
        assert_eq!(s.deferred.len(), 1);
        // The nearest job ran.
        assert_eq!(s.run[0].center, [10.0, 0.0, 0.0], "nearest first");
    }

    #[test]
    fn oversized_jobs_are_offloaded_not_run_or_dropped() {
        let cam = [0.0, 0.0, 0.0];
        let fwd = [1.0, 0.0, 0.0];
        // A 7ms far mesh against a 3ms frame budget: cannot run on the frame.
        let jobs = vec![job([20.0, 0.0, 0.0], LodLevel::Far, 7000)];
        let s = drain_to_budget(jobs, cam, fwd, 0.3, 64.0, 3000);
        assert!(s.run.is_empty(), "never runs a frame-blowing job inline");
        assert_eq!(s.offload.len(), 1, "hands it to the background worker pool");
    }

    #[test]
    fn culled_jobs_are_not_scheduled() {
        let cam = [0.0, 0.0, 0.0];
        let fwd = [1.0, 0.0, 0.0];
        let jobs = vec![job([-200.0, 0.0, 0.0], LodLevel::Near, 100)]; // behind, far
        let s = drain_to_budget(jobs, cam, fwd, 0.5, 16.0, 5000);
        assert!(s.run.is_empty() && s.deferred.is_empty() && s.offload.is_empty(), "out of view → dropped from the queue");
    }

    #[test]
    fn resolution_drops_under_sustained_pressure_only() {
        let mut r = ResolutionController::new(0.5, 0.1, 3);
        let target = 4167;
        // One slow frame: no change (hysteresis).
        r.update(9000, target);
        assert_eq!(r.scale, 1.0, "a lone spike doesn't drop resolution");
        // Sustained overrun: drops.
        r.update(9000, target);
        r.update(9000, target);
        assert!(r.scale < 1.0, "sustained pressure lowers scale: {}", r.scale);
        // Sustained slack: recovers, clamped at 1.0.
        for _ in 0..30 {
            r.update(2000, target);
        }
        assert_eq!(r.scale, 1.0, "recovers to full when there's room");
    }

    #[test]
    fn resolution_never_drops_below_min() {
        let mut r = ResolutionController::new(0.5, 0.25, 1);
        for _ in 0..20 {
            r.update(100_000, 2000);
        }
        assert_eq!(r.scale, 0.5, "clamped at the minimum scale");
    }
}
